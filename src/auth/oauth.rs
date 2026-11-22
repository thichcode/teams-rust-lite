use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::sleep;

use super::token_store::{AccountType, TokenData};

// Standard public client ID for Microsoft Teams Desktop (Work/School only)
pub const TEAMS_WORK_CLIENT_ID: &str = "1fec8e78-bce4-4aaf-ab1b-5451cc387264";
// First-party public client ID with native loopback support on Microsoft Accounts (Consumers / Hotmail / Outlook)
pub const TEAMS_PERSONAL_CLIENT_ID: &str = "04b07795-8ddb-461a-bbee-02f9e1bf7b46";
// Fallback redirect URI
#[allow(dead_code)]
pub const NATIVE_CLIENT_REDIRECT: &str = "https://login.microsoftonline.com/common/oauth2/nativeclient";

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

    /// Interactive browser login via local loopback listener (RFC 8252)
    /// For Work/School: uses Teams client ID with localhost redirect (direct callback)
    /// For Personal/Teams Free: uses Office client ID with nativeclient redirect + localhost relay page
    pub async fn login_interactive_loopback(
        &self,
        account_type: AccountType,
    ) -> Result<TokenData> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("Failed to bind loopback TCP listener")?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://localhost:{}", port);

        let (client_id, tenant) = match account_type {
            AccountType::WorkOrSchool => (
                self.get_client_id(account_type),
                "organizations",
            ),
            AccountType::Personal => (
                TEAMS_PERSONAL_CLIENT_ID,
                "consumers",
            ),
        };

        let (verifier, challenge) = generate_pkce();
        let state = uuid::Uuid::new_v4().to_string();

        let auth_url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/authorize?\
             client_id={}&\
             response_type=code&\
             redirect_uri={}&\
             response_mode=query&\
             scope={}&\
             state={}&\
             code_challenge={}&\
             code_challenge_method=S256",
            tenant,
            client_id,
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(DEFAULT_SCOPES),
            state,
            challenge
        );

        log::info!("Opening browser for {:?} loopback login on port {}: {}", account_type, port, auth_url);
        let _ = open::that(&auth_url);

        let timeout_duration = Duration::from_secs(300);
        let code = self.serve_personal_relay(listener, timeout_duration).await?;

        log::info!("Received authorization code (len: {}). Exchanging for tokens...", code.len());
        self.exchange_code_for_tokens(&code, client_id, tenant, &redirect_uri, &verifier, account_type).await
    }

    /// Serves loopback listener on localhost for authorization callback
    /// Handles both direct Microsoft redirect (/?code=...) and manual paste form (/callback?code=...)
    async fn serve_personal_relay(
        &self,
        listener: tokio::net::TcpListener,
        timeout: Duration,
    ) -> Result<String> {
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let accept_result = tokio::time::timeout_at(deadline, listener.accept()).await;
            let (mut socket, _) = match accept_result {
                Ok(Ok(conn)) => conn,
                Ok(Err(e)) => {
                    log::warn!("Relay listener accept error: {}", e);
                    continue;
                }
                Err(_) => bail!("Timed out waiting for sign-in callback"),
            };

            let mut buf = [0u8; 8192];
            let n = match socket.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => continue,
            };
            let req_str = String::from_utf8_lossy(&buf[..n]);
            let req_line = req_str.lines().next().unwrap_or_default();
            let path = req_line.split_whitespace().nth(1).unwrap_or("/");

            if path.contains("code=") || path.starts_with("/callback") {
                // Authorization code received directly from Microsoft or relay form
                let full_url = format!("http://localhost{}", path);
                if let Ok(parsed) = url::Url::parse(&full_url) {
                    let (code_opt, error_opt) = extract_code_from_url(&parsed);

                    let html_body = success_or_error_html(code_opt.is_some());
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        html_body.len(), html_body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;

                    if let Some(err) = error_opt {
                        bail!("Microsoft login error: {}", err);
                    }
                    if let Some(code) = code_opt {
                        return Ok(code);
                    }
                }
            } else {
                // Serve relay HTML page if opened directly
                let relay_html = PERSONAL_RELAY_HTML;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    relay_html.len(), relay_html
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        }
    }

    async fn exchange_code_for_tokens(
        &self,
        code: &str,
        client_id: &str,
        tenant: &str,
        redirect_uri: &str,
        verifier: &str,
        account_type: AccountType,
    ) -> Result<TokenData> {
        let token_url = format!("https://login.microsoftonline.com/{}/oauth2/v2.0/token", tenant);
        let params = [
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("scope", DEFAULT_SCOPES),
        ];

        let resp = self.http_client
            .post(&token_url)
            .form(&params)
            .send()
            .await
            .context("Failed to exchange authorization code for tokens")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            bail!("Token exchange failed (HTTP {}): {}", status, err_text);
        }

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

        let jwt_candidate = token_resp.id_token.as_deref().unwrap_or(&token_resp.access_token);
        if let Some((name, email, uid)) = parse_jwt_claims(jwt_candidate) {
            token_data.display_name = name;
            token_data.user_email = email;
            token_data.user_id = uid;
        }

        Ok(token_data)
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

