#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use std::time::Duration;

use super::models::{ChatMessage, ChatMessageListResponse, Conversation, ConversationListResponse, SendMessageRequest};

pub const DEFAULT_CHATSVC_URL: &str = "https://chatsvcagg.teams.microsoft.com";
pub const DEFAULT_AUTHSVC_URL: &str = "https://authsvc.teams.microsoft.com/v1.0/authz";

pub struct TeamsChatClient {
    base_url: String,
    http_client: Client,
    access_token: String,
    skype_token: Option<String>,
}

impl TeamsChatClient {
    pub fn new(access_token: String, custom_base_url: Option<String>) -> Self {
        let base_url = custom_base_url
            .unwrap_or_else(|| DEFAULT_CHATSVC_URL.to_string())
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

        Self {
            base_url,
            http_client,
            access_token,
            skype_token: None,
        }
    }

    /// Exchanges the Azure AD / Skype Spaces Access Token for a Teams SkypeToken
    pub async fn exchange_skype_token(&mut self) -> Result<String> {
        let resp = self.http_client
            .post(DEFAULT_AUTHSVC_URL)
            .header(AUTHORIZATION, format!("Bearer {}", self.access_token))
            .header(CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({}))
            .send()
            .await
            .context("Failed to connect to Teams authsvc")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            bail!("authsvc token exchange failed (HTTP {}): {}", status, err_body);
        }

        let body: serde_json::Value = resp.json().await.context("Failed to parse authsvc response")?;
        
        // Extract skypeToken from tokens object or root
        let token_opt = body.get("tokens")
            .and_then(|t| t.get("skypeToken"))
            .and_then(|s| s.as_str())
            .or_else(|| body.get("skypeToken").and_then(|s| s.as_str()))
            .map(|s| s.to_string());

        let skype_token = match token_opt {
            Some(tok) => tok,
            None => bail!("authsvc response did not contain a skypeToken: {}", body),
        };

        log::info!("Successfully acquired SkypeToken from authsvc (len: {})", skype_token.len());
        self.skype_token = Some(skype_token.clone());

        // Extract regional chatService URL if provided by authsvc
        let chat_svc_url = body.get("chatServiceUri")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("chatService").and_then(|v| v.as_str()))
            .or_else(|| body.get("services").and_then(|s| s.get("chatService")).and_then(|v| v.as_str()))
            .or_else(|| body.get("services").and_then(|s| s.get("chatServiceUri")).and_then(|v| v.as_str()));

        if let Some(endpoint) = chat_svc_url {
            let clean_endpoint = endpoint.trim_end_matches('/').to_string();
            log::info!("Updating chatsvc base_url to regional endpoint from authsvc: {}", clean_endpoint);
            self.base_url = clean_endpoint;
        }

        Ok(skype_token)
    }

    pub fn get_active_token(&self) -> String {
        self.skype_token.clone().unwrap_or_else(|| self.access_token.clone())
    }

    /// Prepares request builder with required Teams authentication headers
    fn apply_auth(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req = req.header(AUTHORIZATION, format!("Bearer {}", self.access_token));
        if let Some(ref skype_tok) = self.skype_token {
            req = req
                .header("x-skypetoken", skype_tok.as_str())
                .header("Authentication", format!("skypetoken_asm {}", skype_tok));
        } else {
            req = req.header("Authentication", format!("Bearer {}", self.access_token));
        }
        req
    }

    /// Fetches all active conversations for current user
    pub async fn get_conversations(&self) -> Result<Vec<Conversation>> {
        let url = format!("{}/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=50", self.base_url);
        
        let req = self.http_client.get(&url);
        let resp = self.apply_auth(req)
            .send()
            .await
            .context("Failed to send get_conversations request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            log::error!("chatsvc get_conversations error (HTTP {}): {}", status, err_body);
            bail!("chatsvc HTTP {}: {}", status, err_body);
        }

        let body: ConversationListResponse = resp.json().await.context("Failed to parse conversations JSON")?;
        Ok(body.conversations)
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
