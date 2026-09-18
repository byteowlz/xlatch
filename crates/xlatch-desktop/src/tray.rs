//! Optional native tray. Menu commands navigate; they never execute content implicitly.
use anyhow::Result;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

pub struct Tray {
    _icon: TrayIcon,
}

impl Tray {
    pub fn new() -> Result<Self> {
        let menu = Menu::new();
        menu.append_items(&[
            &MenuItem::with_id("open", "Open xlatch", true, None),
            &MenuItem::with_id("quick", "Find an action…", true, None),
            &MenuItem::with_id("activity", "Recent activity", true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("quit", "Quit xlatch app", true, None),
        ])?;
        let pixels = image::load_from_memory(include_bytes!("../assets/tray.png"))?.into_rgba8();
        let (width, height) = pixels.dimensions();
        let icon = Icon::from_rgba(pixels.into_raw(), width, height)?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("xlatch")
            .with_icon(icon)
            .with_icon_as_template(true)
            .with_menu(Box::new(menu))
            .build()?;
        Ok(Self { _icon: icon })
    }
}

pub fn next_action() -> Option<String> {
    MenuEvent::receiver()
        .try_recv()
        .ok()
        .map(|event| event.id.0)
}
