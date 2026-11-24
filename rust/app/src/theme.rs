use eframe::egui::{self, Color32, CornerRadius, FontData, FontFamily, FontId, Stroke, TextStyle};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(22, 22, 24);
pub const PANEL: Color32 = Color32::from_rgb(30, 30, 32);
pub const CONSOLE: Color32 = Color32::from_rgb(10, 17, 20);
pub const BORDER: Color32 = Color32::from_rgb(56, 56, 59);
pub const GOLD: Color32 = Color32::from_rgb(34, 211, 238);
pub const GREEN: Color32 = Color32::from_rgb(92, 191, 154);
pub const TEXT: Color32 = Color32::from_rgb(226, 236, 240);
pub const MUTED: Color32 = Color32::from_rgb(168, 168, 174);
pub const RED: Color32 = Color32::from_rgb(239, 107, 107);

pub fn icon() -> Option<egui::IconData> {
    eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon.png")).ok()
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    #[cfg(windows)]
    {
        let directory = std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("C:/Windows"))
            .join("Fonts");
        for (name, filename, family) in [
            ("Segoe UI", "segoeui.ttf", FontFamily::Proportional),
            ("Cascadia Mono", "CascadiaMono.ttf", FontFamily::Monospace),
            ("Consolas", "consola.ttf", FontFamily::Monospace),
        ] {
            if let Ok(bytes) = std::fs::read(directory.join(filename)) {
                fonts
                    .font_data
                    .insert(name.into(), Arc::new(FontData::from_owned(bytes)));
                let list = fonts.families.entry(family).or_default();
                if name == "Consolas" && list.first().is_some_and(|s| s == "Cascadia Mono") {
                    list.insert(1, name.into());
                } else {
                    list.insert(0, name.into());
                }
            }
        }
    }
    ctx.set_fonts(fonts);
    let mut style = (*ctx.style()).clone();
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = CONSOLE;
    v.override_text_color = Some(TEXT);
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = CornerRadius::same(8);
    for widget in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        widget.bg_fill = PANEL;
        widget.weak_bg_fill = PANEL;
        widget.bg_stroke = Stroke::new(1.0_f32, BORDER);
        widget.fg_stroke = Stroke::new(1.0_f32, TEXT);
        widget.corner_radius = CornerRadius::same(6);
        widget.expansion = 0.0;
    }
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(44, 44, 46);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, MUTED);
    v.widgets.active.bg_stroke = Stroke::new(2.0_f32, GOLD);
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, GOLD);
    v.selection.bg_fill = Color32::from_rgb(15, 53, 62);
    v.selection.stroke = Stroke::new(1.0_f32, GOLD);
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    style.spacing.interact_size = egui::vec2(40.0, 32.0);
    style.animation_time = 0.0;
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(14.0));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(13.0));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(20.0));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(12.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(12.0));
    ctx.set_style(style);
}
