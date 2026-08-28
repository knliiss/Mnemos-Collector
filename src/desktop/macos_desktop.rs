use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use eframe::egui::{
    self, Align2, Color32, FontFamily, FontId, Id, Pos2, Rect, Sense, Stroke, Vec2,
};
use tokio::runtime::Handle;
use zeroize::Zeroizing;

use crate::application::CollectorApplication;
use crate::diagnostics::{self, RuntimeSnapshot};
use crate::platform::{Autostart, Installation};
use crate::provisioning::{ProvisioningClient, default_device_name};
use crate::security::CredentialStore;

use super::DesktopLaunchContext;
use super::macos_mascot;
use super::macos_native::{self, MacStatusItem};

const WINDOW_WIDTH: f32 = 1080.0;
const WINDOW_HEIGHT: f32 = 720.0;
const REFRESH_INTERVAL: Duration = Duration::from_millis(500);

const CONTENT_MARGIN: f32 = 22.0;
const HEADER_HEIGHT: f32 = 68.0;
const HERO_TOP: f32 = 74.0;
const HERO_HEIGHT: f32 = 154.0;
const CARD_RADIUS: f32 = 24.0;
const STATUS_TILE_HEIGHT: f32 = 46.0;
const STATUS_TILE_GAP: f32 = 8.0;
const ACTIVATION_HEIGHT: f32 = 134.0;
const SECTION_GAP: f32 = 16.0;
const LOG_HEADER_HEIGHT: f32 = 52.0;
const LOG_LINE_HEIGHT: f32 = 18.0;
const LOG_CHAR_WIDTH: f32 = 8.0;

const UI_FONT_SIZE: f32 = 16.0;
const TITLE_FONT_SIZE: f32 = 29.0;
const SECTION_FONT_SIZE: f32 = 21.0;
const MONO_FONT_SIZE: f32 = 14.0;

const BACKGROUND: Color32 = Color32::from_rgb(0x02, 0x03, 0x02);
const LOG_SURFACE: Color32 = Color32::from_rgb(0x0b, 0x0c, 0x09);
const SURFACE: Color32 = Color32::from_rgb(0x15, 0x16, 0x12);
const SURFACE_RAISED: Color32 = Color32::from_rgb(0x1f, 0x20, 0x1a);
const LINE: Color32 = Color32::from_rgb(0x35, 0x38, 0x31);
const LINE_STRONG: Color32 = Color32::from_rgb(0x4a, 0x4e, 0x44);
const TEXT: Color32 = Color32::from_rgb(0xf5, 0xf6, 0xef);
const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xc2, 0xc4, 0xb8);
const TEXT_MUTED: Color32 = Color32::from_rgb(0x7c, 0x80, 0x72);
const ACCENT: Color32 = Color32::from_rgb(0xcb, 0xff, 0x2d);
const ACCENT_DIM: Color32 = Color32::from_rgb(0x26, 0x31, 0x0d);
const AMBER: Color32 = Color32::from_rgb(0xff, 0xb3, 0x4f);
const DANGER_DIM: Color32 = Color32::from_rgb(0x31, 0x16, 0x18);
const DANGER: Color32 = Color32::from_rgb(0xff, 0x68, 0x73);
const POSITIVE: Color32 = Color32::from_rgb(0xbd, 0xe0, 0x6d);

