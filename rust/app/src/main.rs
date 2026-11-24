#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod settings;
mod theme;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Stroke};
use serde_json::{Value, json};
use settings::Settings;
use snapgent_bridge::{Approval, State};
use std::{
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use theme::{BG, BORDER, CONSOLE, GOLD, GREEN, MUTED, PANEL, RED, TEXT};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Activity,
    Settings,
}

struct Desktop {
    settings: Option<Settings>,
    runtime: Option<tokio::runtime::Runtime>,
    state: Option<Arc<State>>,
    running: Arc<Mutex<String>>,
    snapshot: Arc<Mutex<Value>>,
    worker: Option<tokio::task::JoinHandle<()>>,
    approval: Option<Approval>,
    message: String,
    page: Page,
    filter: String,
    logs: Vec<String>,
    last_seq: u64,
    trust: bool,
}

impl Desktop {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let (settings, message) = match Settings::load() {
            Ok(s) => (Some(s), String::new()),
            Err(e) => (None, e.to_string()),
        };
        let page = if settings
            .as_ref()
            .is_none_or(|s| !Settings::valid_id(&s.extension_id))
        {
            Page::Settings
        } else {
            Page::Overview
        };
        let mut desktop = Self {
            settings,
            runtime: Some(tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("Tokio runtime")),
            state: None,
            running: Arc::new(Mutex::new("Stopped".into())),
            snapshot: Arc::new(Mutex::new(json!({}))),
            worker: None,
            approval: None,
            message,
            page,
            filter: String::new(),
            logs: vec![],
            last_seq: 0,
            trust: false,
        };
        if desktop
            .settings
            .as_ref()
            .is_some_and(|s| Settings::valid_id(&s.extension_id))
        {
            desktop.start(&cc.egui_ctx);
        }
        desktop
    }
    fn status(&self) -> String {
        self.running.lock().map(|s| s.clone()).unwrap_or_default()
    }
    fn active(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| !w.is_finished())
    }
    fn start(&mut self, ctx: &egui::Context) {
        if self.active() {
            return;
        }
        let Some(settings) = self.settings.as_mut() else {
            return;
        };
        if let Err(e) = settings.save_origin() {
            self.message = e.to_string();
            self.page = Page::Settings;
            return;
        }
        let state = match State::new(
            settings.root.clone(),
            settings.token.clone(),
            settings.origins(),
        ) {
            Ok(s) => s,
            Err(e) => {
                self.message = e.to_string();
                return;
            }
        };
        self.trust = false;
        self.last_seq = 0;
        self.message.clear();
        self.state = Some(state.clone());
        *self.running.lock().unwrap() = "Starting".into();
        *self.snapshot.lock().unwrap() = json!({});
        let status = self.running.clone();
        let snapshot = self.snapshot.clone();
        let worker_state = state.clone();
        let repaint = ctx.clone();
        self.worker = Some(self.runtime.as_ref().expect("runtime").spawn(async move {
            let observer = tokio::spawn(async move {
                loop {
                    let snap = worker_state.manager.snapshot().await;
                    if let Ok(mut s) = snapshot.lock() {
                        *s = snap;
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            });
            let (_, mut events) = state.events.subscribe(0);
            let wake = repaint.clone();
            let event_worker = tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            wake.request_repaint()
                        }
                        Err(_) => break,
                    }
                }
            });
            let result = state.clone().run(17613, 17614).await;
            observer.abort();
            event_worker.abort();
            if let Ok(mut s) = status.lock() {
                *s = match result {
                    Ok(()) => "Stopped".into(),
                    Err(e) => format!("Error: {e}"),
                };
            }
            repaint.request_repaint();
        }));
    }
    fn stop(&mut self) {
        if let Some(approval) = self.approval.take() {
            approval.respond(false);
        }
        if let Some(state) = &self.state {
            state.approvals.set_trusted_terminal(false);
            state.request_stop();
        }
        self.trust = false;
        if self.active() {
            *self.running.lock().unwrap() = "Stopping".into();
        }
    }
    fn refresh(&mut self) {
        if let Some(state) = &self.state {
            if self.approval.as_ref().is_some_and(|a| a.expired()) {
                self.approval = None;
            }
            if self.approval.is_none() && self.status() == "Running" {
                self.approval = state.approvals.take();
            }
            for event in state.events.recent(self.last_seq) {
                self.last_seq = event["seq"].as_u64().unwrap_or(self.last_seq);
                if event["source"] == "client" {
                    continue;
                }
                let kind = event["event"].as_str().unwrap_or("");
                let line = match kind {
                    "log" => event["message"].as_str().unwrap_or("").to_owned(),
                    "tool_start" => format!(
                        "START  {}  {}",
                        event["server"].as_str().unwrap_or(""),
                        event["tool"].as_str().unwrap_or("")
                    ),
                    "tool_end" => format!(
                        "{}  {}  {} ms",
                        if event["ok"] == true {
                            "DONE"
                        } else {
                            "FAILED"
                        },
                        event["tool"].as_str().unwrap_or(""),
                        event["ms"]
                    ),
                    _ => continue,
                };
                let line: String = line
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(1500)
                    .collect();
                if kind == "log"
                    && line.starts_with("Bridge listening")
                    && self.status() == "Starting"
                {
                    *self.running.lock().unwrap() = "Running".into();
                }
                self.logs.push(line.clone());
                if let Some(settings) = &self.settings {
                    let folder = settings.root.join("logs");
                    let _ = std::fs::create_dir_all(&folder);
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(folder.join("native-bridge.log"))
                    {
                        let _ = writeln!(file, "{:.3} {line}", event["at"].as_f64().unwrap_or(0.0));
                    }
                }
            }
            if self.logs.len() > 500 {
                self.logs.drain(..self.logs.len() - 500);
            }
        }
    }
    fn overview(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let status = self.status();
        let active = self.active();
        let snap = self.snapshot.lock().map(|s| s.clone()).unwrap_or_default();
        let servers = snap["servers"].as_array().cloned().unwrap_or_default();
        let tools = snap["tools"].as_array().map(Vec::len).unwrap_or(0);
        let queue = self
            .state
            .as_ref()
            .map(|s| s.events.snapshot()["queue"].as_u64().unwrap_or(0))
            .unwrap_or(0);
        panel(ui, PANEL, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Local bridge").size(17.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if active {
                        if ui
                            .add_enabled(status != "Stopping", egui::Button::new("Stop bridge"))
                            .clicked()
                        {
                            self.stop();
                        }
                    } else if ui
                        .add_enabled(self.settings.is_some(), primary("Start bridge"))
                        .clicked()
                    {
                        self.start(ctx);
                    }
                });
            });
            ui.horizontal_wrapped(|ui| {
                let color = if status == "Running" {
                    GREEN
                } else if status.starts_with("Error") {
                    RED
                } else if active {
                    GOLD
                } else {
                    MUTED
                };
                dot(ui, color);
                ui.label(
                    RichText::new(if status.starts_with("Error") {
                        "Failed to start"
                    } else {
                        status.as_str()
                    })
                    .color(color)
                    .strong(),
                );
            });
            if status.starts_with("Error") {
                ui.label(RichText::new(&status).color(RED));
                ui.label(
                    RichText::new("If another Snapgent process owns these ports, close that instance before starting this bridge.")
                        .color(MUTED),
                );
            } else if status == "Starting" {
                ui.label(
                    RichText::new("Opening local listeners and loading MCP servers.").color(MUTED),
                );
            } else if status == "Stopped" {
                ui.label(
                    RichText::new("Start the bridge to accept extension connections.").color(MUTED),
                );
            } else if status == "Stopping" {
                ui.label(RichText::new("Closing local listeners.").color(MUTED));
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("127.0.0.1:17613  /  Events :17614")
                    .monospace()
                    .size(12.0)
                    .color(MUTED),
            );
            if status == "Running" {
                ui.separator();
                ui.label(format!(
                    "{} servers   /   {} tools   /   {} active calls",
                    servers.len(),
                    tools,
                    queue
                ));
            }
        });
        ui.add_space(16.0);
        ui.label(RichText::new("MCP servers").size(16.0).strong());
        ui.add_space(4.0);
        if servers.is_empty() || !active {
            ui.label(
                RichText::new(if active {
                    "Waiting for the server catalog."
                } else {
                    "Start the bridge to inspect the configured servers."
                })
                .color(MUTED),
            );
        } else {
            for server in servers {
                let id = server["id"].as_str().unwrap_or("unknown");
                let alive = server["alive"].as_bool().unwrap_or(false);
                let count = server["tools"].as_u64().unwrap_or(0);
                ui.horizontal_wrapped(|ui| {
                    dot(ui, if alive { GREEN } else { RED });
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(match id {
                                "local" => "Local Workspace",
                                _ => id,
                            })
                            .strong(),
                        );
                        ui.label(
                            RichText::new(if id == "local" {
                                "Built-in Rust adapter"
                            } else {
                                "External MCP process"
                            })
                            .small()
                            .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(if alive {
                                format!("{count} tools")
                            } else {
                                "Unavailable".into()
                            })
                            .small()
                            .color(if alive { MUTED } else { RED }),
                        );
                    });
                });
                ui.separator();
            }
        }
    }
    fn activity(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(RichText::new("Activity log").size(17.0).strong());
        ui.label(RichText::new("Bridge events and tool execution results.").color(MUTED));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.logs.is_empty(), egui::Button::new("Copy"))
                .clicked()
            {
                ctx.copy_text(self.logs.join("\n"));
                self.message = "Activity copied.".into();
            }
            if let Some(settings) = &self.settings {
                let log = settings.root.join("logs/native-bridge.log");
                if ui
                    .add_enabled(log.is_file(), egui::Button::new("Open file"))
                    .clicked()
                {
                    if let Err(e) = open_log(&log, false) {
                        self.message = e.to_string();
                    }
                }
                if ui
                    .add_enabled(log.is_file(), egui::Button::new("Follow in terminal"))
                    .clicked()
                {
                    if let Err(e) = open_log(&log, true) {
                        self.message = e.to_string();
                    }
                }
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter activity")
                .desired_width(f32::INFINITY),
        );
        let query = self.filter.to_lowercase();
        panel(ui, CONSOLE, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("activity_log")
                .stick_to_bottom(true)
                .max_height(ui.available_height().max(100.0))
                .show(ui, |ui| {
                    let mut found = false;
                    for line in &self.logs {
                        if !query.is_empty() && !line.to_lowercase().contains(&query) {
                            continue;
                        }
                        found = true;
                        let color = if line.starts_with("FAILED") {
                            RED
                        } else if line.starts_with("DONE") {
                            GREEN
                        } else {
                            TEXT
                        };
                        ui.label(RichText::new(line).monospace().size(12.0).color(color));
                    }
                    if !found {
                        ui.label(
                            RichText::new(if self.logs.is_empty() {
                                "No events yet. Start the bridge to record activity."
                            } else {
                                "No events match this filter."
                            })
                            .color(MUTED),
                        );
                    }
                });
        });
    }
    fn settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let active = self.active();
        ui.label(RichText::new("Extension pairing").size(17.0).strong());
        ui.label(RichText::new("Use the extension ID from chrome://extensions. Only that extension can pair with this bridge.").color(MUTED));
        if let Some(settings) = self.settings.as_mut() {
            ui.label("Trusted extension ID");
            ui.add_enabled(
                !active,
                egui::TextEdit::singleline(&mut settings.extension_id)
                    .hint_text("32-character extension ID")
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!active, egui::Button::new("Save extension ID"))
                    .clicked()
                {
                    self.message = match settings.save_origin() {
                        Ok(()) => "Trusted extension saved.".into(),
                        Err(e) => e.to_string(),
                    };
                }
                if ui.button("Copy pairing credential").clicked() {
                    ctx.copy_text(settings.token.clone());
                    self.message =
                        "Credential copied. Paste only into Snapgent extension pairing.".into();
                }
            });
            if active {
                ui.label(
                    RichText::new("Stop the bridge before changing the trusted extension.")
                        .small()
                        .color(MUTED),
                );
            }
        } else {
            ui.label(
                RichText::new(
                    "Settings could not be loaded. Check the project config and reopen Snapgent.",
                )
                .color(RED),
            );
        }
        ui.add_space(16.0);
        ui.separator();
        ui.label(RichText::new("Local permissions").size(16.0).strong());
        if ui
            .add_enabled(
                active,
                egui::Checkbox::new(
                    &mut self.trust,
                    "Trust terminal commands until bridge stops (all sessions)",
                ),
            )
            .changed()
        {
            if let Some(state) = &self.state {
                state.approvals.set_trusted_terminal(self.trust);
            }
        }
        ui.label(RichText::new("Off by default. This option applies to every connected agent. Workspace binding and administrative changes still require local approval. Trust resets when the bridge stops.").color(MUTED));
    }
}

