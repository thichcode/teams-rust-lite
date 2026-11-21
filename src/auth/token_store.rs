use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const SERVICE_NAME: &str = "teams-rust-lite";
const ACCOUNT_NAME: &str = "oauth_tokens";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64, // Unix timestamp in seconds
    pub user_id: Option<String>,
    pub user_email: Option<String>,
    pub display_name: Option<String>,
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
        let entry = Self::get_entry()?;
        let json = serde_json::to_string(tokens)?;
        entry
            .set_password(&json)
            .map_err(|e| anyhow::anyhow!("Failed to save token to keyring: {}", e))?;
        Ok(())
    }

    pub fn load() -> Result<Option<TokenData>> {
        let entry = Self::get_entry()?;
        match entry.get_password() {
            Ok(json) => {
                println!("Keyring get_password SUCCESS, length: {}", json.len());
                let data: TokenData = serde_json::from_str(&json)
                    .context("Failed to deserialize tokens from keyring")?;
                Ok(Some(data))
            }
            Err(e) => {
                println!("Keyring get_password returned: {:?}", e);
                Ok(None)
            }
        }
    }

    pub fn clear() -> Result<()> {
        let entry = Self::get_entry()?;
        match entry.delete_credential() {
            Ok(_) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("Failed to delete token from keyring: {}", e)),
        }
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
        };
        assert!(expired.is_expired());

        let valid = TokenData {
            access_token: "tok2".into(),
            refresh_token: None,
            expires_at: now + 3600,
            user_id: None,
            user_email: None,
            display_name: None,
        };
        assert!(!valid.is_expired());
    }
}
