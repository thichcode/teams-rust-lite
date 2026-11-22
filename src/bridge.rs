use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::auth::{AccountType, OAuthManager, TokenData, TokenStore};
use crate::teams::chatsvc::TeamsChatClient;
use crate::teams::models::{ChatMessage, Conversation};
use crate::teams::trouter::{TrouterClient, TrouterNotification};
use crate::windows_os::WindowsNotifier;
use crate::{ConversationData, MainWindow, MessageData};

#[derive(Debug, Clone)]
pub enum UiCommand {
    StartLogin(AccountType),
    OpenBrowserLogin,
    Logout,
    RefreshConversations,
    SelectConversation(String),
    SendMessage { conversation_id: String, text: String },
    CheckForUpdates,
    OpenUpdateUrl,
    InstallUpdate,
    OpenLogs,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum CoreEvent {
    DeviceCodeReceived { user_code: String, verification_url: String },
    AuthSuccess { user_name: String, email: String },
    AuthFailed(String),
    ConversationsLoaded(Vec<Conversation>),
    MessagesLoaded { conversation_id: String, messages: Vec<ChatMessage> },
    NewMessageReceived(ChatMessage),
    ConnectionStatusChanged { status: String, is_connected: bool },
    ErrorOccurred(String),
    UpdateResult(crate::updater::UpdateStatus),
    UpdateProgress(String),
}

pub struct CoreWorker {
    rx_cmd: mpsc::Receiver<UiCommand>,
    tx_event: mpsc::Sender<CoreEvent>,
    token: Option<TokenData>,
    chat_client: Option<Arc<tokio::sync::Mutex<TeamsChatClient>>>,
    current_conversation_id: Option<String>,
    latest_update_url: Option<String>,
    latest_download_url: Option<String>,
    latest_verification_url: Option<String>,
}

impl CoreWorker {
    pub fn new(rx_cmd: mpsc::Receiver<UiCommand>, tx_event: mpsc::Sender<CoreEvent>) -> Self {
        Self {
            rx_cmd,
            tx_event,
            token: None,
            chat_client: None,
            current_conversation_id: None,
            latest_update_url: None,
            latest_download_url: None,
            latest_verification_url: None,
        }
    }

    pub async fn run(mut self) {
        log::info!("CoreWorker started in Tokio runtime.");

        // Check if existing token exists in Windows Credential Manager
        if let Ok(Some(saved_token)) = TokenStore::load() {
            log::info!("Found stored credentials in Windows Keyring");
            if !saved_token.is_expired() {
                self.setup_session(saved_token).await;
            } else if let Some(ref refresh_tok) = saved_token.refresh_token {
                log::info!("Stored token expired, attempting refresh for {:?}...", saved_token.account_type);
                let oauth = OAuthManager::default();
                if let Ok(new_token) = oauth.refresh_token(refresh_tok, saved_token.account_type).await {
                    let _ = TokenStore::save(&new_token);
                    self.setup_session(new_token).await;
                } else {
                    let _ = self.tx_event.send(CoreEvent::AuthFailed("Saved session expired. Please sign in again.".into())).await;
                }
            }
        }

        while let Some(cmd) = self.rx_cmd.recv().await {
            match cmd {
                UiCommand::StartLogin(account_type) => {
                    self.handle_login(account_type).await;
                }
                UiCommand::OpenBrowserLogin => {
                    let url = self.latest_verification_url.clone().unwrap_or_else(|| "https://login.microsoft.com/device".to_string());
                    let _ = open::that(&url);
                }
                UiCommand::Logout => {
                    let _ = TokenStore::clear();
                    self.token = None;
                    self.chat_client = None;
                    let _ = self.tx_event.send(CoreEvent::ConnectionStatusChanged {
                        status: "Logged out".into(),
                        is_connected: false,
                    }).await;
                }
                UiCommand::RefreshConversations => {
                    self.refresh_conversations().await;
                }
                UiCommand::SelectConversation(conv_id) => {
                    self.current_conversation_id = Some(conv_id.clone());
                    self.load_messages_for_conversation(&conv_id).await;
                }
                UiCommand::SendMessage { conversation_id, text } => {
                    self.send_message(&conversation_id, &text).await;
                }
                UiCommand::CheckForUpdates => {
                    let status = crate::updater::UpdateChecker::check().await;
                    if let crate::updater::UpdateStatus::UpdateAvailable { ref release_url, ref download_url, .. } = status {
                        self.latest_update_url = Some(release_url.clone());
                        self.latest_download_url = download_url.clone();
                    }
                    let _ = self.tx_event.send(CoreEvent::UpdateResult(status)).await;
                }
                UiCommand::OpenUpdateUrl => {
                    let url = self.latest_update_url.clone()
                        .unwrap_or_else(|| "https://github.com/thichcode/teams-rust-lite/releases".to_string());
                    let _ = open::that(&url);
                }
                UiCommand::InstallUpdate => {
                    if let Some(ref download_url) = self.latest_download_url {
                        let _ = self.tx_event.send(CoreEvent::UpdateProgress("Đang tải bản cập nhật...".into())).await;
                        if let Err(e) = crate::updater::UpdateChecker::download_and_install_update(download_url).await {
                            log::error!("Failed to install update: {}", e);
                            let _ = self.tx_event.send(CoreEvent::ErrorOccurred(format!("Cập nhật thất bại: {}", e))).await;
                        }
                    } else if let Some(ref release_url) = self.latest_update_url {
                        let _ = open::that(release_url);
                    }
                }
                UiCommand::OpenLogs => {
                    log::info!("Opening debug log file on request...");
                    if let Err(e) = crate::logger::open_log_file() {
                        log::error!("Failed to open log file: {:#}", e);
                    }
                }
            }
        }
    }