pub fn run(context: DesktopLaunchContext, runtime: Handle) -> Result<bool> {
    let launch_installed = Arc::new(AtomicBool::new(false));
    let launch_signal = Arc::clone(&launch_installed);
    let viewport = egui::ViewportBuilder::default()
        .with_title("Mnemos Collector")
        .with_app_id("rest.knalis.mnemos-collector")
        .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
        .with_min_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
        .with_max_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_decorations(false)
        .with_icon(macos_mascot::icon(32));
    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        ..eframe::NativeOptions::default()
    };

    eframe::run_native(
        "Mnemos Collector",
        options,
        Box::new(move |creation_context| {
            configure_style(&creation_context.egui_ctx);

            Ok(Box::new(MacDesktop::new(context, runtime, launch_signal)))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    Ok(launch_installed.load(Ordering::Acquire))
}

pub fn show_fatal_error(message: &str) {
    let escaped = message
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    let script = format!(
        "display alert \"Mnemos Collector\" message \"{escaped}\" as critical buttons {{\"OK\"}} default button \"OK\""
    );

    if Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }

    eprintln!("Mnemos Collector: {message}");
}

enum ActivationOutcome {
    CurrentInstallation(String),
    Installed,
}

struct MacDesktop {
    runtime: Handle,
    current_installation: bool,
    provisioned: bool,
    worker_started: bool,
    provisioning: bool,
    exit_requested: bool,
    activation_token: String,
    device_name: String,
    activation_error: Option<String>,
    activation_receiver: Option<Receiver<Result<ActivationOutcome, String>>>,
    log_scroll_from_bottom: usize,
    selected_log_entry: Option<usize>,
    launch_installed: Arc<AtomicBool>,
    _status_item: Option<MacStatusItem>,
}

impl MacDesktop {
    fn new(
        context: DesktopLaunchContext,
        runtime: Handle,
        launch_installed: Arc<AtomicBool>,
    ) -> Self {
        let status_item = match MacStatusItem::install() {
            Ok(status_item) => Some(status_item),
            Err(error) => {
                diagnostics::warn(
                    "desktop",
                    format!("macOS status bar item could not be created: {error:#}"),
                );
                None
            }
        };
        let mut desktop = Self {
            runtime,
            current_installation: context.current_installation,
            provisioned: context.access_key.is_some(),
            worker_started: false,
            provisioning: false,
            exit_requested: false,
            activation_token: String::new(),
            device_name: default_device_name(),
            activation_error: None,
            activation_receiver: None,
            log_scroll_from_bottom: 0,
            selected_log_entry: None,
            launch_installed,
            _status_item: status_item,
        };

        if let Some(access_key) = context.access_key {
            desktop.start_collector(access_key);
        }

        desktop
    }

    fn start_collector(&mut self, access_key: String) {
        if self.worker_started {
            return;
        }

        self.worker_started = true;
        spawn_collector(self.runtime.clone(), access_key);
    }

    fn begin_activation(&mut self) {
        if self.provisioned || self.provisioning {
            return;
        }

        if self.activation_token.trim().is_empty() {
            self.activation_error = Some("Введите одноразовый код активации из Mnemos.".to_owned());
            return;
        }

        let token = Zeroizing::new(std::mem::take(&mut self.activation_token));
        let device_name = if self.device_name.trim().is_empty() {
            default_device_name()
        } else {
            self.device_name.trim().to_owned()
        };
        let current_installation = self.current_installation;
        let runtime = self.runtime.clone();
        let (sender, receiver) = mpsc::channel();

        self.provisioning = true;
        self.activation_error = None;
        self.activation_receiver = Some(receiver);

        runtime.spawn(async move {
            diagnostics::info("provisioning", "Activation started from macOS desktop UI");

            let result = if current_installation {
                provision_current_installation(token.as_str(), &device_name)
                    .await
                    .map(ActivationOutcome::CurrentInstallation)
            } else {
                provision_and_install(token.as_str(), &device_name)
                    .await
                    .map(|()| ActivationOutcome::Installed)
            };

            let result = result.map_err(|error| format!("{error:#}"));
            let _ = sender.send(result);
        });
    }

    fn poll_activation(&mut self, context: &egui::Context) {
        let result = match self.activation_receiver.as_ref() {
            Some(receiver) => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(
                    "Задача активации неожиданно завершилась без результата.".to_owned(),
                )),
            },
            None => None,
        };

        let Some(result) = result else {
            return;
        };

        self.activation_receiver = None;
        self.provisioning = false;

        match result {
            Ok(ActivationOutcome::CurrentInstallation(access_key)) => {
                diagnostics::info("provisioning", "Activation completed successfully");
                self.provisioned = true;
                self.activation_error = None;
                self.start_collector(access_key);
            }
            Ok(ActivationOutcome::Installed) => {
                diagnostics::info(
                    "provisioning",
                    "Provisioned Collector installed; stable launch will follow instance-lock release",
                );
                self.launch_installed.store(true, Ordering::Release);
                self.exit_requested = true;
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(message) => {
                diagnostics::error("provisioning", message.clone());
                self.activation_error = Some(message);
            }
        }
    }

    fn handle_external_close(&self, context: &egui::Context) {
        let close_requested = context.input(|input| input.viewport().close_requested());

        if close_requested && !self.exit_requested {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            macos_native::hide_application();
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui, context: &egui::Context, runtime: &RuntimeSnapshot) {
        let layout = Layout::new(self.provisioned);

        self.draw_header(ui, context, layout);
        self.draw_hero(ui, runtime, layout.hero);

        if let Some(activation) = layout.activation {
            self.draw_activation(ui, activation, layout);
        }

        self.draw_journal(ui, context, layout);
        self.draw_version(ui, layout);
    }

    fn draw_header(&mut self, ui: &mut egui::Ui, context: &egui::Context, layout: Layout) {
        let icon = rect(22.0, 12.0, 44.0, 44.0);

        draw_card(ui.painter(), icon, SURFACE, LINE, 18.0);
        macos_mascot::draw(ui.painter(), icon.shrink(2.0));
        draw_text(
            ui.painter(),
            Pos2::new(78.0, 14.0),
            "MNEMOS",
            UI_FONT_SIZE,
            ACCENT,
        );
        draw_text(
            ui.painter(),
            Pos2::new(78.0, 34.0),
            "Collector",
            SECTION_FONT_SIZE,
            TEXT,
        );

        let minimize = ui.interact(
            layout.window_minimize,
            Id::new("macos-window-minimize"),
            Sense::click(),
        );
        draw_window_button(
            ui.painter(),
            layout.window_minimize,
            "—",
            false,
            minimize.hovered(),
        );

        if minimize.clicked() {
            context.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        let close = ui.interact(
            layout.window_close,
            Id::new("macos-window-close"),
            Sense::click(),
        );
        draw_window_button(
            ui.painter(),
            layout.window_close,
            "×",
            true,
            close.hovered(),
        );

        if close.clicked() {
            macos_native::hide_application();
        }

        let drag_region = Rect::from_min_max(
            Pos2::new(0.0, 0.0),
            Pos2::new(layout.window_minimize.left() - 8.0, HEADER_HEIGHT),
        );
        let drag = ui.interact(drag_region, Id::new("macos-titlebar-drag"), Sense::drag());

        if drag.drag_started() {
            context.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }

    fn draw_hero(&self, ui: &mut egui::Ui, runtime: &RuntimeSnapshot, hero: Rect) {
        let (title, detail, status_color) = status_copy(runtime);

        draw_card(ui.painter(), hero, SURFACE, LINE, CARD_RADIUS);
        ui.painter().rect_filled(
            Rect::from_min_max(
                Pos2::new(hero.left() + 1.0, hero.top() + 24.0),
                Pos2::new(hero.left() + 4.0, hero.top() + 82.0),
            ),
            2.0,
            status_color,
        );
        draw_text(
            ui.painter(),
            Pos2::new(hero.left() + 20.0, hero.top() + 14.0),
            "СТАТУС",
            UI_FONT_SIZE,
            ACCENT,
        );
        draw_text(
            ui.painter(),
            Pos2::new(hero.left() + 20.0, hero.top() + 35.0),
            title,
            TITLE_FONT_SIZE,
            status_color,
        );
        draw_text(
            ui.painter(),
            Pos2::new(hero.left() + 20.0, hero.top() + 70.0),
            detail,
            UI_FONT_SIZE,
            TEXT_SECONDARY,
        );

        let mascot = Rect::from_min_size(
            Pos2::new(hero.right() - 68.0, hero.top() + 17.0),
            Vec2::splat(48.0),
        );
        macos_mascot::draw(ui.painter(), mascot);
        draw_status_tiles(ui.painter(), runtime, hero);
    }

    fn draw_activation(&mut self, ui: &mut egui::Ui, activation: Rect, layout: Layout) {
        draw_card(ui.painter(), activation, SURFACE, LINE, CARD_RADIUS);
        draw_text(
            ui.painter(),
            Pos2::new(activation.left() + 18.0, activation.top() + 14.0),
            if self.current_installation {
                "Подключить Collector"
            } else {
                "Установить Collector"
            },
            SECTION_FONT_SIZE,
            TEXT,
        );
        draw_text(
            ui.painter(),
            Pos2::new(activation.left() + 18.0, activation.top() + 46.0),
            "Код активации",
            UI_FONT_SIZE,
            TEXT_MUTED,
        );
        draw_text(
            ui.painter(),
            Pos2::new(layout.device_field.left(), activation.top() + 46.0),
            "Устройство",
            UI_FONT_SIZE,
            TEXT_MUTED,
        );
        draw_input_background(ui.painter(), layout.token_field);
        draw_input_background(ui.painter(), layout.device_field);

        let token_edit = egui::TextEdit::singleline(&mut self.activation_token)
            .password(true)
            .frame(false)
            .hint_text("Одноразовый код")
            .text_color(TEXT);
        ui.put(layout.token_edit, token_edit);

        let device_edit = egui::TextEdit::singleline(&mut self.device_name)
            .frame(false)
            .text_color(TEXT);
        ui.put(layout.device_edit, device_edit);

        let button = ui.interact(
            layout.activate_button,
            Id::new("macos-activate"),
            Sense::click(),
        );
        draw_primary_button(
            ui.painter(),
            layout.activate_button,
            if self.provisioning {
                "Подключаем..."
            } else {
                "Активировать"
            },
            button.hovered(),
            self.provisioning,
        );

        let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));

        if !self.provisioning && (button.clicked() || enter_pressed) {
            self.begin_activation();
        }

        if let Some(error) = self.activation_error.as_deref() {
            draw_text(
                ui.painter(),
                Pos2::new(activation.left() + 18.0, activation.bottom() - 23.0),
                error,
                UI_FONT_SIZE,
                DANGER,
            );
        }
    }

    fn draw_journal(&mut self, ui: &mut egui::Ui, context: &egui::Context, layout: Layout) {
        let log_text = diagnostics::recent_text();
        let max_scroll = log_scroll_limit(&log_text, layout.logs_view);

        self.log_scroll_from_bottom = self.log_scroll_from_bottom.min(max_scroll);

        draw_card(ui.painter(), layout.logs_card, SURFACE, LINE, CARD_RADIUS);
        draw_text(
            ui.painter(),
            Pos2::new(
                layout.logs_card.left() + 16.0,
                layout.logs_card.top() + 13.0,
            ),
            "Журнал",
            SECTION_FONT_SIZE,
            TEXT,
        );

        let copy = ui.interact(layout.copy_logs, Id::new("macos-copy-logs"), Sense::click());
        draw_secondary_button(
            ui.painter(),
            layout.copy_logs,
            "Копировать всё",
            copy.hovered(),
        );

        if copy.clicked() {
            context.copy_text(log_text.clone());
            diagnostics::info("desktop", "Journal copied to clipboard as text");
        }

        let debug = ui.interact(
            layout.debug_toggle,
            Id::new("macos-debug-toggle"),
            Sense::click(),
        );
        draw_toggle(
            ui.painter(),
            layout.debug_toggle,
            "Диагностика",
            diagnostics::debug_enabled(),
            debug.hovered(),
        );

        if debug.clicked() {
            diagnostics::set_debug_enabled(!diagnostics::debug_enabled());
        }

        let log_response = ui.interact(
            layout.logs_view,
            Id::new("macos-log-view"),
            Sense::click_and_drag(),
        );

        if log_response.hovered() {
            let scroll_delta = ui.input(|input| input.raw_scroll_delta.y);

            if scroll_delta > 0.0 {
                self.log_scroll_from_bottom = (self.log_scroll_from_bottom + 3).min(max_scroll);
            } else if scroll_delta < 0.0 {
                self.log_scroll_from_bottom = self.log_scroll_from_bottom.saturating_sub(3);
            }
        }

        if log_response.clicked()
            && let Some(position) = log_response.interact_pointer_pos()
        {
            self.selected_log_entry = log_entry_at(
                &log_text,
                layout.logs_view,
                self.log_scroll_from_bottom,
                position,
            );
        }

        draw_card(ui.painter(), layout.logs_view, LOG_SURFACE, LINE, 18.0);
        draw_log_text(
            ui.painter(),
            layout.logs_view,
            &log_text,
            self.log_scroll_from_bottom,
            self.selected_log_entry,
        );
    }

    fn draw_version(&self, ui: &egui::Ui, layout: Layout) {
        ui.painter().text(
            Pos2::new(layout.logs_card.right(), layout.logs_card.bottom() + 3.0),
            Align2::RIGHT_TOP,
            format!("v{}", env!("CARGO_PKG_VERSION")),
            FontId::new(UI_FONT_SIZE, FontFamily::Proportional),
            TEXT_MUTED,
        );
    }
}

impl eframe::App for MacDesktop {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_activation(context);
        self.handle_external_close(context);
        context.request_repaint_after(REFRESH_INTERVAL);

        let runtime = diagnostics::runtime_snapshot();

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(0))
            .show(context, |ui| {
                ui.set_min_size(Vec2::new(WINDOW_WIDTH, WINDOW_HEIGHT));
                self.draw(ui, context, &runtime);
            });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BACKGROUND.to_normalized_gamma_f32()
    }
}

