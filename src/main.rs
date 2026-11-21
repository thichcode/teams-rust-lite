#![windows_subsystem = "windows"]

use anyhow::Result;
use slint::ComponentHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

mod auth;
mod bridge;
mod teams;
mod windows_os;

slint::include_modules!();

#[tokio::main]
async fn main() -> Result<()> {
    // Setup logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("Starting Microsoft Teams Lite (Native Slint Desktop Client)...");

    // Initialize UI Window
    let main_window = MainWindow::new()?;

    // Setup Communication Channels
    let (tx_cmd, rx_cmd) = mpsc::channel(100);
    let (tx_event, rx_event) = mpsc::channel(100);

    // Initialize Core Worker in background Tokio task
    let worker = bridge::CoreWorker::new(rx_cmd, tx_event);
    tokio::spawn(async move {
        worker.run().await;
    });

    // Attach Bridge connecting UI callbacks to Async Core
    bridge::attach_ui_bridge(&main_window, tx_cmd, rx_event);

    // Setup System Tray
    let tray_manager = match windows_os::TrayManager::new() {
        Ok(tm) => Some(tm),
        Err(e) => {
            log::warn!("Could not create system tray icon: {}", e);
            None
        }
    };

    // System Tray Event Poller via Slint Timer
    let is_window_visible = Arc::new(AtomicBool::new(true));
    let weak_window = main_window.as_weak();
    let tray_timer = slint::Timer::default();

    if let Some(tray) = tray_manager {
        let is_visible = is_window_visible.clone();
        tray_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(200),
            move || {
                if let Some(action) = tray.poll_action() {
                    match action {
                        windows_os::TrayAction::Show => {
                            if let Some(win) = weak_window.upgrade() {
                                let curr = is_visible.load(Ordering::Relaxed);
                                if curr {
                                    let _ = win.hide();
                                    is_visible.store(false, Ordering::Relaxed);
                                } else {
                                    let _ = win.show();
                                    is_visible.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                        windows_os::TrayAction::Hide => {
                            if let Some(win) = weak_window.upgrade() {
                                let _ = win.hide();
                                is_visible.store(false, Ordering::Relaxed);
                            }
                        }
                        windows_os::TrayAction::Quit => {
                            let _ = slint::quit_event_loop();
                        }
                    }
                }
            },
        );
    }

    // Run Slint Event Loop (blocks main thread until application exit)
    log::info!("Launching Slint GUI Event Loop...");
    main_window.run()?;

    log::info!("Microsoft Teams Lite shut down cleanly.");
    Ok(())
}