fn generate_pkce() -> (String, String) {
    let mut random_bytes = [0u8; 32];
    getrandom::getrandom(&mut random_bytes).expect("Failed to get random bytes");
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    let challenge = URL_SAFE_NO_PAD.encode(hash);
    (verifier, challenge)
}

/// Extract authorization code and error from a parsed URL's query parameters
fn extract_code_from_url(parsed_url: &url::Url) -> (Option<String>, Option<String>) {
    let mut code_opt = None;
    let mut error_opt = None;
    for (k, v) in parsed_url.query_pairs() {
        if k == "code" {
            code_opt = Some(v.to_string());
        } else if k == "error" || k == "error_description" {
            error_opt = Some(v.to_string());
        }
    }
    (code_opt, error_opt)
}

/// Generate success or error HTML response body
fn success_or_error_html(success: bool) -> &'static str {
    if success {
        "<!DOCTYPE html><html><head><meta charset='utf-8'><title>Teams Lite Login</title>\
         <style>body{font-family:Segoe UI,sans-serif;text-align:center;padding:50px;background:#1e1e2e;color:#cdd6f4}\
         .card{background:#313244;border-radius:12px;padding:32px;display:inline-block;box-shadow:0 8px 24px rgba(0,0,0,0.3)}\
         h2{color:#a6e3a1;margin:0 0 12px}p{color:#a6adc8;margin:0}</style></head>\
         <body><div class='card'><h2>✅ Đăng nhập Microsoft Teams Lite thành công!</h2>\
         <p>Bạn có thể đóng tab trình duyệt này và quay lại ứng dụng.</p></div></body></html>"
    } else {
        "<!DOCTYPE html><html><head><meta charset='utf-8'><title>Teams Lite Login</title>\
         <style>body{font-family:Segoe UI,sans-serif;text-align:center;padding:50px;background:#1e1e2e;color:#cdd6f4}\
         .card{background:#313244;border-radius:12px;padding:32px;display:inline-block;box-shadow:0 8px 24px rgba(0,0,0,0.3)}\
         h2{color:#f38ba8;margin:0 0 12px}p{color:#a6adc8;margin:0}</style></head>\
         <body><div class='card'><h2>❌ Đăng nhập không thành công</h2>\
         <p>Vui lòng thử lại trong ứng dụng.</p></div></body></html>"
    }
}

