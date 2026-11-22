#![windows_subsystem = "windows"]

use anyhow::Result;
use slint::ComponentHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

mod auth;
mod bridge;
mod logger;
mod solar;
mod teams;
mod updater;
mod windows_os;

slint::include_modules!();

#[tokio::main]
async fn main() -> Result<()> {
    // Setup persistent file + stderr logger
    let _ = logger::init();
    log::info!("Starting Microsoft Teams Lite (Native Slint Desktop Client)...");

    // Initialize UI Window
    let main_window = MainWindow::new()?;
    main_window.set_app_version(crate::updater::CURRENT_VERSION.into());

    // Initialize Solar Dynamic Theme (Auto switch light/dark based on sunrise/sunset)
    let current_theme_mode = Arc::new(std::sync::Mutex::new(solar::ThemeMode::AutoSolar));
    let initial_is_dark = solar::should_be_dark_theme(solar::ThemeMode::AutoSolar);
    main_window.set_is_dark_theme(initial_is_dark);
    main_window.set_theme_mode_icon(solar::ThemeMode::AutoSolar.icon().into());
    main_window.set_theme_status_text("Auto".into());

    // Hook up Theme Mode toggle callback
    let weak_theme_win = main_window.as_weak();
    let theme_mode_toggle = current_theme_mode.clone();
    main_window.on_toggle_theme_mode(move || {
        if let Ok(mut mode) = theme_mode_toggle.lock() {
            *mode = mode.next();
            let is_dark = solar::should_be_dark_theme(*mode);
            let icon = mode.icon();
            let label = match *mode {
                solar::ThemeMode::AutoSolar => "Auto",
                solar::ThemeMode::ForceLight => "Sáng",
                solar::ThemeMode::ForceDark => "Tối",
            };
            log::info!("Switched theme mode to: {:?} (is_dark: {})", *mode, is_dark);
            if let Some(win) = weak_theme_win.upgrade() {
                win.set_is_dark_theme(is_dark);
                win.set_theme_mode_icon(icon.into());
                win.set_theme_status_text(label.into());
            }
        }
    });

    // Periodic Solar Timer: checks every 60s and updates theme if in Auto mode
    let solar_timer = slint::Timer::default();
    let weak_solar_win = main_window.as_weak();
    let theme_mode_solar = current_theme_mode.clone();
    solar_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(60),
        move || {
            if let Ok(mode) = theme_mode_solar.lock() {
                if *mode == solar::ThemeMode::AutoSolar {
                    let is_dark = solar::should_be_dark_theme(solar::ThemeMode::AutoSolar);
                    if let Some(win) = weak_solar_win.upgrade() {
                        if win.get_is_dark_theme() != is_dark {
                            log::info!("Auto solar theme transition triggered: is_dark = {}", is_dark);
                            win.set_is_dark_theme(is_dark);
                        }
                    }
                }
            }
        },
    );

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
