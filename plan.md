# PROJECT PLAN: Rust Lightweight Microsoft Teams Client for Windows

## 1. Project Overview
Build an ultra-lightweight, native Windows desktop client for Microsoft Teams using Rust. The primary goal is to replace the resource-heavy WebView2/Electron official client (~800MB+ RAM) with a single native binary (<60MB RAM, <0.5s startup) that supports real-time messaging, Windows system tray integration, and native Windows Toast notifications.

### Core Constraints & Principles
- **No WebView / No Chromium:** UI must be rendered natively using **Slint** (GPU-accelerated).
- **Direct Protocol Integration:** Bypass Microsoft Graph API polling delays by using Teams Web's internal authentication (`authsvc`), messaging (`chatsvc`), and real-time WebSocket push service (`Trouter`).
- **Clean Architecture:** Strictly decouple the async network/protocol core (`tokio`) from the UI thread (`slint`) using `tokio::sync::mpsc` channels.

---

## 2. Tech Stack & Crates (`Cargo.toml` Baseline)
- **Language:** Rust (2021 edition)
- **Async Runtime:** `tokio` (features: `full`)
- **HTTP & WebSocket:** `reqwest` (features: `json`, `rustls-tls`), `tokio-tungstenite` (features: `rustls-tls-native-roots`), `futures-util`
- **Authentication & Security:** `oauth2`, `tiny_http` (local callback server), `keyring` (Windows Credential Manager storage)
- **Serialization & Parsing:** `serde`, `serde_json`, `html2text` (strip HTML tags from Teams messages), `chrono`, `uuid`
- **GUI & Windows Integration:** `slint`, `tray-icon`, `notify-rust` (or `winrt-notification`), `open` (fallback for video/audio calls in browser)

---

## 3. Directory Structure
```text
teams-rust-lite/
├── Cargo.toml
├── build.rs                 # Slint UI compilation script
├── assets/
│   └── icon.ico             # App & System Tray icon
├── ui/
│   ├── main.slint           # Main window layout (Sidebar + Chat view)
│   ├── login.slint          # Login prompt / status screen
│   └── components/
│       ├── chat_item.slint  # Conversation list item with unread badge
│       └── message.slint    # Message bubble component
└── src/
    ├── main.rs              # Entry point, bridges Tokio runtime & Slint UI event loop
    ├── auth/
    │   ├── mod.rs
    │   ├── oauth.rs         # OAuth2 PKCE flow via system browser + localhost listener
    │   └── token_store.rs   # Windows Credential Manager integration & auto-refresh
    ├── teams/
    │   ├── mod.rs
    │   ├── models.rs        # Serde structs for Conversations, Messages, Trouter events
    │   ├── chatsvc.rs       # REST client for fetching chats, history, and sending messages
    │   └── trouter.rs       # Socket.IO v1/v4 WebSocket client, heartbeat, endpoint registration
    ├── windows_os/
    │   ├── mod.rs
    │   ├── tray.rs          # System tray icon & context menu (Show/Hide, Quit)
    │   └── notify.rs        # Native Windows Toast notifications on NewMessage
    └── bridge.rs            # State management & mpsc message passing between Core and UI