    async fn handle_login(&mut self, account_type: AccountType) {
        let type_desc = match account_type {
            AccountType::WorkOrSchool => "Work/School",
            AccountType::Personal => "Personal/Teams Free",
        };
        log::info!("Starting login flow for account type: {:?}", account_type);

        let _ = self.tx_event.send(CoreEvent::ConnectionStatusChanged {
            status: format!("Requesting {} sign-in code from Microsoft...", type_desc),
            is_connected: false,
        }).await;

        let oauth = OAuthManager::default();
        let device_code_resp = match oauth.request_device_code(account_type).await {
            Ok(resp) => resp,
            Err(e) => {
                log::error!("Failed to request device code: {:#}", e);
                let _ = self.tx_event.send(CoreEvent::AuthFailed(format!("Sign-in initialization failed: {:#}", e))).await;
                return;
            }
        };

        // Send code to UI
        let _ = self.tx_event.send(CoreEvent::DeviceCodeReceived {
            user_code: device_code_resp.user_code.clone(),
            verification_url: device_code_resp.verification_uri.clone(),
        }).await;

        // Try to copy code to Windows clipboard
        let user_code = device_code_resp.user_code.clone();
        tokio::task::spawn_blocking(move || {
            let _ = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &format!("Set-Clipboard -Value '{}'", user_code)])
                .output();
        });

        // Open browser
        self.latest_verification_url = Some(device_code_resp.verification_uri.clone());
        log::info!("Opening browser at: {}", device_code_resp.verification_uri);
        let _ = open::that(&device_code_resp.verification_uri);

