use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

use super::token_store::TokenData;

// Standard public client ID for Microsoft Teams Web or Azure App
pub const DEFAULT_CLIENT_ID: &str = "1fec8e78-bce4-4aaf-ab1b-5451cc387264";
pub const DEFAULT_SCOPES: &str = "https://api.spaces.skype.com/.default offline_access openid profile email";

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct DeviceCodeResponse {
    pub user_code: String,
    pub device_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: Option<u64>,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphMeResponse {
    id: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    mail: Option<String>,
    #[serde(rename = "userPrincipalName")]
    user_principal_name: Option<String>,
}

pub struct OAuthManager {
    client_id: String,
    http_client: Client,
}

impl Default for OAuthManager {
    fn default() -> Self {
        Self::new(None)
    }
}

impl OAuthManager {
    pub fn new(custom_client_id: Option<String>) -> Self {
        let client_id = custom_client_id
            .or_else(|| std::env::var("TEAMS_CLIENT_ID").ok())
            .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string());

        Self {
            client_id,
            http_client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Step 1: Request a device authorization code from Microsoft Identity Platform
    pub async fn request_device_code(&self) -> Result<DeviceCodeResponse> {
        let url = "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";
        let params = [
            ("client_id", self.client_id.as_str()),
            ("scope", DEFAULT_SCOPES),
        ];

        let resp = self.http_client
            .post(url)
            .form(&params)
            .send()
            .await
            .context("Failed to request device code from Microsoft")?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            bail!("Failed to get device code from Microsoft: {}", err);
        }

        let device_resp: DeviceCodeResponse = resp.json().await.context("Failed to parse device code response")?;
        Ok(device_resp)
    }

    /// Step 2: Poll token endpoint until user approves the sign-in in browser
    pub async fn poll_device_code(&self, device_code: &str, poll_interval: u64, expires_in: u64) -> Result<TokenData> {
        let token_url = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
        let interval_duration = Duration::from_secs(poll_interval.max(3));
        let deadline = SystemTime::now() + Duration::from_secs(expires_in);

        while SystemTime::now() < deadline {
            sleep(interval_duration).await;

            let params = [
                ("client_id", self.client_id.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ];

            let resp = self.http_client
                .post(token_url)
                .form(&params)
                .send()
                .await
                .context("Failed to poll token endpoint")?;

            if resp.status().is_success() {
                let token_resp: TokenResponse = resp.json().await.context("Failed to parse token response")?;
                let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                let expires_at = now + token_resp.expires_in.unwrap_or(3600);

                let mut token_data = TokenData {
                    access_token: token_resp.access_token,
                    refresh_token: token_resp.refresh_token,
                    expires_at,
                    user_id: None,
                    user_email: None,
                    display_name: None,
                };

                // Fetch profile info
                if let Ok(profile) = self.fetch_user_profile(&token_data.access_token).await {
                    token_data.user_id = profile.id;
                    token_data.display_name = profile.display_name;
                    token_data.user_email = profile.mail.or(profile.user_principal_name);
                }

                return Ok(token_data);
            }

            // Check if still pending
            let err_body = resp.text().await.unwrap_or_default();
            if let Ok(err_obj) = serde_json::from_str::<TokenErrorResponse>(&err_body) {
                match err_obj.error.as_str() {
                    "authorization_pending" => {
                        // User hasn't finished logging in yet, continue polling
                        continue;
                    }
                    "slow_down" => {
                        sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                    "authorization_declined" => {
                        bail!("Sign-in request was declined in the browser.");
                    }
                    "expired_token" => {
                        bail!("The sign-in code has expired. Please try again.");
                    }
                    other => {
                        bail!("Authentication error: {} - {}", other, err_obj.error_description.unwrap_or_default());
                    }
                }
            } else {
                bail!("Token endpoint returned unexpected error: {}", err_body);
            }
        }

        bail!("Device login timed out. Please try again.");
    }

    /// Refreshes the access token using a refresh token
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenData> {
        let token_url = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
        let params = [
            ("client_id", self.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", DEFAULT_SCOPES),
        ];

        let resp = self.http_client
            .post(token_url)
            .form(&params)
            .send()
            .await
            .context("Failed to contact refresh token endpoint")?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            bail!("Refresh token failed: {}", err);
        }

        let token_resp: TokenResponse = resp.json().await?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let expires_at = now + token_resp.expires_in.unwrap_or(3600);

        Ok(TokenData {
            access_token: token_resp.access_token,
            refresh_token: token_resp.refresh_token.or_else(|| Some(refresh_token.to_string())),
            expires_at,
            user_id: None,
            user_email: None,
            display_name: None,
        })
    }

    async fn fetch_user_profile(&self, access_token: &str) -> Result<GraphMeResponse> {
        let resp = self.http_client
            .get("https://graph.microsoft.com/v1.0/me")
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await?;

        if resp.status().is_success() {
            let me: GraphMeResponse = resp.json().await?;
            Ok(me)
        } else {
            bail!("Failed to get user profile from Graph")
        }
    }
}
