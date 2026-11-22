#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use std::time::Duration;

use crate::auth::AccountType;
use super::models::{ChatMessage, ChatMessageListResponse, Conversation, ConversationListResponse, SendMessageRequest};

pub const DEFAULT_CHATSVC_URL: &str = "https://chatsvcagg.teams.microsoft.com";
pub const DEFAULT_AUTHSVC_URL: &str = "https://teams.microsoft.com/api/authsvc/v1.0/authz";
pub const FALLBACK_AUTHSVC_URL: &str = "https://authsvc.teams.microsoft.com/v1.0/authz";

pub const TEAMS_LIVE_CHATSVC_URL: &str = "https://teams.live.com/api/chatsvc/consumer";
pub const TEAMS_LIVE_AUTHSVC_URL: &str = "https://teams.live.com/api/auth/v2.0/authz/consumer";

pub struct TeamsChatClient {
    base_url: String,
    account_type: AccountType,
    http_client: Client,
    access_token: String,
    skype_token: Option<String>,
}

impl TeamsChatClient {
    pub fn new(access_token: String, account_type: AccountType, custom_base_url: Option<String>) -> Self {
        let default_url = match account_type {
            AccountType::WorkOrSchool => DEFAULT_CHATSVC_URL,
            AccountType::Personal => TEAMS_LIVE_CHATSVC_URL,
        };

        let base_url = custom_base_url
            .unwrap_or_else(|| default_url.to_string())
            .trim_end_matches('/')
            .to_string();

        let mut headers = HeaderMap::new();
        headers.insert("ClientInfo", HeaderValue::from_static("os=windows; osVer=10; app=teams-rust-lite; appVer=1.0.0"));
        headers.insert("Accept", HeaderValue::from_static("application/json, text/plain, */*"));
        headers.insert(
            "User-Agent",
            HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Teams/1.5.00.0 Chrome/100.0.4896.127 Electron/19.1.9 Safari/537.36"),
        );

        let http_client = Client::builder()
            .timeout(Duration::from_secs(25))
            .default_headers(headers)
            .build()
            .unwrap_or_default();

        let skype_token = if access_token.starts_with("eyJ") {
            Some(access_token.clone())
        } else {
            None
        };

        Self {
            base_url,
            account_type,
            http_client,
            access_token,
            skype_token,
        }
    }

    pub fn account_type(&self) -> AccountType {
        self.account_type
    }

    pub fn set_account_type(&mut self, account_type: AccountType) {
        self.account_type = account_type;
        self.base_url = match account_type {
            AccountType::WorkOrSchool => DEFAULT_CHATSVC_URL.to_string(),
            AccountType::Personal => TEAMS_LIVE_CHATSVC_URL.to_string(),
        };
    }

    pub fn set_skype_token(&mut self, token: Option<String>) {
        self.skype_token = token;
    }

