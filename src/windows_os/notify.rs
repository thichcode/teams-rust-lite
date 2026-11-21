use anyhow::Result;
use notify_rust::Notification;

pub struct WindowsNotifier;

impl WindowsNotifier {
    /// Shows a native Windows Toast notification
    pub fn show(title: &str, body: &str) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            Notification::new()
                .app_id("Microsoft.Teams.Lite")
                .summary(title)
                .body(body)
                .timeout(notify_rust::Timeout::Milliseconds(5000))
                .show()?;
        }

        #[cfg(not(target_os = "windows"))]
        {
            log::info!("Toast Notification [{}]: {}", title, body);
        }

        Ok(())
    }

    /// Notification specifically formatted for an incoming chat message
    pub fn notify_chat_message(sender_name: &str, conversation_title: &str, clean_content: &str) {
        let title = if conversation_title.is_empty() || conversation_title == sender_name {
            format!("Teams: {}", sender_name)
        } else {
            format!("Teams: {} ({})", sender_name, conversation_title)
        };

        let preview = if clean_content.len() > 140 {
            format!("{}...", &clean_content[..140])
        } else {
            clean_content.to_string()
        };

        if let Err(e) = Self::show(&title, &preview) {
            log::warn!("Failed to display Windows toast notification: {}", e);
        }
    }
}