/// HTML relay page for Personal account login
/// After Microsoft login redirects to nativeclient?code=..., the user copies the full URL
/// from the browser address bar and pastes it into this relay page, which extracts the code
/// and sends it back to localhost.
const PERSONAL_RELAY_HTML: &str = r#"<!DOCTYPE html>
<html><head><meta charset='utf-8'><title>Teams Lite - Đăng nhập Cá nhân</title>
<style>
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:'Segoe UI',system-ui,sans-serif;background:#1e1e2e;color:#cdd6f4;min-height:100vh;display:flex;align-items:center;justify-content:center}
.card{background:#313244;border-radius:16px;padding:40px;max-width:560px;width:90%;box-shadow:0 8px 32px rgba(0,0,0,0.4)}
h2{color:#89b4fa;font-size:22px;margin-bottom:8px;text-align:center}
.subtitle{color:#a6adc8;font-size:14px;text-align:center;margin-bottom:24px}
.steps{background:#1e1e2e;border-radius:10px;padding:20px;margin-bottom:20px}
.step{display:flex;gap:12px;margin-bottom:14px;align-items:flex-start}
.step:last-child{margin-bottom:0}
.num{background:#89b4fa;color:#1e1e2e;width:28px;height:28px;border-radius:50%;display:flex;align-items:center;justify-content:center;font-weight:700;font-size:14px;flex-shrink:0}
.step-text{color:#bac2de;font-size:14px;line-height:1.5}
.step-text b{color:#cdd6f4}
.input-group{margin-bottom:16px}
label{color:#a6adc8;font-size:13px;display:block;margin-bottom:6px}
input[type=text]{width:100%;padding:12px 16px;background:#45475a;border:2px solid #585b70;border-radius:8px;color:#cdd6f4;font-size:14px;outline:none;transition:border-color 0.2s}
input[type=text]:focus{border-color:#89b4fa}
input[type=text]::placeholder{color:#6c7086}
.btn{width:100%;padding:14px;background:#89b4fa;color:#1e1e2e;border:none;border-radius:8px;font-size:16px;font-weight:700;cursor:pointer;transition:background 0.2s}
.btn:hover{background:#74c7ec}
.btn:disabled{background:#585b70;cursor:not-allowed}
.error{color:#f38ba8;font-size:13px;margin-top:8px;display:none}
.success{color:#a6e3a1;font-size:15px;text-align:center;padding:20px;display:none}
</style></head>
<body>
<div class="card">
<h2>🔐 Đăng nhập Tài khoản Cá nhân</h2>
<p class="subtitle">Hotmail, Outlook, Gmail — Teams Free</p>
<div class="steps">
<div class="step"><div class="num">1</div><div class="step-text">Đăng nhập tài khoản Microsoft ở <b>tab trình duyệt khác</b> đã mở</div></div>
<div class="step"><div class="num">2</div><div class="step-text">Sau khi đăng nhập xong, trình duyệt sẽ chuyển đến <b>trang trắng</b>. <b>Copy toàn bộ URL</b> từ thanh địa chỉ</div></div>
<div class="step"><div class="num">3</div><div class="step-text"><b>Dán URL</b> vào ô bên dưới rồi nhấn nút xác nhận</div></div>
</div>
<div class="input-group">
<label>Dán URL callback (bắt đầu bằng https://login.microsoftonline.com/...?code=...)</label>
<input type="text" id="url-input" placeholder="Dán URL từ thanh địa chỉ trình duyệt vào đây..." autofocus>
</div>
<div class="error" id="error-msg"></div>
<button class="btn" id="submit-btn" onclick="submitUrl()">✅ Xác nhận đăng nhập</button>
<div class="success" id="success-msg">✅ Đăng nhập thành công! Đang chuyển hướng...</div>
</div>
<script>
function submitUrl(){
  var url=document.getElementById('url-input').value.trim();
  var err=document.getElementById('error-msg');
  var btn=document.getElementById('submit-btn');
  err.style.display='none';
  if(!url){err.textContent='Vui lòng dán URL vào ô trên.';err.style.display='block';return}
  var m=url.match(/[?&]code=([^&]+)/);
  if(!m){err.textContent='URL không hợp lệ. Hãy copy toàn bộ URL từ thanh địa chỉ sau khi trang trắng hiện ra.';err.style.display='block';return}
  btn.disabled=true;btn.textContent='Đang xử lý...';
  window.location.href='/callback?code='+encodeURIComponent(m[1]);
}
document.getElementById('url-input').addEventListener('keypress',function(e){if(e.key==='Enter')submitUrl()});
</script>
</body></html>"#;