fn panel(ui: &mut egui::Ui, fill: Color32, body: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}
fn primary(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).color(BG).strong()).fill(GOLD)
}
fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}
fn open_log(path: &Path, follow: bool) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if follow {
            let quoted = path.to_string_lossy().replace('\'', "''");
            std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NoExit",
                    "-Command",
                    &format!("Get-Content -LiteralPath '{quoted}' -Tail 100 -Wait"),
                ])
                .spawn()?;
        } else {
            std::process::Command::new("notepad.exe")
                .arg(path)
                .spawn()?;
        }
    }
    #[cfg(not(windows))]
    {
        let _ = follow;
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Closing the window is a real shutdown, not a hide-to-tray action.
        if ctx.input(|i| i.viewport().close_requested()) {
            self.stop();
        }
        self.refresh();
        egui::TopBottomPanel::top("header")
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(16)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Snapgent").size(22.0).strong());
                    ui.label(
                        RichText::new(env!("CARGO_PKG_VERSION"))
                            .small()
                            .color(MUTED),
                    );
                });
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for (page, label) in [
                        (Page::Overview, "Overview"),
                        (Page::Activity, "Activity"),
                        (Page::Settings, "Settings"),
                    ] {
                        let selected = self.page == page;
                        let label = RichText::new(label).color(if selected { GOLD } else { MUTED });
                        if ui
                            .add(
                                egui::Button::new(if selected { label.strong() } else { label })
                                    .selected(selected),
                            )
                            .clicked()
                        {
                            self.page = page;
                            self.message.clear();
                        }
                    }
                });
            });
        egui::TopBottomPanel::bottom("footer")
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(16, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Local MCP bridge").small().color(MUTED));
                    ui.label(RichText::new("Closing this window stops the bridge.").small().color(MUTED));
                });
                if !self.message.is_empty() {
                    ui.label(RichText::new(&self.message).small().color(GOLD));
                }
            });
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(16)))
            .show(ctx, |ui| {
                ui.add_enabled_ui(self.approval.is_none(), |ui| {
                    if self.page == Page::Activity {
                        self.activity(ui, ctx);
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt("main_content")
                            .show(ui, |ui| match self.page {
                                Page::Overview => self.overview(ui, ctx),
                                Page::Settings => self.settings(ui, ctx),
                                Page::Activity => {}
                            });
                    }
                });
            });
        if let Some(approval) = &self.approval {
            let mut answer = if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                Some(false)
            } else {
                None
            };
            egui::Window::new("Local approval required")
                .collapsible(false)
                .resizable(true)
                .default_width(400.0)
                .max_width((ctx.content_rect().width() - 32.0).max(220.0))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(RichText::new(&approval.title).strong().color(GOLD));
                    ui.label("An authenticated client requested a privileged operation.");
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(
                                    serde_json::to_string_pretty(&approval.details)
                                        .unwrap_or_default(),
                                )
                                .monospace(),
                            );
                        });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Deny").clicked() {
                            answer = Some(false);
                        }
                        if ui.add(primary("Allow once")).clicked() {
                            answer = Some(true);
                        }
                    });
                });
            if let Some(answer) = answer {
                if let Some(approval) = self.approval.take() {
                    approval.respond(answer);
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(if self.active() {
            750
        } else {
            1500
        }));
    }
}
impl Drop for Desktop {
    fn drop(&mut self) {
        self.stop();
        if let Some(runtime) = self.runtime.take() {
            if let Some(mut worker) = self.worker.take() {
                if runtime.block_on(async {
                    tokio::time::timeout(Duration::from_secs(5), &mut worker).await
                }).is_err() {
                    worker.abort();
                }
            }
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}
fn main() -> eframe::Result<()> {
    // Old Native Messaging registrations may still invoke this executable.
    // Never start a bridge or return credentials to that legacy invocation.
    if std::env::args().nth(1).is_some_and(|arg| arg.starts_with("chrome-extension://")) {
        return Ok(());
    }
    // All normal invocations open the desktop UI, including obsolete CLI flags.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Snapgent")
        .with_inner_size([540.0, 570.0])
        .with_min_inner_size([400.0, 420.0]);
    if let Some(icon) = theme::icon() {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        "Snapgent",
        eframe::NativeOptions {
            viewport,
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(Desktop::new(cc)))),
    )
}
