use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

pub enum Action {
    Show,
    Hide,
    Quit,
}
pub struct Tray {
    _icon: TrayIcon,
    show: MenuItem,
    hide: MenuItem,
    quit: MenuItem,
}
impl Tray {
    pub fn new() -> anyhow::Result<Self> {
        let menu = Menu::new();
        let show = MenuItem::new("Open Snapgent", true, None);
        let hide = MenuItem::new("Hide window", true, None);
        let quit = MenuItem::new("Quit Snapgent", true, None);
        menu.append_items(&[&show, &hide, &quit])?;
        let asset = crate::theme::icon()
            .ok_or_else(|| anyhow::anyhow!("Unable to decode Snapgent icon"))?;
        let icon = Icon::from_rgba(asset.rgba, asset.width, asset.height)?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("Snapgent Native MCP Bridge")
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .build()?;
        Ok(Self {
            _icon: tray,
            show,
            hide,
            quit,
        })
    }
    pub fn poll(&self) -> Option<Action> {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if &event.id == self.show.id() {
                return Some(Action::Show);
            }
            if &event.id == self.hide.id() {
                return Some(Action::Hide);
            }
            if &event.id == self.quit.id() {
                return Some(Action::Quit);
            }
        }
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                return Some(Action::Show);
            }
        }
        None
    }
}