        // Poll for authorization
        let poll_interval = device_code_resp.interval.unwrap_or(5);
        match oauth.poll_device_code(&device_code_resp.device_code, poll_interval, device_code_resp.expires_in, account_type).await {
            Ok(tokens) => {
                let _ = TokenStore::save(&tokens);
                self.setup_session(tokens).await;
            }
            Err(e) => {
                log::error!("Sign-in polling error: {:#}", e);
                let _ = self.tx_event.send(CoreEvent::AuthFailed(format!("Sign-in failed: {:#}", e))).await;
            }
        }
    }

    async fn setup_session(&mut self, mut tokens: TokenData) {
        let user_name = tokens.display_name.clone().unwrap_or_else(|| "Teams User".into());
        let email = tokens.user_email.clone().unwrap_or_default();
        let access_tok = tokens.access_token.clone();

        // Auto-detect Personal consumer accounts (e.g. @hotmail.com, @outlook.com, @live.com, etc.)
        let is_consumer_email = email.to_lowercase().ends_with("@hotmail.com")
            || email.to_lowercase().ends_with("@outlook.com")
            || email.to_lowercase().ends_with("@live.com")
            || email.to_lowercase().ends_with("@msn.com")
            || email.to_lowercase().ends_with("@gmail.com")
            || email.to_lowercase().ends_with("@yahoo.com");

        if is_consumer_email && tokens.account_type != AccountType::Personal {
            log::info!("Detected consumer email ({}) - automatically switching account type to Personal", email);
            tokens.account_type = AccountType::Personal;
            let _ = TokenStore::save(&tokens);
        }

        let account_type = tokens.account_type;

        log::info!("Setting up session for '{}' ({}) with account type {:?}", user_name, email, account_type);

        self.token = Some(tokens.clone());
        let mut chat_client = TeamsChatClient::new(access_tok.clone(), account_type, None);

        // Exchange for SkypeToken via authsvc (with automatic fallback and detection)
        let push_token = match chat_client.exchange_skype_token().await {
            Ok(skype_tok) => {
                log::info!("Successfully acquired Teams SkypeToken from authsvc!");
                // If chat client switched account type, persist it
                if chat_client.account_type() != tokens.account_type {
                    log::info!("Syncing dynamic account type {:?} to TokenStore", chat_client.account_type());
                    tokens.account_type = chat_client.account_type();
                    let _ = TokenStore::save(&tokens);
                    self.token = Some(tokens.clone());
                }
                skype_tok
            }
            Err(e) => {
                log::warn!("authsvc token exchange failed: {}. Falling back to access_token...", e);
                access_tok.clone()
            }
        };

        let client = Arc::new(tokio::sync::Mutex::new(chat_client));
        self.chat_client = Some(client.clone());

        let _ = self.tx_event.send(CoreEvent::AuthSuccess {
            user_name,
            email,
        }).await;

        let _ = self.tx_event.send(CoreEvent::ConnectionStatusChanged {
            status: "Connected (chatsvc)".into(),
            is_connected: true,
        }).await;

        // Fetch conversations
        self.refresh_conversations().await;

        // Spawn Trouter real-time push service using push_token (SkypeToken)
        let (tx_trouter, mut rx_trouter) = mpsc::channel(100);
        let trouter = TrouterClient::new(push_token, None, tx_trouter);
        tokio::spawn(async move {
            trouter.run_loop().await;
        });

        // Forward trouter events
        let tx_core = self.tx_event.clone();
        tokio::spawn(async move {
            while let Some(notif) = rx_trouter.recv().await {
                match notif {
                    TrouterNotification::Connected => {
                        let _ = tx_core.send(CoreEvent::ConnectionStatusChanged {
                            status: "Live (Trouter Push Active)".into(),
                            is_connected: true,
                        }).await;
                    }
                    TrouterNotification::Disconnected => {
                        let _ = tx_core.send(CoreEvent::ConnectionStatusChanged {
                            status: "Reconnecting push gateway...".into(),
                            is_connected: false,
                        }).await;
                    }
                    TrouterNotification::NewMessage(msg) => {
                        let clean = msg.clean_text();
                        let sender = msg.im_display_name.as_deref().unwrap_or("Someone");
                        WindowsNotifier::notify_chat_message(sender, "", &clean);
                        let _ = tx_core.send(CoreEvent::NewMessageReceived(msg)).await;
                    }
                    TrouterNotification::RawEvent(_) => {}
                }
            }
        });
    }

    async fn refresh_conversations(&self) {
        if let Some(ref client_lock) = self.chat_client {
            let mut client = client_lock.lock().await;
            match client.get_conversations().await {
                Ok(conversations) => {
                    let count = conversations.len();
                    log::info!("Successfully loaded {} conversations", count);
                    let _ = self.tx_event.send(CoreEvent::ConversationsLoaded(conversations)).await;
                    let _ = self.tx_event.send(CoreEvent::ConnectionStatusChanged {
                        status: format!("Connected ({} chats)", count),
                        is_connected: true,
                    }).await;
                }
                Err(e) => {
                    log::warn!("Failed to fetch conversations: {}", e);
                    let _ = self.tx_event.send(CoreEvent::ErrorOccurred(format!("{}", e))).await;
                }
            }
        }
    }

    async fn load_messages_for_conversation(&self, conv_id: &str) {
        if let Some(ref client_lock) = self.chat_client {
            let client = client_lock.lock().await;
            match client.get_messages(conv_id, 40).await {
                Ok(messages) => {
                    let _ = self.tx_event.send(CoreEvent::MessagesLoaded {
                        conversation_id: conv_id.to_string(),
                        messages,
                    }).await;
                }
                Err(e) => {
                    log::warn!("Failed to fetch messages for {}: {}", conv_id, e);
                }
            }
        }
    }

    async fn send_message(&self, conv_id: &str, text: &str) {
        if let Some(ref client_lock) = self.chat_client {
            let client = client_lock.lock().await;
            match client.send_message(conv_id, text).await {
                Ok(sent_msg) => {
                    let _ = self.tx_event.send(CoreEvent::NewMessageReceived(sent_msg)).await;
                }
                Err(e) => {
                    log::error!("Failed to send message: {}", e);
                    let _ = self.tx_event.send(CoreEvent::ErrorOccurred(format!("Failed to send: {}", e))).await;
                }
            }
        }
    }
}

