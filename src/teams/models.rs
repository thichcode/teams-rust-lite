#![allow(dead_code)]

use chrono::DateTime;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub display_name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationListResponse {
    #[serde(default)]
    pub conversations: Vec<Conversation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(rename = "type", default)]
    pub conv_type: Option<String>,
    #[serde(rename = "lastMessage", default)]
    pub last_message: Option<ChatMessage>,
    #[serde(default)]
    pub members: Vec<ConversationMember>,
    #[serde(rename = "lastJoinTimestamp", default)]
    pub last_join_timestamp: Option<i64>,
}

pub fn extract_initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.is_empty() {
        "TU".to_string()
    } else if words.len() == 1 {
        words[0].chars().take(2).collect::<String>().to_uppercase()
    } else {
        let first = words[0].chars().next().unwrap_or(' ');
        let last = words[words.len() - 1].chars().next().unwrap_or(' ');
        format!("{}{}", first, last).to_uppercase()
    }
}

impl Conversation {
    pub fn display_title(&self, current_user_id: &str) -> String {
        if let Some(ref t) = self.topic {
            if !t.trim().is_empty() {
                return t.clone();
            }
        }
        
        let other_members: Vec<&ConversationMember> = self
            .members
            .iter()
            .filter(|m| !m.id.contains(current_user_id) && !m.user_principal_name.as_deref().unwrap_or("").contains(current_user_id))
            .collect();

        if !other_members.is_empty() {
            let names: Vec<String> = other_members
                .iter()
                .map(|m| m.display_name.clone().unwrap_or_else(|| "Unknown".to_string()))
                .collect();
            names.join(", ")
        } else {
            self.id.clone()
        }
    }

    pub fn initials(&self, current_user_id: &str) -> String {
        extract_initials(&self.display_title(current_user_id))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMember {
    pub id: String,
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    #[serde(rename = "userPrincipalName", default)]
    pub user_principal_name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageListResponse {
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    #[serde(rename = "sequenceId", default)]
    pub sequence_id: Option<i64>,
    #[serde(rename = "conversationId", default)]
    pub conversation_id: Option<String>,
    #[serde(rename = "conversationLink", default)]
    pub conversation_link: Option<String>,
    #[serde(rename = "from", default)]
    pub from: Option<String>,
    #[serde(rename = "imdisplayname", default)]
    pub im_display_name: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(rename = "messagetype", default)]
    pub message_type: Option<String>,
    #[serde(rename = "composetime", default)]
    pub compose_time: Option<String>,
    #[serde(rename = "originalarrivaltime", default)]
    pub original_arrival_time: Option<String>,
}

impl ChatMessage {
    pub fn clean_text(&self) -> String {
        let raw = match &self.content {
            Some(c) => c,
            None => return String::new(),
        };

        // Convert HTML messages into readable plain text
        let plain = html2text::from_read(raw.as_bytes(), 80).unwrap_or_else(|_| raw.clone());
        plain.trim().to_string()
    }

    pub fn formatted_time(&self) -> String {
        let raw_time = self
            .compose_time
            .as_ref()
            .or(self.original_arrival_time.as_ref());

        if let Some(t) = raw_time {
            if let Ok(dt) = DateTime::parse_from_rfc3339(t) {
                return dt.format("%H:%M").to_string();
            }
        }
        "12:00".to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    #[serde(rename = "messagetype")]
    pub message_type: String,
    #[serde(rename = "contenttype")]
    pub content_type: String,
    #[serde(rename = "clientmessageid")]
    pub client_message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrouterRegistrationRequest {
    pub r#type: String,
    pub transport: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrouterEvent {
    #[serde(rename = "eventType", default)]
    pub event_type: Option<String>,
    #[serde(rename = "resourceLink", default)]
    pub resource_link: Option<String>,
    #[serde(rename = "resource", default)]
    pub resource: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_message_html_strip() {
        let msg = ChatMessage {
            id: "123".into(),
            sequence_id: Some(1),
            conversation_id: Some("conv1".into()),
            conversation_link: None,
            from: Some("user1".into()),
            im_display_name: Some("Alice".into()),
            content: Some("<p>Hello <b>World</b>! &amp; welcome</p>".into()),
            message_type: Some("RichText/Html".into()),
            compose_time: Some("2026-11-21T12:30:00Z".into()),
            original_arrival_time: None,
        };

        let clean = msg.clean_text();
        assert!(clean.contains("Hello World! & welcome"));
        assert_eq!(msg.formatted_time(), "12:30");
    }

    #[test]
    fn test_conversation_display_title_and_initials() {
        let conv = Conversation {
            id: "19:abc@thread.v2".into(),
            topic: Some("Engineering Standup".into()),
            conv_type: Some("chat".into()),
            last_message: None,
            members: vec![],
            last_join_timestamp: None,
        };

        assert_eq!(conv.display_title("user_me"), "Engineering Standup");
        assert_eq!(conv.initials("user_me"), "ES");
    }

    #[test]
    fn test_conversation_fallback_title() {
        let conv = Conversation {
            id: "19:abc@thread.v2".into(),
            topic: None,
            conv_type: Some("chat".into()),
            last_message: None,
            members: vec![
                ConversationMember {
                    id: "user_me".into(),
                    display_name: Some("Me".into()),
                    user_principal_name: None,
                    role: None,
                },
                ConversationMember {
                    id: "user_bob".into(),
                    display_name: Some("Bob Smith".into()),
                    user_principal_name: None,
                    role: None,
                },
            ],
            last_join_timestamp: None,
        };

        assert_eq!(conv.display_title("user_me"), "Bob Smith");
        assert_eq!(conv.initials("user_me"), "BS");
    }

    #[test]
    fn test_extract_initials() {
        assert_eq!(extract_initials("Thưởng Đinh"), "TĐ");
        assert_eq!(extract_initials("John Doe"), "JD");
        assert_eq!(extract_initials("Microsoft"), "MI");
        assert_eq!(extract_initials(""), "TU");
    }
}