#[derive(Clone, Copy)]
struct Layout {
    window_minimize: Rect,
    window_close: Rect,
    hero: Rect,
    activation: Option<Rect>,
    token_field: Rect,
    token_edit: Rect,
    device_field: Rect,
    device_edit: Rect,
    activate_button: Rect,
    logs_card: Rect,
    logs_view: Rect,
    copy_logs: Rect,
    debug_toggle: Rect,
}

impl Layout {
    fn new(provisioned: bool) -> Self {
        let content_right = WINDOW_WIDTH - CONTENT_MARGIN;
        let window_close = Rect::from_min_max(
            Pos2::new(WINDOW_WIDTH - 56.0, 13.0),
            Pos2::new(WINDOW_WIDTH - 18.0, 47.0),
        );
        let window_minimize = Rect::from_min_max(
            Pos2::new(window_close.left() - 46.0, 13.0),
            Pos2::new(window_close.left() - 8.0, 47.0),
        );
        let hero = Rect::from_min_max(
            Pos2::new(CONTENT_MARGIN, HERO_TOP),
            Pos2::new(content_right, HERO_TOP + HERO_HEIGHT),
        );
        let activation = (!provisioned).then(|| {
            Rect::from_min_max(
                Pos2::new(CONTENT_MARGIN, hero.bottom() + SECTION_GAP),
                Pos2::new(
                    content_right,
                    hero.bottom() + SECTION_GAP + ACTIVATION_HEIGHT,
                ),
            )
        });
        let logs_top = activation.map_or(hero.bottom() + SECTION_GAP, |card| {
            card.bottom() + SECTION_GAP
        });
        let activation_rect = activation.unwrap_or(Rect::NOTHING);
        let edit_top = activation_rect.top() + 72.0;
        let inner_width = activation_rect.width() - 36.0;
        let device_width = 176.0;
        let button_width = 132.0;
        let gap = 10.0;
        let token_width = inner_width - device_width - button_width - gap * 2.0;
        let token_field = Rect::from_min_size(
            Pos2::new(activation_rect.left() + 18.0, edit_top),
            Vec2::new(token_width, 34.0),
        );
        let device_field = Rect::from_min_size(
            Pos2::new(token_field.right() + gap, edit_top),
            Vec2::new(device_width, 34.0),
        );
        let activate_button = Rect::from_min_max(
            Pos2::new(device_field.right() + gap, edit_top),
            Pos2::new(activation_rect.right() - 18.0, edit_top + 34.0),
        );
        let logs_card = Rect::from_min_max(
            Pos2::new(CONTENT_MARGIN, logs_top),
            Pos2::new(content_right, WINDOW_HEIGHT - CONTENT_MARGIN),
        );
        let debug_toggle = Rect::from_min_max(
            Pos2::new(logs_card.right() - 168.0, logs_card.top() + 13.0),
            Pos2::new(logs_card.right() - 14.0, logs_card.top() + 43.0),
        );
        let copy_logs = Rect::from_min_max(
            Pos2::new(debug_toggle.left() - 166.0, debug_toggle.top()),
            Pos2::new(debug_toggle.left() - 10.0, debug_toggle.bottom()),
        );
        let logs_view = Rect::from_min_max(
            Pos2::new(logs_card.left() + 14.0, logs_card.top() + LOG_HEADER_HEIGHT),
            Pos2::new(logs_card.right() - 14.0, logs_card.bottom() - 14.0),
        );

        Self {
            window_minimize,
            window_close,
            hero,
            activation,
            token_field,
            token_edit: token_field.shrink2(Vec2::new(10.0, 5.0)),
            device_field,
            device_edit: device_field.shrink2(Vec2::new(10.0, 5.0)),
            activate_button,
            logs_card,
            logs_view,
            copy_logs,
            debug_toggle,
        }
    }
}

