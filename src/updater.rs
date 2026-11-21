use anyhow::{Context, Result};
use reqwest::header::USER_AGENT;
use serde::Deserialize;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_API_URL: &str = "https://api.github.com/repos/thichcode/teams-rust-lite/releases/latest";

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub html_url: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum UpdateStatus {
    UpdateAvailable {
        current_version: String,
        latest_version: String,
        release_url: String,
    },
    UpToDate {
        current_version: String,
    },
    CheckFailed(String),
}

pub struct UpdateChecker;

impl UpdateChecker {
    /// Checks GitHub repository for newer releases
    pub async fn check() -> UpdateStatus {
        match Self::fetch_latest_release().await {
            Ok(release) => {
                let latest = release.tag_name.trim_start_matches('v');
                if is_newer_version(CURRENT_VERSION, latest) {
                    UpdateStatus::UpdateAvailable {
                        current_version: CURRENT_VERSION.to_string(),
                        latest_version: release.tag_name,
                        release_url: release.html_url,
                    }
                } else {
                    UpdateStatus::UpToDate {
                        current_version: CURRENT_VERSION.to_string(),
                    }
                }
            }
            Err(e) => UpdateStatus::CheckFailed(e.to_string()),
        }
    }

    async fn fetch_latest_release() -> Result<GitHubRelease> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        let resp = client
            .get(GITHUB_API_URL)
            .header(USER_AGENT, "teams-rust-lite-updater")
            .send()
            .await
            .context("Failed to send request to GitHub Releases API")?;

        if !resp.status().is_success() {
            anyhow::bail!("GitHub API returned HTTP {}", resp.status());
        }

        let release: GitHubRelease = resp.json().await.context("Failed to parse GitHub release")?;
        Ok(release)
    }
}

/// Compares two semver strings: returns true if latest is strictly newer than current
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .filter_map(|part| part.split('-').next().unwrap_or("0").parse::<u64>().ok())
            .collect()
    };

    let cur_parts = parse(current);
    let lat_parts = parse(latest);

    let max_len = cur_parts.len().max(lat_parts.len());
    for i in 0..max_len {
        let c = cur_parts.get(i).copied().unwrap_or(0);
        let l = lat_parts.get(i).copied().unwrap_or(0);
        if l > c {
            return true;
        } else if l < c {
            return false;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        assert!(is_newer_version("0.1.0", "0.2.0"));
        assert!(is_newer_version("0.1.0", "v0.1.1"));
        assert!(is_newer_version("0.1.0", "1.0.0"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.2.0", "0.1.9"));
    }
}
