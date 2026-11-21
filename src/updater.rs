use anyhow::{Context, Result};
use reqwest::header::USER_AGENT;
use serde::Deserialize;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_API_URL: &str = "https://api.github.com/repos/thichcode/teams-rust-lite/releases/latest";

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub html_url: String,
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum UpdateStatus {
    UpdateAvailable {
        current_version: String,
        latest_version: String,
        release_url: String,
        download_url: Option<String>,
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
            Ok(Some(release)) => {
                let latest = release.tag_name.trim_start_matches('v');
                if is_newer_version(CURRENT_VERSION, latest) {
                    let download_url = release
                        .assets
                        .iter()
                        .find(|a| a.name.to_lowercase().ends_with(".exe"))
                        .map(|a| a.browser_download_url.clone());

                    UpdateStatus::UpdateAvailable {
                        current_version: CURRENT_VERSION.to_string(),
                        latest_version: release.tag_name,
                        release_url: release.html_url,
                        download_url,
                    }
                } else {
                    UpdateStatus::UpToDate {
                        current_version: CURRENT_VERSION.to_string(),
                    }
                }
            }
            Ok(None) => UpdateStatus::UpToDate {
                current_version: CURRENT_VERSION.to_string(),
            },
            Err(e) => UpdateStatus::CheckFailed(e.to_string()),
        }
    }

    /// Downloads the updated binary, spawns a detached script to overwrite and restart, then exits.
    pub async fn download_and_install_update(download_url: &str) -> Result<()> {
        let current_exe = std::env::current_exe().context("Failed to get current executable path")?;
        let temp_dir = std::env::temp_dir();
        let temp_exe = temp_dir.join("teams-rust-lite-update.exe");

        log::info!("Downloading update from {} to {:?}", download_url, temp_exe);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?;

        let resp = client
            .get(download_url)
            .header(USER_AGENT, "teams-rust-lite-updater")
            .send()
            .await
            .context("Failed to send download request to update URL")?;

        if !resp.status().is_success() {
            anyhow::bail!("Download failed with HTTP status {}", resp.status());
        }

        let bytes = resp.bytes().await.context("Failed to read response body")?;
        tokio::fs::write(&temp_exe, &bytes)
            .await
            .context("Failed to write downloaded executable to temp path")?;

        log::info!("Update downloaded successfully ({} bytes). Spawning restart script...", bytes.len());

        let current_exe_str = current_exe.to_string_lossy().to_string();
        let temp_exe_str = temp_exe.to_string_lossy().to_string();

        let script = format!(
            "Start-Sleep -Milliseconds 600; Move-Item -Path '{temp}' -Destination '{target}' -Force; Start-Process '{target}'",
            temp = temp_exe_str.replace('\'', "''"),
            target = current_exe_str.replace('\'', "''")
        );

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;

            std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .context("Failed to spawn updater PowerShell process")?;
        }

        #[cfg(not(windows))]
        {
            anyhow::bail!("Auto-update is currently only supported on Windows");
        }

        std::process::exit(0);
    }

    async fn fetch_latest_release() -> Result<Option<GitHubRelease>> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        let resp = client
            .get(GITHUB_API_URL)
            .header(USER_AGENT, "teams-rust-lite-updater")
            .send()
            .await
            .context("Failed to send request to GitHub Releases API")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            anyhow::bail!("GitHub API returned HTTP {}", resp.status());
        }

        let release: GitHubRelease = resp.json().await.context("Failed to parse GitHub release")?;
        Ok(Some(release))
    }
}

/// Compares two semver strings: returns true if latest is strictly newer than current
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> (u64, u64, u64, u64) {
        let s = s.trim().trim_start_matches('v');
        let mut parts = s.split('-');
        let version_part = parts.next().unwrap_or("");
        let build_part = parts.next().unwrap_or("");

        let mut nums = version_part.split('.').filter_map(|p| p.parse::<u64>().ok());
        let major = nums.next().unwrap_or(0);
        let minor = nums.next().unwrap_or(0);
        let patch = nums.next().unwrap_or(0);

        let build = if let Some(idx) = build_part.find("build.") {
            build_part[idx + 6..].split(|c: char| !c.is_ascii_digit()).next().unwrap_or("0").parse::<u64>().unwrap_or(0)
        } else {
            build_part.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().unwrap_or(0)
        };

        (major, minor, patch, build)
    };

    let cur = parse(current);
    let lat = parse(latest);
    lat > cur
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        assert!(is_newer_version("0.1.0", "0.2.0"));
        assert!(is_newer_version("0.1.0", "v0.1.1"));
        assert!(is_newer_version("0.1.0", "1.0.0"));
        assert!(is_newer_version("0.1.0", "v0.1.0-build.5"));
        assert!(is_newer_version("0.1.0-build.4", "v0.1.0-build.5"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.1.0-build.5", "v0.1.0-build.5"));
        assert!(!is_newer_version("0.2.0", "0.1.9"));
        assert!(!is_newer_version("0.1.0-build.5", "0.1.0"));
    }
}