struct LogVisualLine {
    text: String,
    color: Color32,
    entry_index: usize,
}

fn configure_style(context: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = LOG_SURFACE;
    visuals.faint_bg_color = SURFACE;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = ACCENT_DIM;
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    context.set_visuals(visuals);
    context.style_mut(|style| {
        style.spacing.item_spacing = Vec2::ZERO;
        style.spacing.button_padding = Vec2::ZERO;
        style.spacing.interact_size.y = 24.0;
        style.text_styles.insert(
            egui::TextStyle::Body,
            FontId::new(UI_FONT_SIZE, FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            FontId::new(UI_FONT_SIZE, FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            FontId::new(MONO_FONT_SIZE, FontFamily::Monospace),
        );
    });
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, height))
}

fn draw_card(painter: &egui::Painter, rect: Rect, fill: Color32, border: Color32, radius: f32) {
    painter.rect_filled(rect, radius, border);
    painter.rect_filled(rect.shrink(1.0), (radius - 1.0).max(0.0), fill);
}

fn draw_text(painter: &egui::Painter, position: Pos2, text: &str, size: f32, color: Color32) {
    painter.text(
        position,
        Align2::LEFT_TOP,
        text,
        FontId::new(size, FontFamily::Proportional),
        color,
    );
}

fn draw_window_button(
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    destructive: bool,
    hovered: bool,
) {
    let (fill, border, text) = if destructive && hovered {
        (DANGER, DANGER, BACKGROUND)
    } else if destructive {
        (DANGER_DIM, DANGER, DANGER)
    } else if hovered {
        (SURFACE_RAISED, LINE_STRONG, TEXT)
    } else {
        (SURFACE, LINE, TEXT_SECONDARY)
    };

    draw_card(painter, rect, fill, border, 14.0);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::new(UI_FONT_SIZE, FontFamily::Proportional),
        text,
    );
}

