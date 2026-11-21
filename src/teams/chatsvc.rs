#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use std::time::Duration;

use super::models::{ChatMessage, ChatMessageListResponse, Conversation, ConversationListResponse, SendMessageRequest};

pub const DEFAULT_CHATSVC_URL: &str = "https://chatsvcagg.teams.microsoft.com";

pub struct TeamsChatClient {
    base_url: String,
    http_client: Client,
    token: String,
}

impl TeamsChatClient {
    pub fn new(token: String, custom_base_url: Option<String>) -> Self {
        let base_url = custom_base_url
            .unwrap_or_else(|| DEFAULT_CHATSVC_URL.to_string())
            .trim_end_matches('/')
            .to_string();

        let mut headers = HeaderMap::new();
        headers.insert("BehaviorOverride", HeaderValue::from_static("redirectAs404"));
        headers.insert("ClientInfo", HeaderValue::from_static("os=windows; osVer=10; app=teams-rust-lite; appVer=1.0.0"));

        let http_client = Client::builder()
            .timeout(Duration::from_secs(20))
            .default_headers(headers)
            .build()
            .unwrap_or_default();

        Self {
            base_url,
            http_client,
            token,
        }
    }

    pub fn set_token(&mut self, token: String) {
        self.token = token;
    }

    fn auth_header(&self) -> String {
        if self.token.starts_with("skypetoken_asm ") || self.token.starts_with("Bearer ") {
            self.token.clone()
        } else if self.token.len() > 100 && !self.token.contains(' ') {
            // If it's a raw skypetoken or bearer token
            format!("Bearer {}", self.token)
        } else {
            self.token.clone()
        }
    }

    /// Fetches all active conversations for current user
    pub async fn get_conversations(&self) -> Result<Vec<Conversation>> {
        let url = format!("{}/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=50", self.base_url);
        
        let resp = self.http_client
            .get(&url)
            .header(AUTHORIZATION, self.auth_header())
            .header("Authentication", self.auth_header())
            .send()
            .await
            .context("Failed to send get_conversations request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            bail!("chatsvc get_conversations failed (HTTP {}): {}", status, err_body);
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

        let resp = self.http_client
            .get(&url)
            .header(AUTHORIZATION, self.auth_header())
            .header("Authentication", self.auth_header())
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

        // Escape HTML entities for safe inclusion into RichText/Html format
        let html_content = format!("<p>{}</p>", html_escape::encode_text(text));
        let client_message_id = chrono::Utc::now().timestamp_millis().to_string();

        let payload = SendMessageRequest {
            content: html_content,
            message_type: "RichText/Html".to_string(),
            content_type: "text".to_string(),
            client_message_id,
        };

        let resp = self.http_client
            .post(&url)
            .header(AUTHORIZATION, self.auth_header())
            .header("Authentication", self.auth_header())
            .header(CONTENT_TYPE, "application/json")
            .json(&payload)
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

    /// Register endpoint with chatsvc to obtain push notification / Trouter subscription URL
    pub async fn register_endpoint(&self) -> Result<serde_json::Value> {
        let url = format!("{}/v1/users/ME/endpoints", self.base_url);
        let endpoint_id = format!("{{{}}}", uuid::Uuid::new_v4());
        
        let payload = serde_json::json!({
            "id": endpoint_id,
            "type": "EndpointRegistration",
            "clientInfo": {
                "clientPlatform": "skype.com",
                "platformUIVersion": "teams-rust-lite/1.0"
            },
            "subscriptions": [
                {
                    "channelType": "HttpLongPoll",
                    "interestedResources": [
                        "/v1/users/ME/conversations/ALL/messages",
                        "/v1/users/ME/conversations/ALL/properties"
                    ]
                }
            ]
        });

        let resp = self.http_client
            .post(&url)
            .header(AUTHORIZATION, self.auth_header())
            .header("Authentication", self.auth_header())
            .header(CONTENT_TYPE, "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to register endpoint")?;

        let res_json: serde_json::Value = resp.json().await?;
        Ok(res_json)
    }
}
