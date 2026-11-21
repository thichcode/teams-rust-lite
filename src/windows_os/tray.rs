use anyhow::{Context, Result};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[allow(dead_code)]
pub enum TrayAction {
    Show,
    Hide,
    Quit,
}

pub struct TrayManager {
    _tray_icon: TrayIcon,
    menu_item_show: MenuItem,
    menu_item_quit: MenuItem,
}

impl TrayManager {
    pub fn new() -> Result<Self> {
        let tray_menu = Menu::new();
        let menu_item_show = MenuItem::new("Show / Hide Teams Lite", true, None);
        let menu_item_quit = MenuItem::new("Quit", true, None);

        tray_menu.append(&menu_item_show)?;
        tray_menu.append(&PredefinedMenuItem::separator())?;
        tray_menu.append(&menu_item_quit)?;

        let icon = Self::create_default_icon().context("Failed to build tray icon")?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("Microsoft Teams Lite")
            .with_icon(icon)
            .build()
            .map_err(|e| anyhow::anyhow!("Tray icon initialization error: {}", e))?;

        Ok(Self {
            _tray_icon: tray_icon,
            menu_item_show,
            menu_item_quit,
        })
    }

    /// Generates a crisp 32x32 RGBA Teams purple icon
    fn create_default_icon() -> Result<Icon> {
        let width = 32u32;
        let height = 32u32;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);

        for y in 0..height {
            for x in 0..width {
                // Rounded corner check
                let cx = x as f32 - 15.5;
                let cy = y as f32 - 15.5;
                let dist = (cx * cx + cy * cy).sqrt();

                if dist > 15.5 {
                    // Transparent exterior
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    // Teams purple: #5b5fc7 (R: 91, G: 95, B: 199)
                    // Draw a white 'T' inside
                    let is_t_bar = y >= 8 && y <= 12 && x >= 8 && x <= 23;
                    let is_t_stem = y >= 12 && y <= 24 && x >= 13 && x <= 18;

                    if is_t_bar || is_t_stem {
                        rgba.extend_from_slice(&[255, 255, 255, 255]); // White 'T'
                    } else {
                        rgba.extend_from_slice(&[91, 95, 199, 255]);   // Purple background
                    }
                }
            }
        }

        Icon::from_rgba(rgba, width, height)
            .map_err(|e| anyhow::anyhow!("Icon::from_rgba failed: {}", e))
    }

    /// Non-blocking check for tray menu actions
    pub fn poll_action(&self) -> Option<TrayAction> {
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.menu_item_show.id() {
                return Some(TrayAction::Show);
            } else if event.id == self.menu_item_quit.id() {
                return Some(TrayAction::Quit);
            }
        }

        if let Ok(tray_event) = TrayIconEvent::receiver().try_recv() {
            match tray_event {
                TrayIconEvent::Click { button, .. } => {
                    if button == tray_icon::MouseButton::Left {
                        return Some(TrayAction::Show);
                    }
                }
                _ => {}
            }
        }

        None
    }
}