fn draw_status_tiles(painter: &egui::Painter, runtime: &RuntimeSnapshot, hero: Rect) {
    let left = hero.left() + 20.0;
    let right = hero.right() - 20.0;
    let available = right - left;
    let tile_width = (available - STATUS_TILE_GAP * 2.0) / 3.0;
    let bottom = hero.bottom() - 12.0;
    let top = bottom - STATUS_TILE_HEIGHT;
    let game = Rect::from_min_size(
        Pos2::new(left, top),
        Vec2::new(tile_width, STATUS_TILE_HEIGHT),
    );
    let mode = Rect::from_min_size(
        Pos2::new(game.right() + STATUS_TILE_GAP, top),
        Vec2::new(tile_width, STATUS_TILE_HEIGHT),
    );
    let mnemos = Rect::from_min_max(
        Pos2::new(mode.right() + STATUS_TILE_GAP, top),
        Pos2::new(right, bottom),
    );

    draw_status_tile(
        painter,
        game,
        "ИГРА",
        if runtime.cristalix_running {
            "Cristalix"
        } else {
            "Ожидание"
        },
        if runtime.cristalix_running {
            POSITIVE
        } else {
            TEXT_MUTED
        },
    );
    draw_status_tile(
        painter,
        mode,
        "РЕЖИМ",
        game_mode_label(runtime.game_mode.as_str()),
        if is_master_sword(runtime.game_mode.as_str()) {
            ACCENT
        } else {
            AMBER
        },
    );
    draw_status_tile(
        painter,
        mnemos,
        "MNEMOS",
        if runtime.realtime_connected {
            "Подключён"
        } else {
            "Нет связи"
        },
        if runtime.realtime_connected {
            POSITIVE
        } else {
            DANGER
        },
    );
}