pub fn attach_ui_bridge(
    window: &MainWindow,
    tx_cmd: mpsc::Sender<UiCommand>,
    mut rx_event: mpsc::Receiver<CoreEvent>,
) {
    let weak_window = window.as_weak();

    // Hook up UI callbacks to UiCommand channel
    let tx_login_work = tx_cmd.clone();
    let weak_login_work = weak_window.clone();
    window.on_start_login_work(move || {
        if let Some(win) = weak_login_work.upgrade() {
            win.set_is_authenticating(true);
            win.set_login_status("Opening browser for Work/School Sign-in...".into());
        }
        let tx = tx_login_work.clone();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::StartLogin(AccountType::WorkOrSchool)).await;
        });
    });

    let tx_login_personal = tx_cmd.clone();
    let weak_login_personal = weak_window.clone();
    window.on_start_login_personal(move || {
        if let Some(win) = weak_login_personal.upgrade() {
            win.set_is_authenticating(true);
            win.set_login_status("Opening browser for Personal/Teams Free Sign-in...".into());
        }
        let tx = tx_login_personal.clone();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::StartLogin(AccountType::Personal)).await;
        });
    });

    let tx_open_browser = tx_cmd.clone();
    window.on_open_browser_login(move || {
        let tx = tx_open_browser.clone();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::OpenBrowserLogin).await;
        });
    });

    let tx_logout = tx_cmd.clone();
    window.on_logout(move || {
        let tx = tx_logout.clone();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::Logout).await;
        });
    });

    let tx_refresh = tx_cmd.clone();
    window.on_trigger_refresh(move || {
        let tx = tx_refresh.clone();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::RefreshConversations).await;
        });
    });

    let tx_select = tx_cmd.clone();
    let weak_select = weak_window.clone();
    window.on_select_conversation(move |conv_id| {
        if let Some(win) = weak_select.upgrade() {
            win.set_active_conversation_id(conv_id.clone());
        }
        let tx = tx_select.clone();
        let id_str = conv_id.to_string();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::SelectConversation(id_str)).await;
        });
    });

    let tx_send = tx_cmd.clone();
    let weak_send = weak_window.clone();
    window.on_send_message(move |text| {
        if let Some(win) = weak_send.upgrade() {
            let conv_id = win.get_active_conversation_id().to_string();
            let text_str = text.to_string();
            if !conv_id.is_empty() && !text_str.is_empty() {
                let tx = tx_send.clone();
                tokio::spawn(async move {
                    let _ = tx.send(UiCommand::SendMessage {
                        conversation_id: conv_id,
                        text: text_str,
                    }).await;
                });
            }
        }
    });

    let tx_check_update = tx_cmd.clone();
    let weak_win_check = weak_window.clone();
    window.on_check_for_updates(move || {
        if let Some(win) = weak_win_check.upgrade() {
            win.set_login_status("Checking for updates...".into());
            win.set_connection_status("Checking for updates...".into());
        }
        let tx = tx_check_update.clone();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::CheckForUpdates).await;
        });
    });

    let tx_open_update = tx_cmd.clone();
    window.on_open_update_url(move || {
        let tx = tx_open_update.clone();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::OpenUpdateUrl).await;
        });
    });

    let tx_install_update = tx_cmd.clone();
    window.on_install_update(move || {
        let tx = tx_install_update.clone();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::InstallUpdate).await;
        });
    });

    let tx_open_logs = tx_cmd.clone();
    window.on_open_logs(move || {
        let tx = tx_open_logs.clone();
        tokio::spawn(async move {
            let _ = tx.send(UiCommand::OpenLogs).await;
        });
    });

    // Listen to CoreEvents and update Slint UI via invoke_from_event_loop
    let weak_events = weak_window.clone();
    tokio::spawn(async move {
        while let Some(evt) = rx_event.recv().await {
            let weak_target = weak_events.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = weak_target.upgrade() {
                    handle_core_event(&win, evt);
                }
            });
        }
    });
}

