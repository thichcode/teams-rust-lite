#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;

use super::models::ChatMessage;

pub const DEFAULT_TROUTER_WS_URL: &str = "wss://go.trouter.teams.microsoft.com/v4/a";
pub const SKYPE_TROUTER_WS_URL: &str = "wss://go.trouter.skype.com/v4/c";

#[derive(Debug, Clone)]
pub enum TrouterNotification {
    Connected,
    Disconnected,
    NewMessage(ChatMessage),
    RawEvent(String),
}

pub struct TrouterClient {
    ws_url: String,
    token: String,
    tx_events: mpsc::Sender<TrouterNotification>,
    is_running: Arc<AtomicBool>,
}

impl TrouterClient {
    pub fn new(
        token: String,
        custom_url: Option<String>,
        tx_events: mpsc::Sender<TrouterNotification>,
    ) -> Self {
        let ws_url = custom_url.unwrap_or_else(|| DEFAULT_TROUTER_WS_URL.to_string());
        Self {
            ws_url,
            token,
            tx_events,
            is_running: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn stop(&self) {
        self.is_running.store(false, Ordering::Relaxed);
    }

    /// Connects to Trouter WebSocket and maintains heartbeat & auto-reconnect
    pub async fn run_loop(self) {
        let mut backoff_secs = 1u64;

        while self.is_running.load(Ordering::Relaxed) {
            log::info!("Connecting to Trouter WebSocket at: {}", self.ws_url);
            
            // Build WebSocket URL with query parameters
            let connect_url_result = Url::parse(&self.ws_url).and_then(|mut u| {
                u.query_pairs_mut()
                    .append_pair("auth", &self.token)
                    .append_pair("clientver", "1.0")
                    .append_pair("transport", "websocket");
                Ok(u)
            });

            let connect_url = match connect_url_result {
                Ok(u) => u.to_string(),
                Err(e) => {
                    log::error!("Invalid Trouter URL: {}", e);
                    sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            match connect_async(&connect_url).await {
                Ok((ws_stream, _response)) => {
                    log::info!("Successfully connected to Trouter real-time gateway!");
                    let _ = self.tx_events.send(TrouterNotification::Connected).await;
                    backoff_secs = 1; // Reset backoff on success

                    let (mut write, mut read) = ws_stream.split();
                    let mut ping_ticker = interval(Duration::from_secs(25));

                    loop {
                        tokio::select! {
                            _ = ping_ticker.tick() => {
                                // Engine.io ping packet '2'
                                if let Err(e) = write.send(Message::Text("2".to_string())).await {
                                    log::warn!("Failed to send Trouter ping: {}", e);
                                    break;
                                }
                            }
                            msg_opt = read.next() => {
                                match msg_opt {
                                    Some(Ok(Message::Text(text))) => {
                                        self.handle_incoming_text(&text, &mut write).await;
                                    }
                                    Some(Ok(Message::Ping(payload))) => {
                                        let _ = write.send(Message::Pong(payload)).await;
                                    }
                                    Some(Ok(Message::Close(_))) | None => {
                                        log::warn!("Trouter WebSocket closed by remote server");
                                        break;
                                    }
                                    Some(Err(e)) => {
                                        log::warn!("Trouter WebSocket error: {}", e);
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    let _ = self.tx_events.send(TrouterNotification::Disconnected).await;
                }
                Err(e) => {
                    log::warn!("Trouter connection failed: {}. Retrying in {}s...", e, backoff_secs);
                }
            }

            if !self.is_running.load(Ordering::Relaxed) {
                break;
            }

            sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(30);
        }

        log::info!("Trouter client stopped.");
    }

    async fn handle_incoming_text<S>(&self, text: &str, write: &mut S)
    where
        S: SinkExt<Message> + Unpin,
    {
        // Engine.io protocol handling:
        // '0' -> Handshake response: 0{"sid":"...","pingInterval":25000,...}
        // '2' -> Server ping, respond with '3' (pong)
        // '3' -> Server pong to our ping
        // '42' -> Socket.io event packet: 42["message", {...}]
        if text.starts_with('2') {
            let _ = write.send(Message::Text("3".to_string())).await;
            return;
        }

        if text.starts_with("42") {
            let payload_str = &text[2..];
            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(payload_str) {
                if let Some(arr) = json_val.as_array() {
                    if let Some(event_body) = arr.get(1) {
                        self.parse_and_dispatch_event(event_body).await;
                    }
                }
            }
        } else if text.starts_with('{') {
            // Raw JSON event
            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(text) {
                self.parse_and_dispatch_event(&json_val).await;
            }
        }
    }

    async fn parse_and_dispatch_event(&self, val: &serde_json::Value) {
        // Check if there is an embedded message inside the event
        if let Some(body) = val.get("body").or_else(|| val.get("resource")) {
            if let Ok(msg) = serde_json::from_value::<ChatMessage>(body.clone()) {
                if msg.content.is_some() {
                    let _ = self.tx_events.send(TrouterNotification::NewMessage(msg)).await;
                    return;
                }
            }
        }

        // Fallback: send raw notification
        let _ = self.tx_events.send(TrouterNotification::RawEvent(val.to_string())).await;
    }
}
