# Auto-Update & Enhanced UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide a clearer update button in both Home and Chat views, and implement a one-click auto-updater that downloads the new release executable, terminates the running client, overwrites the current binary, and restarts seamlessly.

**Architecture:** 
1. `src/updater.rs`: Parse asset download URL (`browser_download_url`) from GitHub releases API; implement `download_and_install_update` which saves to a temp location and launches a detached PowerShell process to swap files and restart after client exit.
2. `src/bridge.rs`: Add `InstallUpdate` command, track `latest_download_url`, handle download errors, and bridge UI callbacks.
3. `ui/login.slint` & `ui/main.slint`: Redesign update button into a clear pill button with labels; enhance update banner with "Cập nhật ngay" (one-click auto-update) and "Chi tiết" buttons.

**Tech Stack:** Slint 1.9, Tokio, Reqwest 0.12, Windows Process API.

---

### Task 1: Extend `src/updater.rs` for Auto-Download & In-Place Replacement
- [ ] Add `download_url` to `UpdateStatus::UpdateAvailable`
- [ ] In `fetch_latest_release`, parse the asset `browser_download_url` for `teams-rust-lite.exe`
- [ ] Implement `download_and_install_update(download_url: &str) -> anyhow::Result<()>`
- [ ] Write unit tests for release parsing and asset URL resolution

### Task 2: Update Bridge & Core Event Handling
- [ ] Add `UiCommand::InstallUpdate` to `src/bridge.rs`
- [ ] Store `latest_download_url` in `Bridge`
- [ ] Handle `InstallUpdate` by calling `download_and_install_update` and updating status during download
- [ ] Hook `window.on_install_update` in `attach_ui_bridge`

### Task 3: Redesign Slint UI Elements for Update
- [ ] In `ui/login.slint`: Add clearer button `[ ⇪ Cập nhật ]` and "Cập nhật ngay" button on banner
- [ ] In `ui/main.slint`: Wire `install-update` callback and add "Cập nhật ngay" button in banner
- [ ] Add visual feedback for downloading state ("Đang tải bản cập nhật...")

### Task 4: Verification, Test & Deployment
- [ ] Run `cargo test`
- [ ] Run `cargo build`
- [ ] Commit and push to GitHub `main` branch