fn handle_core_event(win: &MainWindow, evt: CoreEvent) {
    match evt {
        CoreEvent::DeviceCodeReceived { user_code, verification_url } => {
            win.set_login_user_code(user_code.into());
            win.set_login_verification_url(verification_url.into());
            win.set_login_status("Code copied to clipboard! Paste it into the browser window to sign in.".into());
            win.set_is_authenticating(true);
        }
        CoreEvent::AuthSuccess { user_name, .. } => {
            win.set_login_user_code("".into());
            win.set_is_logged_in(true);
            win.set_is_authenticating(false);
            win.set_user_name(user_name.into());
            win.set_login_status("Signed in successfully".into());
        }
        CoreEvent::AuthFailed(err) => {
            win.set_is_authenticating(false);
            win.set_is_logged_in(false);
            win.set_login_status(err.into());
        }
        CoreEvent::ConnectionStatusChanged { status, is_connected } => {
            win.set_connection_status(status.into());
            win.set_is_connected(is_connected);
        }
        CoreEvent::ConversationsLoaded(convs) => {
            let user_id = win.get_user_name().to_string();
            let slint_convs: Vec<ConversationData> = convs
                .into_iter()
                .map(|c| {
                    let title = c.display_title(&user_id);
                    let initials = c.initials(&user_id);
                    let last_msg = c.last_message.as_ref().map(|m| m.clean_text()).unwrap_or_default();
                    let time = c.last_message.as_ref().map(|m| m.formatted_time()).unwrap_or_default();

                    ConversationData {
                        id: c.id.into(),
                        title: title.into(),
                        last_message: last_msg.into(),
                        timestamp: time.into(),
                        unread_count: 0,
                        avatar_text: initials.into(),
                    }
                })
                .collect();

            let model = ModelRc::new(VecModel::from(slint_convs));
            win.set_conversations(model);
        }
        CoreEvent::MessagesLoaded { messages, .. } => {
            let user_name = win.get_user_name().to_string();
            let slint_msgs: Vec<MessageData> = messages
                .into_iter()
                .map(|m| {
                    let sender = m.im_display_name.clone().unwrap_or_else(|| "User".into());
                    let is_self = sender == user_name || m.from.as_deref().unwrap_or("").contains(&user_name);
                    let content = m.clean_text();
                    let time = m.formatted_time();

                    MessageData {
                        id: m.id.into(),
                        sender_name: sender.into(),
                        content: content.into(),
                        timestamp: time.into(),
                        is_self,
                    }
                })
                .collect();

            let model = ModelRc::new(VecModel::from(slint_msgs));
            win.set_messages(model);
        }
        CoreEvent::NewMessageReceived(m) => {
            let user_name = win.get_user_name().to_string();
            let sender = m.im_display_name.clone().unwrap_or_else(|| "User".into());
            let is_self = sender == user_name || m.from.as_deref().unwrap_or("").contains(&user_name);
            let content = m.clean_text();
            let time = m.formatted_time();

            let new_msg = MessageData {
                id: m.id.into(),
                sender_name: sender.into(),
                content: content.into(),
                timestamp: time.into(),
                is_self,
            };

            // Append to current messages model
            let current_model = win.get_messages();
            let mut list: Vec<MessageData> = current_model.iter().collect();
            list.push(new_msg);
            win.set_messages(ModelRc::new(VecModel::from(list)));
        }
        CoreEvent::ErrorOccurred(err) => {
            win.set_connection_status(format!("Error: {}", err).into());
            win.set_is_connected(false);
        }
        CoreEvent::UpdateResult(status) => match status {
            crate::updater::UpdateStatus::UpdateAvailable { latest_version, .. } => {
                win.set_is_update_available(true);
                let msg = format!("New version {} available! Click Download.", latest_version);
                win.set_update_info_text(msg.clone().into());
                win.set_login_status(msg.into());
                win.set_connection_status(format!("Update available: {}", latest_version).into());
                WindowsNotifier::notify_chat_message(
                    "Update Available",
                    "Teams Lite",
                    &format!("Version {} is available. Click to download!", latest_version),
                );
            }
            crate::updater::UpdateStatus::UpToDate { current_version } => {
                win.set_is_update_available(false);
                let msg = format!("Teams Lite is up to date (v{})", current_version);
                win.set_login_status(msg.clone().into());
                win.set_connection_status(msg.into());
                WindowsNotifier::notify_chat_message(
                    "Teams Lite",
                    "Update Check",
                    &format!("You are on the latest version (v{}).", current_version),
                );
            }
            crate::updater::UpdateStatus::CheckFailed(err) => {
                log::warn!("Check for updates failed: {}", err);
                let msg = format!("Update check failed: {}", err);
                win.set_login_status(msg.into());
                win.set_connection_status("Update check failed".into());
                WindowsNotifier::notify_chat_message(
                    "Teams Lite",
                    "Update Check",
                    &format!("Could not check for updates: {}", err),
                );
            }
        },
        CoreEvent::UpdateProgress(msg) => {
            win.set_login_status(msg.clone().into());
            win.set_update_info_text(msg.clone().into());
            win.set_connection_status(msg.into());
        }
    }
}