    /// Exchanges the Azure AD / Skype Spaces Access Token for a Teams SkypeToken
    pub async fn exchange_skype_token(&mut self) -> Result<String> {
        // If we already have a valid SkypeToken, or if current access_token is already a SkypeToken
        if let Some(ref tok) = self.skype_token {
            log::info!("Already have active SkypeToken (len: {})", tok.len());
            return Ok(tok.clone());
        }

        let candidates: Vec<(&str, &str)> = match self.account_type {
            AccountType::Personal => vec![
                (TEAMS_LIVE_AUTHSVC_URL, "POST"),
                ("https://teams.live.com/api/authsvc/v1.0/authz", "GET"),
                (DEFAULT_AUTHSVC_URL, "POST"),
                (DEFAULT_AUTHSVC_URL, "GET"),
                (FALLBACK_AUTHSVC_URL, "POST"),
            ],
            AccountType::WorkOrSchool => vec![
                (DEFAULT_AUTHSVC_URL, "POST"),
                (DEFAULT_AUTHSVC_URL, "GET"),
                (FALLBACK_AUTHSVC_URL, "POST"),
                (TEAMS_LIVE_AUTHSVC_URL, "POST"),
            ],
        };

        let mut last_err = String::new();
        for (auth_url, method) in candidates {
            log::info!("Attempting authsvc token exchange via {} {} for {:?}", method, auth_url, self.account_type);

            let req_builder = if method == "POST" {
                self.http_client
                    .post(auth_url)
                    .header(CONTENT_TYPE, "application/json")
                    .json(&serde_json::json!({}))
            } else {
                self.http_client.get(auth_url)
            };

            let req_builder = req_builder
                .header(AUTHORIZATION, format!("Bearer {}", self.access_token))
                .header("Accept", "application/json");

            match req_builder.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        match resp.json::<serde_json::Value>().await {
                            Ok(body) => {
                                // Extract skypeToken (supports both Work and Personal response formats)
                                let token_opt = body.get("skypeToken")
                                    .and_then(|s| {
                                        if let Some(str_val) = s.as_str() {
                                            Some(str_val.to_string())
                                        } else if let Some(obj) = s.as_object() {
                                            obj.get("skypetoken").and_then(|st| st.as_str()).map(|st| st.to_string())
                                        } else {
                                            None
                                        }
                                    })
                                    .or_else(|| body.get("tokens").and_then(|t| t.get("skypeToken")).and_then(|s| s.as_str()).map(|s| s.to_string()))
                                    .or_else(|| body.get("tokens").and_then(|t| t.get("token")).and_then(|s| s.as_str()).map(|s| s.to_string()))
                                    .or_else(|| body.get("token").and_then(|s| s.as_str()).map(|s| s.to_string()))
                                    .or_else(|| body.get("tokens").and_then(|t| t.get("skype_token")).and_then(|s| s.as_str()).map(|s| s.to_string()));

                                if let Some(skype_tok) = token_opt {
                                    log::info!("Successfully acquired SkypeToken from {} (len: {})", auth_url, skype_tok.len());
                                    self.skype_token = Some(skype_tok.clone());

                                    // Extract regional chatService URL if provided by authsvc
                                    let chat_svc_url = body.get("regionGtms")
                                        .and_then(|r| r.get("chatService").or_else(|| r.get("chatServiceAfd")))
                                        .and_then(|v| v.as_str())
                                        .or_else(|| body.get("chatServiceUri").and_then(|v| v.as_str()))
                                        .or_else(|| body.get("chatService").and_then(|v| v.as_str()))
                                        .or_else(|| body.get("tokens").and_then(|t| t.get("chatService")).and_then(|v| v.as_str()))
                                        .or_else(|| body.get("tokens").and_then(|t| t.get("chatServiceUri")).and_then(|v| v.as_str()))
                                        .or_else(|| body.get("services").and_then(|s| s.get("chatService")).and_then(|v| v.as_str()))
                                        .or_else(|| body.get("services").and_then(|s| s.get("chatServiceUri")).and_then(|v| v.as_str()));

                                    if let Some(endpoint) = chat_svc_url {
                                        let clean_endpoint = endpoint.trim_end_matches('/').to_string();
                                        log::info!("Updating chatsvc base_url to regional endpoint from authsvc: {}", clean_endpoint);
                                        self.base_url = clean_endpoint;
                                    }

                                    return Ok(skype_tok);
                                } else {
                                    log::warn!("authsvc response from {} did not contain skypeToken: {}", auth_url, body);
                                    last_err = format!("Missing skypeToken from {}: {}", auth_url, body);
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to parse JSON response from {}: {}", auth_url, e);
                                last_err = format!("Failed to parse JSON from {}: {}", auth_url, e);
                            }
                        }
                    } else {
                        let err_text = resp.text().await.unwrap_or_default();
                        log::warn!("authsvc {} returned HTTP {}: {}", auth_url, status, err_text);
                        last_err = format!("HTTP {} from {}: {}", status, auth_url, err_text);
                    }
                }
                Err(e) => {
                    log::warn!("Failed to send request to {}: {}", auth_url, e);
                    last_err = format!("Connection error to {}: {}", auth_url, e);
                }
            }
        }

        bail!("All authsvc endpoints failed to exchange token. Last error: {}", last_err);
    }

    pub fn get_active_token(&self) -> String {
        self.skype_token.clone().unwrap_or_else(|| self.access_token.clone())
    }

    /// Prepares request builder with required Teams authentication headers
    fn apply_auth(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref skype_tok) = self.skype_token {
            req = req
                .header(AUTHORIZATION, format!("skypetoken={}", skype_tok))
                .header("Authentication", format!("skypetoken={}", skype_tok))
                .header("x-skypetoken", skype_tok.as_str());
        } else {
            req = req
                .header(AUTHORIZATION, format!("Bearer {}", self.access_token))
                .header("Authentication", format!("Bearer {}", self.access_token));
        }

        if self.account_type == AccountType::Personal {
            req = req.header("x-csendpoint", "https://msgapi.teams.live.com/");
        }
        req = req.header("BehaviorOverride", "redirectAs404");
        req
    }

    /// Fetches all active conversations for current user
    pub async fn get_conversations(&mut self) -> Result<Vec<Conversation>> {
        let url = format!("{}/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=50", self.base_url);
        log::info!("Fetching conversations from: {}", url);
        
        let req = self.http_client.get(&url);
        let resp = self.apply_auth(req)
            .send()
            .await
            .context("Failed to send get_conversations request")?;

        let status = resp.status();
        log::info!("chatsvc get_conversations response HTTP {}", status);

        // If unauthorized or not found, try fallback to alternative endpoint (enterprise <-> personal)
        if status == reqwest::StatusCode::UNAUTHORIZED 
            || status == reqwest::StatusCode::FORBIDDEN 
            || status == reqwest::StatusCode::NOT_FOUND 
        {
            let fallback_url = if self.base_url.contains("teams.live.com") {
                DEFAULT_CHATSVC_URL
            } else {
                TEAMS_LIVE_CHATSVC_URL
            };

            log::warn!("chatsvc returned HTTP {} on {}. Attempting fallback to alternative endpoint: {}...", status, self.base_url, fallback_url);
            self.base_url = fallback_url.trim_end_matches('/').to_string();

            let retry_url = format!("{}/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=50", self.base_url);
            log::info!("Retrying conversations on alternative endpoint: {}", retry_url);
            let retry_req = self.http_client.get(&retry_url);
            if let Ok(retry_resp) = self.apply_auth(retry_req).send().await {
                if retry_resp.status().is_success() {
                    if let Ok(body) = retry_resp.json::<ConversationListResponse>().await {
                        log::info!("Successfully parsed {} conversations from fallback endpoint", body.conversations.len());
                        return Ok(body.conversations);
                    }
                }
            }
        }

        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            log::error!("chatsvc get_conversations error (HTTP {}): {}", status, err_body);
            bail!("chatsvc HTTP {}: {}", status, err_body);
        }

        let body: ConversationListResponse = resp.json().await.context("Failed to parse conversations JSON")?;
        log::info!("Successfully parsed {} conversations", body.conversations.len());
        Ok(body.conversations)
    }

    /// Fetches current user's primary MRI from chatsvc properties
    pub async fn get_my_mri(&self) -> Result<String> {
        let url = format!("{}/v1/users/ME/properties", self.base_url);
        let req = self.http_client.get(&url);
        let resp = self.apply_auth(req)
            .send()
            .await
            .context("Failed to send get_my_mri request")?;

        if resp.status().is_success() {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            if let Some(primary) = body.get("primaryMemberName").and_then(|v| v.as_str()) {
                let mri = if primary.starts_with("8:") {
                    primary.to_string()
                } else {
                    format!("8:{}", primary)
                };
                return Ok(mri);
            }
        }
        bail!("Failed to determine user MRI from properties");
    }

    /// Creates a new 1:1 conversation or thread with a recipient
    pub async fn create_conversation(&self, recipient: &str) -> Result<Conversation> {
        let clean_recip = recipient.trim();
        if clean_recip.is_empty() {
            bail!("Recipient cannot be empty");
        }

        let my_mri = match self.get_my_mri().await {
            Ok(mri) => mri,
            Err(e) => {
                log::warn!("Could not fetch my_mri ({}), falling back to 8:live:ME", e);
                "8:live:ME".to_string()
            }
        };

        let recip_mri = if clean_recip.starts_with("8:") || clean_recip.starts_with("19:") {
            clean_recip.to_string()
        } else {
            format!("8:{}", clean_recip)
        };

        // For direct 1:1 user MRIs (e.g. 8:live:... or 8:user@domain.com), in Skype MSNP24
        // messages are addressed directly to the recipient MRI without creating a group thread.
        if recip_mri.starts_with("8:") && !recip_mri.contains("@thread") {
            log::info!("Recipient is direct 1:1 user MRI: {}", recip_mri);
            return Ok(Conversation {
                id: recip_mri.clone(),
                topic: Some(clean_recip.to_string()),
                thread_properties: Some(super::models::ThreadProperties {
                    topic: Some(clean_recip.to_string()),
                    thread_type: Some("chat".to_string()),
                }),
                conv_type: Some("chat".to_string()),
                last_message: None,
                members: vec![],
                last_join_timestamp: None,
            });
        }

        let url = format!("{}/v1/threads", self.base_url);
        log::info!("Creating new chat thread via POST {} (initiator: {}, recipient: {})", url, my_mri, recip_mri);

        let payload = serde_json::json!({
            "members": [
                { "id": my_mri, "role": "Admin" },
                { "id": recip_mri, "role": "User" }
            ],
            "properties": {
                "threadType": "chat",
                "topic": clean_recip
            }
        });

        let req = self.http_client.post(&url).json(&payload);
        let resp = self.apply_auth(req)
            .send()
            .await
            .context("Failed to send create thread request")?;

        let status = resp.status();
        if status == reqwest::StatusCode::CREATED || status.is_success() {
            let thread_id = resp.headers()
                .get("Location")
                .and_then(|l| l.to_str().ok())
                .and_then(|loc| loc.split('/').last())
                .unwrap_or("")
                .to_string();

            log::info!("Successfully created thread via POST /threads: {}", thread_id);

            // Set topic on the thread
            if !thread_id.is_empty() {
                let topic_url = format!("{}/v1/threads/{}/properties", self.base_url, thread_id);
                let topic_req = self.http_client.put(&topic_url).json(&serde_json::json!({ "topic": clean_recip }));
                let _ = self.apply_auth(topic_req).send().await;
            }

            return Ok(Conversation {
                id: thread_id,
                topic: Some(clean_recip.to_string()),
                thread_properties: Some(super::models::ThreadProperties {
                    topic: Some(clean_recip.to_string()),
                    thread_type: Some("chat".to_string()),
                }),
                conv_type: Some("chat".to_string()),
                last_message: None,
                members: vec![],
                last_join_timestamp: None,
            });
        }

        let err_body = resp.text().await.unwrap_or_default();
        log::error!("POST /threads failed (HTTP {}): {}", status, err_body);
        bail!("Không thể tạo cuộc trò chuyện (HTTP {}): {}", status, err_body);
    }

    /// Fetches chat message history for a specific conversation
    pub async fn get_messages(&self, conversation_id: &str, page_size: usize) -> Result<Vec<ChatMessage>> {
        let url = format!(
            "{}/v1/users/ME/conversations/{}/messages?view=msnp24Equivalent&pageSize={}",
            self.base_url,
            urlencoding::encode(conversation_id),
            page_size
        );

        let req = self.http_client.get(&url);
        let resp = self.apply_auth(req)
            .send()
            .await
            .context("Failed to send get_messages request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            bail!("chatsvc get_messages failed (HTTP {}): {}", status, err_body);
        }

        let body: ChatMessageListResponse = resp.json().await.context("Failed to parse messages JSON")?;
        Ok(body.messages)
    }

    /// Sends a new message to a conversation
    pub async fn send_message(&self, conversation_id: &str, text: &str) -> Result<ChatMessage> {
        let url = format!(
            "{}/v1/users/ME/conversations/{}/messages",
            self.base_url,
            urlencoding::encode(conversation_id)
        );

        let html_content = format!("<p>{}</p>", html_escape::encode_text(text));
        let client_message_id = chrono::Utc::now().timestamp_millis().to_string();

        let payload = SendMessageRequest {
            content: html_content,
            message_type: "RichText/Html".to_string(),
            content_type: "text".to_string(),
            client_message_id,
        };

        let req = self.http_client
            .post(&url)
            .header(CONTENT_TYPE, "application/json")
            .json(&payload);

        let resp = self.apply_auth(req)
            .send()
            .await
            .context("Failed to send message")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            bail!("chatsvc send_message failed (HTTP {}): {}", status, err_body);
        }

        let sent: ChatMessage = resp.json().await.context("Failed to parse send_message response")?;
        Ok(sent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chatsvc_urls_and_account_types() {
        let client_work = TeamsChatClient::new("token123".into(), AccountType::WorkOrSchool, None);
        assert_eq!(client_work.account_type(), AccountType::WorkOrSchool);
        assert_eq!(client_work.base_url, DEFAULT_CHATSVC_URL);

        let client_personal = TeamsChatClient::new("token123".into(), AccountType::Personal, None);
        assert_eq!(client_personal.account_type(), AccountType::Personal);
        assert_eq!(client_personal.base_url, TEAMS_LIVE_CHATSVC_URL);
    }

    #[test]
    fn test_apply_auth_headers() {
        let mut client = TeamsChatClient::new("bearer_123".into(), AccountType::WorkOrSchool, None);
        let req = reqwest::Client::new().get("https://example.com");
        let req = client.apply_auth(req);
        let built = req.build().unwrap();
        assert_eq!(built.headers().get("Authorization").unwrap(), "Bearer bearer_123");

        client.skype_token = Some("skype_456".into());
        let req = reqwest::Client::new().get("https://example.com");
        let req = client.apply_auth(req);
        let built = req.build().unwrap();
        assert_eq!(built.headers().get("Authorization").unwrap(), "skypetoken=skype_456");
        assert_eq!(built.headers().get("x-skypetoken").unwrap(), "skype_456");

        let client_personal = TeamsChatClient::new("eyJ_skype_personal_789".into(), AccountType::Personal, None);
        let req = reqwest::Client::new().get("https://example.com");
        let req = client_personal.apply_auth(req);
        let built = req.build().unwrap();
        assert_eq!(built.headers().get("Authorization").unwrap(), "skypetoken=eyJ_skype_personal_789");
        assert_eq!(built.headers().get("x-skypetoken").unwrap(), "eyJ_skype_personal_789");
    }

    #[tokio::test]
    async fn test_create_conversation_direct_mri() {
        let client = TeamsChatClient::new("token123".into(), AccountType::Personal, None);
        let conv = client.create_conversation("8:live:.cid.12345").await.unwrap();
        assert_eq!(conv.id, "8:live:.cid.12345");
        assert_eq!(conv.topic.unwrap(), "8:live:.cid.12345");
    }

    #[tokio::test]
    async fn test_live_session_conversations() {
        if let Ok(Some(token_data)) = crate::auth::TokenStore::load() {
            println!("Testing live session for: {:?}", token_data.user_email);
            let mut client = TeamsChatClient::new(token_data.access_token.clone(), token_data.account_type, None);
            let _ = client.exchange_skype_token().await;
            let convs_res = client.get_conversations().await;
            match convs_res {
                Ok(convs) => {
                    println!("LIVE TEST SUCCESS: Loaded {} conversations!", convs.len());
                }
                Err(e) => {
                    println!("Live get_conversations note: {:#}", e);
                }
            }
        } else {
            println!("No stored session found, skipping live session test");
        }
    }
}