fn draw_status_tile(
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    value: &str,
    status_color: Color32,
) {
    draw_card(painter, rect, SURFACE_RAISED, LINE, 18.0);
    painter.circle_filled(
        Pos2::new(rect.left() + 14.5, rect.top() + 13.5),
        3.5,
        status_color,
    );
    draw_text(
        painter,
        Pos2::new(rect.left() + 24.0, rect.top() + 3.0),
        label,
        UI_FONT_SIZE,
        TEXT_MUTED,
    );
    draw_text(
        painter,
        Pos2::new(rect.left() + 12.0, rect.top() + 23.0),
        value,
        UI_FONT_SIZE,
        TEXT,
    );
}

fn draw_input_background(painter: &egui::Painter, rect: Rect) {
    draw_card(painter, rect, SURFACE_RAISED, LINE_STRONG, rect.height());
}

fn draw_primary_button(
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    hovered: bool,
    disabled: bool,
) {
    let fill = if disabled { ACCENT_DIM } else { ACCENT };
    let border = if hovered && !disabled {
        TEXT_SECONDARY
    } else {
        ACCENT
    };
    let text = if disabled { TEXT_SECONDARY } else { BACKGROUND };

    draw_card(painter, rect, fill, border, rect.height());
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::new(UI_FONT_SIZE, FontFamily::Proportional),
        text,
    );
}

fn draw_secondary_button(painter: &egui::Painter, rect: Rect, label: &str, hovered: bool) {
    draw_card(
        painter,
        rect,
        if hovered { SURFACE_RAISED } else { SURFACE },
        if hovered { LINE_STRONG } else { LINE },
        rect.height(),
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::new(UI_FONT_SIZE, FontFamily::Proportional),
        if hovered { TEXT } else { TEXT_SECONDARY },
    );
}

