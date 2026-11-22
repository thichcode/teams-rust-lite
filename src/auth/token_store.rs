use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const SERVICE_NAME: &str = "teams-rust-lite";
const ACCOUNT_NAME: &str = "oauth_tokens";

fn get_tokens_file_path() -> std::path::PathBuf {
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        std::path::PathBuf::from(local_appdata).join("teams-rust-lite").join("session.json")
    } else {
        std::env::temp_dir().join("teams-rust-lite-session.json")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountType {
    WorkOrSchool,
    Personal,
}

impl Default for AccountType {
    fn default() -> Self {
        AccountType::WorkOrSchool
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64, // Unix timestamp in seconds
    pub user_id: Option<String>,
    pub user_email: Option<String>,
    pub display_name: Option<String>,
    #[serde(default)]
    pub account_type: AccountType,
}

impl TokenData {
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Buffer of 60 seconds before actual expiration
        now + 60 >= self.expires_at
    }
}

pub struct TokenStore;

impl TokenStore {
    fn get_entry() -> Result<Entry> {
        Self::get_entry_for_account(ACCOUNT_NAME)
    }

    pub fn get_entry_for_account(account: &str) -> Result<Entry> {
        Entry::new(SERVICE_NAME, account)
            .map_err(|e| anyhow::anyhow!("Keyring entry error: {}", e))
    }

    pub fn save(tokens: &TokenData) -> Result<()> {
        let json = serde_json::to_string(tokens)?;

        // Try storing in OS keyring first
        if let Ok(entry) = Self::get_entry() {
            let _ = entry.set_password(&json);
        }

        // Store reliably in AppData session file as well (avoids Windows Credential Manager blob size limits)
        let file_path = get_tokens_file_path();
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&file_path, &json)
            .with_context(|| format!("Failed to write session file: {:?}", file_path))?;

        log::info!("Session tokens successfully saved.");
        Ok(())
    }

    pub fn load() -> Result<Option<TokenData>> {
        // Try OS keyring first
        if let Ok(entry) = Self::get_entry() {
            if let Ok(json) = entry.get_password() {
                if let Ok(data) = serde_json::from_str::<TokenData>(&json) {
                    return Ok(Some(data));
                }
            }
        }

        // Fallback to AppData session file
        let file_path = get_tokens_file_path();
        if file_path.exists() {
            if let Ok(json) = std::fs::read_to_string(&file_path) {
                if let Ok(data) = serde_json::from_str::<TokenData>(&json) {
                    log::info!("Loaded stored session tokens from AppData session.json");
                    return Ok(Some(data));
                }
            }
        }

        Ok(None)
    }

    pub fn clear() -> Result<()> {
        if let Ok(entry) = Self::get_entry() {
            let _ = entry.delete_credential();
        }

        let file_path = get_tokens_file_path();
        if file_path.exists() {
            let _ = std::fs::remove_file(file_path);
        }

        log::info!("Session tokens cleared.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyring_save_load() {
        let tok = TokenData {
            access_token: "test_access_123".into(),
            refresh_token: Some("test_refresh_123".into()),
            expires_at: 9999999999,
            user_id: Some("uid".into()),
            user_email: Some("a@b.com".into()),
            display_name: Some("Test User".into()),
            account_type: AccountType::WorkOrSchool,
        };

        let entry = match TokenStore::get_entry_for_account("oauth_tokens_test") {
            Ok(e) => e,
            Err(e) => {
                println!("Skipping keyring test: {:?}", e);
                return;
            }
        };

        let json = serde_json::to_string(&tok).unwrap();
        if let Err(e) = entry.set_password(&json) {
            println!("Skipping keyring test on headless CI environment: {:?}", e);
            return;
        }

        let loaded_json = entry.get_password().unwrap();
        let loaded: TokenData = serde_json::from_str(&loaded_json).unwrap();
        assert_eq!(loaded.access_token, "test_access_123");

        let _ = entry.delete_credential();
    }

    #[test]
    fn test_token_expiration() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let expired = TokenData {
            access_token: "tok1".into(),
            refresh_token: None,
            expires_at: now - 100,
            user_id: None,
            user_email: None,
            display_name: None,
            account_type: AccountType::WorkOrSchool,
        };
        assert!(expired.is_expired());

        let valid = TokenData {
            access_token: "tok2".into(),
            refresh_token: None,
            expires_at: now + 3600,
            user_id: None,
            user_email: None,
            display_name: None,
            account_type: AccountType::WorkOrSchool,
        };
        assert!(!valid.is_expired());
    }
}
