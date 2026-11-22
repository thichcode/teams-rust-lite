use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

use super::token_store::{AccountType, TokenData};

// Standard public client ID for Microsoft Teams for Work/School
pub const TEAMS_WORK_CLIENT_ID: &str = "1fec8e78-bce4-4aaf-ab1b-5451cc387264";
// First-party public client ID for Microsoft Office (supports Personal / Consumer accounts)
pub const TEAMS_PERSONAL_CLIENT_ID: &str = "d3590ed6-52b3-4102-aeff-aad2292ab01c";

#[allow(dead_code)]
pub const DEFAULT_CLIENT_ID: &str = TEAMS_WORK_CLIENT_ID;
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
#[allow(dead_code)]
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
            .unwrap_or_else(|| TEAMS_WORK_CLIENT_ID.to_string());

        Self {
            client_id,
            http_client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn get_client_id(&self, account_type: AccountType) -> &str {
        match account_type {
            AccountType::WorkOrSchool => &self.client_id,
            AccountType::Personal => TEAMS_PERSONAL_CLIENT_ID,
        }
    }

    /// Step 1: Request a device authorization code from Microsoft Identity Platform
    pub async fn request_device_code(&self, account_type: AccountType) -> Result<DeviceCodeResponse> {
        let (tenant, client_id) = match account_type {
            AccountType::WorkOrSchool => ("organizations", self.get_client_id(account_type)),
            AccountType::Personal => ("common", TEAMS_PERSONAL_CLIENT_ID),
        };
        let url = format!("https://login.microsoftonline.com/{}/oauth2/v2.0/devicecode", tenant);
        log::info!("Requesting device code from {} with client_id {} for {:?}", url, client_id, account_type);

        let params = [
            ("client_id", client_id),
            ("scope", DEFAULT_SCOPES),
        ];

        let resp = self.http_client
            .post(&url)
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
    pub async fn poll_device_code(
        &self,
        device_code: &str,
        poll_interval: u64,
        expires_in: u64,
        account_type: AccountType,
    ) -> Result<TokenData> {
        let (tenant, client_id) = match account_type {
            AccountType::WorkOrSchool => ("organizations", self.get_client_id(account_type)),
            AccountType::Personal => ("common", TEAMS_PERSONAL_CLIENT_ID),
        };
        let token_url = format!("https://login.microsoftonline.com/{}/oauth2/v2.0/token", tenant);
        log::info!("Polling token endpoint at {} with client_id {} for {:?}", token_url, client_id, account_type);

        let interval_duration = Duration::from_secs(poll_interval.max(3));
        let deadline = SystemTime::now() + Duration::from_secs(expires_in);

        while SystemTime::now() < deadline {
            sleep(interval_duration).await;

            let params = [
                ("client_id", client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ];

            let resp = self.http_client
                .post(&token_url)
                .form(&params)
                .send()
                .await
                .context("Failed to poll token endpoint")?;

            if resp.status().is_success() {
                let token_resp: TokenResponse = resp.json().await.context("Failed to parse token response")?;
                let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                let expires_at = now + token_resp.expires_in.unwrap_or(3600);

                let mut token_data = TokenData {
                    access_token: token_resp.access_token.clone(),
                    refresh_token: token_resp.refresh_token.clone(),
                    expires_at,
                    user_id: None,
                    user_email: None,
                    display_name: None,
                    account_type,
                };

                // Extract user claims directly from Access Token or ID Token JWT
                let jwt_candidate = token_resp.id_token.as_deref().unwrap_or(&token_resp.access_token);
                if let Some((name, email, uid)) = parse_jwt_claims(jwt_candidate) {
                    token_data.display_name = name;
                    token_data.user_email = email;
                    token_data.user_id = uid;
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
    pub async fn refresh_token(&self, refresh_token: &str, account_type: AccountType) -> Result<TokenData> {
        let (tenant, client_id) = match account_type {
            AccountType::WorkOrSchool => ("organizations", self.get_client_id(account_type)),
            AccountType::Personal => ("common", TEAMS_PERSONAL_CLIENT_ID),
        };
        let token_url = format!("https://login.microsoftonline.com/{}/oauth2/v2.0/token", tenant);
        log::info!("Refreshing token at {} with client_id {} for {:?}", token_url, client_id, account_type);

        let params = [
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", DEFAULT_SCOPES),
        ];

        let resp = self.http_client
            .post(&token_url)
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
            account_type,
        })
    }

    #[allow(dead_code)]
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

/// Decodes standard JWT payload without external signature validation
pub fn parse_jwt_claims(token: &str) -> Option<(Option<String>, Option<String>, Option<String>)> {
    use base64::Engine as _;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }

    let payload_b64 = parts[1];
    // Add padding if missing
    let padded = match payload_b64.len() % 4 {
        2 => format!("{}==", payload_b64),
        3 => format!("{}=", payload_b64),
        _ => payload_b64.to_string(),
    };

    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(&padded)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&padded))
        .ok()?;

    let val = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    let name = val.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
    let email = val.get("preferred_username")
        .or_else(|| val.get("upn"))
        .or_else(|| val.get("email"))
        .or_else(|| val.get("unique_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let uid = val.get("oid")
        .or_else(|| val.get("sub"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some((name, email, uid))
}