fn draw_toggle(painter: &egui::Painter, rect: Rect, label: &str, enabled: bool, hovered: bool) {
    let border = if enabled {
        ACCENT
    } else if hovered {
        LINE_STRONG
    } else {
        LINE
    };
    let text = if enabled || hovered {
        TEXT
    } else {
        TEXT_SECONDARY
    };

    draw_card(
        painter,
        rect,
        if enabled { ACCENT_DIM } else { SURFACE_RAISED },
        border,
        rect.height(),
    );
    painter.circle_filled(
        Pos2::new(rect.left() + 13.5, rect.center().y),
        3.5,
        if enabled { ACCENT } else { TEXT_MUTED },
    );
    painter.text(
        Pos2::new(rect.left() + 25.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::new(UI_FONT_SIZE, FontFamily::Proportional),
        text,
    );
}

fn draw_log_text(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    scroll_from_bottom: usize,
    selected_entry: Option<usize>,
) {
    let text_rect = log_text_rect(rect);
    let max_chars = log_chars_per_line(text_rect);
    let lines = wrapped_log_lines(text, max_chars);
    let visible_count = log_visible_line_count(text_rect);
    let total = lines.len();
    let clamped_scroll = scroll_from_bottom.min(total.saturating_sub(visible_count));
    let end = total.saturating_sub(clamped_scroll);
    let start = end.saturating_sub(visible_count);
    let clipped = painter.with_clip_rect(text_rect);
    let mut y = text_rect.top();

    for line in &lines[start..end] {
        let line_rect = Rect::from_min_size(
            Pos2::new(text_rect.left(), y),
            Vec2::new(text_rect.width(), LOG_LINE_HEIGHT),
        );

        if selected_entry == Some(line.entry_index) {
            clipped.rect_filled(line_rect, 0.0, ACCENT_DIM);
        }

        clipped.text(
            line_rect.left_top(),
            Align2::LEFT_TOP,
            &line.text,
            FontId::new(MONO_FONT_SIZE, FontFamily::Monospace),
            line.color,
        );
        y += LOG_LINE_HEIGHT;
    }

    draw_log_scrollbar(painter, rect, total, visible_count, start);
}

fn draw_log_scrollbar(
    painter: &egui::Painter,
    rect: Rect,
    total_lines: usize,
    visible_lines: usize,
    start_line: usize,
) {
    if total_lines <= visible_lines || visible_lines == 0 {
        return;
    }

    let track = Rect::from_min_max(
        Pos2::new(rect.right() - 12.0, rect.top() + 10.0),
        Pos2::new(rect.right() - 6.0, rect.bottom() - 10.0),
    );
    let track_height = track.height().max(1.0);
    let thumb_height =
        (track_height * visible_lines as f32 / total_lines as f32).clamp(24.0, track_height);
    let max_start = total_lines.saturating_sub(visible_lines).max(1);
    let travel = (track_height - thumb_height).max(0.0);
    let thumb_top = track.top() + travel * start_line as f32 / max_start as f32;
    let thumb = Rect::from_min_max(
        Pos2::new(rect.right() - 14.0, thumb_top),
        Pos2::new(rect.right() - 4.0, thumb_top + thumb_height),
    );

    painter.rect_filled(track, 3.0, LINE);
    painter.rect_filled(thumb, 5.0, TEXT_MUTED);
}

fn log_scroll_limit(text: &str, rect: Rect) -> usize {
    let text_rect = log_text_rect(rect);
    let max_chars = log_chars_per_line(text_rect);
    let total_lines = wrapped_log_line_count(text, max_chars);
    let visible_lines = log_visible_line_count(text_rect);

    total_lines.saturating_sub(visible_lines)
}

fn log_entry_at(
    text: &str,
    rect: Rect,
    scroll_from_bottom: usize,
    position: Pos2,
) -> Option<usize> {
    if !rect.contains(position) {
        return None;
    }

    let text_rect = log_text_rect(rect);

    if !text_rect.contains(position) {
        return None;
    }

    let max_chars = log_chars_per_line(text_rect);
    let lines = wrapped_log_lines(text, max_chars);
    let visible_count = log_visible_line_count(text_rect);
    let total = lines.len();
    let clamped_scroll = scroll_from_bottom.min(total.saturating_sub(visible_count));
    let end = total.saturating_sub(clamped_scroll);
    let start = end.saturating_sub(visible_count);
    let row = ((position.y - text_rect.top()) / LOG_LINE_HEIGHT) as usize;
    let visual_index = start + row;

    if visual_index >= end {
        return None;
    }

    lines.get(visual_index).map(|line| line.entry_index)
}

fn log_text_rect(rect: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.left() + 12.0, rect.top() + 10.0),
        Pos2::new(rect.right() - 22.0, rect.bottom() - 10.0),
    )
}

fn wrapped_log_lines(text: &str, max_chars: usize) -> Vec<LogVisualLine> {
    let mut output = Vec::new();

    for (entry_index, logical_line) in text.lines().enumerate() {
        let color = log_line_color(logical_line);
        let chars = logical_line.chars().collect::<Vec<_>>();

        if chars.is_empty() {
            output.push(LogVisualLine {
                text: String::new(),
                color,
                entry_index,
            });
            continue;
        }

        for chunk in chars.chunks(max_chars.max(1)) {
            output.push(LogVisualLine {
                text: chunk.iter().collect(),
                color,
                entry_index,
            });
        }
    }

    output
}

fn wrapped_log_line_count(text: &str, max_chars: usize) -> usize {
    text.lines()
        .map(|line| {
            let count = line.chars().count().max(1);
            count.div_ceil(max_chars.max(1))
        })
        .sum()
}

fn log_chars_per_line(rect: Rect) -> usize {
    ((rect.width().max(LOG_CHAR_WIDTH) / LOG_CHAR_WIDTH) as usize).max(20)
}

fn log_visible_line_count(rect: Rect) -> usize {
    ((rect.height().max(LOG_LINE_HEIGHT) / LOG_LINE_HEIGHT) as usize).max(1)
}

fn status_copy(runtime: &RuntimeSnapshot) -> (&'static str, &'static str, Color32) {
    if runtime.observing {
        return (
            "Сбор активен",
            "Master Sword распознан. Новые события отправляются в Mnemos.",
            ACCENT,
        );
    }

    if is_master_sword(runtime.game_mode.as_str()) && !runtime.cristalix_running {
        return (
            "Master Sword найден",
            "Ждём свежую активность в логе, чтобы подтвердить текущую сессию.",
            AMBER,
        );
    }

    if !runtime.cristalix_running {
        return (
            "Ожидаем Cristalix",
            "Collector готов и сам подхватит игру после появления свежего лога.",
            TEXT,
        );
    }

    if is_master_sword(runtime.game_mode.as_str()) && !runtime.realtime_connected {
        return (
            "Подключаем Mnemos",
            "Master Sword активен. Восстанавливаем соединение.",
            AMBER,
        );
    }

    if is_master_sword(runtime.game_mode.as_str()) {
        return (
            "Подтверждаем сбор",
            "Сессия активна. Завершаем подключение Collector.",
            AMBER,
        );
    }

    ("Cristalix активен", "Ожидаем переход в Master Sword.", TEXT)
}

fn game_mode_label(mode: &str) -> &str {
    if is_master_sword(mode) {
        "Master Sword"
    } else if mode.eq_ignore_ascii_case("unknown") || mode.trim().is_empty() {
        "Не определён"
    } else {
        mode
    }
}

fn is_master_sword(mode: &str) -> bool {
    mode.eq_ignore_ascii_case("MasterSword") || mode.eq_ignore_ascii_case("Master Sword")
}

fn log_line_color(line: &str) -> Color32 {
    if line.contains("[ERROR]") {
        DANGER
    } else if line.contains("[WARN ") || line.contains("[WARN]") {
        AMBER
    } else if line.contains("[DEBUG]") {
        TEXT_MUTED
    } else {
        TEXT_SECONDARY
    }
}

async fn provision_current_installation(token: &str, device_name: &str) -> Result<String> {
    ProvisioningClient::new()?
        .provision(token, device_name)
        .await
        .context("не удалось активировать Collector")?;

    Autostart::ensure_enabled().context("не удалось включить автозапуск Collector")?;

    CredentialStore
        .load()?
        .context("активация завершилась без сохранённого credential")
}

async fn provision_and_install(token: &str, device_name: &str) -> Result<()> {
    ProvisioningClient::new()?
        .provision(token, device_name)
        .await
        .context("не удалось активировать Collector")?;

    tokio::task::spawn_blocking(Installation::install_current_executable)
        .await
        .context("задача установки Collector завершилась аварийно")??;

    Ok(())
}

fn spawn_collector(runtime: Handle, access_key: String) {
    runtime.spawn(async move {
        diagnostics::clear_last_error();
        diagnostics::info("runtime", "Collector worker starting");

        let result = async {
            let application = CollectorApplication::new(access_key).await?;
            application.run().await
        }
        .await;

        match result {
            Ok(()) => diagnostics::info("runtime", "Collector worker stopped cleanly"),
            Err(error) => {
                diagnostics::error("runtime", format!("Collector worker failed: {error:#}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_layout_matches_windows_geometry_when_provisioned() {
        let layout = Layout::new(true);

        assert_eq!(layout.hero, rect(22.0, 74.0, 1036.0, 154.0));
        assert!(layout.activation.is_none());
        assert_eq!(layout.logs_card.top(), 244.0);
        assert_eq!(layout.logs_card.bottom(), 698.0);
    }

    #[test]
    fn macos_layout_matches_windows_geometry_before_activation() {
        let layout = Layout::new(false);
        let activation = layout.activation.unwrap();

        assert_eq!(activation.top(), 244.0);
        assert_eq!(activation.bottom(), 378.0);
        assert_eq!(layout.logs_card.top(), 394.0);
        assert_eq!(layout.logs_card.bottom(), 698.0);
        assert_eq!(layout.token_field.height(), 34.0);
        assert_eq!(layout.device_field.width(), 176.0);
    }

    #[test]
    fn macos_font_metrics_match_windows_font_contract() {
        assert_eq!(UI_FONT_SIZE, 16.0);
        assert_eq!(TITLE_FONT_SIZE, 29.0);
        assert_eq!(SECTION_FONT_SIZE, 21.0);
        assert_eq!(MONO_FONT_SIZE, 14.0);
    }

    #[test]
    fn collector_icon_uses_shared_mascot_source() {
        let icon = macos_mascot::icon(32);

        assert_eq!(icon.width, 32);
        assert_eq!(icon.height, 32);
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
    }

    #[test]
    fn master_sword_label_accepts_runtime_and_human_forms() {
        assert!(is_master_sword("MasterSword"));
        assert!(is_master_sword("Master Sword"));
        assert!(!is_master_sword("Unknown"));
    }
}
