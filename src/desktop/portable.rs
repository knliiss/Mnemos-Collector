use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};
use tokio::runtime::Handle;
use zeroize::Zeroizing;

use crate::application::CollectorApplication;
#[cfg(target_os = "macos")]
use crate::cristalix::set_configured_latest_log_path;
use crate::cristalix::{clear_configured_latest_log_path, configured_latest_log_path};
use crate::diagnostics::{self, RuntimeSnapshot};
use crate::platform::{Autostart, Installation};
use crate::provisioning::{ProvisioningClient, default_device_name};
use crate::security::CredentialStore;

use super::DesktopLaunchContext;
#[cfg(target_os = "macos")]
use super::macos_native::{self, MacStatusItem};

const WINDOW_WIDTH: f32 = 1080.0;
const WINDOW_HEIGHT: f32 = 720.0;
const REFRESH_INTERVAL: Duration = Duration::from_millis(500);

const CONTENT_MARGIN: f32 = 22.0;
const HEADER_HEIGHT: f32 = 68.0;
const HERO_TOP: f32 = HEADER_HEIGHT + 6.0;
const HERO_HEIGHT: f32 = 154.0;
const CARD_RADIUS: u8 = 24;
const LOG_ACTION_HEIGHT: f32 = 30.0;
const DEBUG_TOGGLE_WIDTH: f32 = 154.0;
const COPY_LOGS_WIDTH: f32 = 156.0;
const UPDATE_BUTTON_WIDTH: f32 = 184.0;
const LOG_ACTION_GAP: f32 = 10.0;
const STATUS_TILE_HEIGHT: f32 = 46.0;
const STATUS_TILE_BOTTOM_MARGIN: f32 = 12.0;
const STATUS_LABEL_TOP_PADDING: f32 = 3.0;
const STATUS_LABEL_HEIGHT: f32 = 20.0;
const STATUS_VALUE_BOTTOM_PADDING: f32 = 2.0;
const LOG_SOURCE_HEIGHT: f32 = 64.0;
const LOG_LINE_HEIGHT: f32 = 18.0;

const UI_FONT_SIZE: f32 = 16.0;
const TITLE_FONT_SIZE: f32 = 29.0;
const SECTION_FONT_SIZE: f32 = 21.0;
const MONO_FONT_SIZE: f32 = 14.0;

const BACKGROUND: Color32 = Color32::from_rgb(0x02, 0x03, 0x02);
const LOG_SURFACE: Color32 = Color32::from_rgb(0x0b, 0x0c, 0x09);
const SURFACE: Color32 = Color32::from_rgb(0x15, 0x16, 0x12);
const SURFACE_RAISED: Color32 = Color32::from_rgb(0x1f, 0x20, 0x1a);
const ACCENT_DIM: Color32 = Color32::from_rgb(0x26, 0x31, 0x0d);
const LINE: Color32 = Color32::from_rgb(0x35, 0x38, 0x31);
const LINE_STRONG: Color32 = Color32::from_rgb(0x4a, 0x4e, 0x44);
const TEXT: Color32 = Color32::from_rgb(0xf5, 0xf6, 0xef);
const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xc2, 0xc4, 0xb8);
const TEXT_MUTED: Color32 = Color32::from_rgb(0x7c, 0x80, 0x72);
const ACCENT: Color32 = Color32::from_rgb(0xcb, 0xff, 0x2d);
const POSITIVE: Color32 = Color32::from_rgb(0xbd, 0xe0, 0x6d);
const WARNING: Color32 = Color32::from_rgb(0xff, 0xb3, 0x4f);
const DANGER_DIM: Color32 = Color32::from_rgb(0x31, 0x16, 0x18);
const DANGER: Color32 = Color32::from_rgb(0xff, 0x68, 0x73);

pub fn run(context: DesktopLaunchContext, runtime: Handle) -> Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Mnemos Collector")
        .with_app_id("rest.knalis.mnemos-collector")
        .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
        .with_min_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
        .with_max_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_icon(portable_icon());

    #[cfg(target_os = "macos")]
    let viewport = viewport.with_decorations(false);

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

            Ok(Box::new(PortableDesktop::new(context, runtime)))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

pub fn show_fatal_error(message: &str) {
    #[cfg(target_os = "macos")]
    {
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
    }

    #[cfg(target_os = "linux")]
    {
        if Command::new("zenity")
            .arg("--error")
            .arg("--title=Mnemos Collector")
            .arg(format!("--text={message}"))
            .status()
            .is_ok_and(|status| status.success())
        {
            return;
        }

        if Command::new("kdialog")
            .arg("--error")
            .arg(message)
            .arg("--title")
            .arg("Mnemos Collector")
            .status()
            .is_ok_and(|status| status.success())
        {
            return;
        }
    }

    eprintln!("Mnemos Collector: {message}");
}

enum ActivationOutcome {
    CurrentInstallation(String),
    InstalledAndLaunched,
}

#[derive(Debug, Clone, Copy)]
struct PortableLayout {
    #[cfg(target_os = "macos")]
    title_bar: Rect,
    #[cfg(target_os = "macos")]
    window_minimize: Rect,
    #[cfg(target_os = "macos")]
    window_close: Rect,
    hero: Rect,
    activation: Option<Rect>,
    token_field: Rect,
    token_edit: Rect,
    device_field: Rect,
    device_edit: Rect,
    activate_button: Rect,
    log_source: Option<Rect>,
    logs_card: Rect,
    logs_view: Rect,
    copy_logs: Rect,
    debug_toggle: Rect,
    update_button: Rect,
    diagnostics_summary: Rect,
}

impl PortableLayout {
    fn new(provisioned: bool, log_source_recovery: bool, update_available: bool) -> Self {
        let content_right = WINDOW_WIDTH - CONTENT_MARGIN;
        #[cfg(target_os = "macos")]
        let title_bar = Rect::from_min_max(
            egui::pos2(0.0, 0.0),
            egui::pos2(WINDOW_WIDTH, HEADER_HEIGHT),
        );
        #[cfg(target_os = "macos")]
        let window_close = Rect::from_min_max(
            egui::pos2(WINDOW_WIDTH - 56.0, 13.0),
            egui::pos2(WINDOW_WIDTH - 18.0, 47.0),
        );
        #[cfg(target_os = "macos")]
        let window_minimize = Rect::from_min_max(
            egui::pos2(window_close.left() - 46.0, 13.0),
            egui::pos2(window_close.left() - 8.0, 47.0),
        );
        let hero = Rect::from_min_max(
            egui::pos2(CONTENT_MARGIN, HERO_TOP),
            egui::pos2(content_right, HERO_TOP + HERO_HEIGHT),
        );

        let (activation, mut next_top) = if provisioned {
            (None, hero.bottom() + 16.0)
        } else {
            let activation = Rect::from_min_max(
                egui::pos2(CONTENT_MARGIN, hero.bottom() + 16.0),
                egui::pos2(content_right, hero.bottom() + 150.0),
            );

            (Some(activation), activation.bottom() + 16.0)
        };

        let activation_rect = activation.unwrap_or(Rect::from_min_max(
            egui::pos2(CONTENT_MARGIN, 0.0),
            egui::pos2(content_right, 0.0),
        ));
        let edit_top = activation_rect.top() + 72.0;
        let inner_width = (activation_rect.width() - 36.0).max(540.0);
        let device_width = 176.0;
        let button_width = 132.0;
        let gap = 10.0;
        let token_width = (inner_width - device_width - button_width - gap * 2.0).max(210.0);
        let token_field = Rect::from_min_max(
            egui::pos2(activation_rect.left() + 18.0, edit_top),
            egui::pos2(activation_rect.left() + 18.0 + token_width, edit_top + 34.0),
        );
        let token_edit = token_field.shrink2(egui::vec2(10.0, 5.0));
        let device_field = Rect::from_min_max(
            egui::pos2(token_field.right() + gap, edit_top),
            egui::pos2(token_field.right() + gap + device_width, edit_top + 34.0),
        );
        let device_edit = device_field.shrink2(egui::vec2(10.0, 5.0));
        let activate_button = Rect::from_min_max(
            egui::pos2(device_field.right() + gap, edit_top),
            egui::pos2(activation_rect.right() - 18.0, edit_top + 34.0),
        );

        let log_source = if log_source_recovery {
            let rect = Rect::from_min_max(
                egui::pos2(CONTENT_MARGIN, next_top),
                egui::pos2(content_right, next_top + LOG_SOURCE_HEIGHT),
            );
            next_top = rect.bottom() + 16.0;
            Some(rect)
        } else {
            None
        };

        let logs_card = Rect::from_min_max(
            egui::pos2(CONTENT_MARGIN, next_top),
            egui::pos2(content_right, WINDOW_HEIGHT - CONTENT_MARGIN),
        );
        let debug_toggle = Rect::from_min_max(
            egui::pos2(
                logs_card.right() - 14.0 - DEBUG_TOGGLE_WIDTH,
                logs_card.top() + 13.0,
            ),
            egui::pos2(
                logs_card.right() - 14.0,
                logs_card.top() + 13.0 + LOG_ACTION_HEIGHT,
            ),
        );
        let copy_logs = Rect::from_min_max(
            egui::pos2(
                debug_toggle.left() - LOG_ACTION_GAP - COPY_LOGS_WIDTH,
                debug_toggle.top(),
            ),
            egui::pos2(debug_toggle.left() - LOG_ACTION_GAP, debug_toggle.bottom()),
        );
        let update_button = Rect::from_min_max(
            egui::pos2(
                copy_logs.left() - LOG_ACTION_GAP - UPDATE_BUTTON_WIDTH,
                copy_logs.top(),
            ),
            egui::pos2(copy_logs.left() - LOG_ACTION_GAP, copy_logs.bottom()),
        );
        let diagnostics_right = if update_available {
            update_button.left() - 8.0
        } else {
            copy_logs.left() - 8.0
        };
        let diagnostics_summary = Rect::from_min_max(
            egui::pos2(logs_card.left() + 178.0, copy_logs.top()),
            egui::pos2(diagnostics_right, copy_logs.bottom()),
        );
        let logs_view = Rect::from_min_max(
            egui::pos2(logs_card.left() + 14.0, logs_card.top() + 52.0),
            egui::pos2(logs_card.right() - 14.0, logs_card.bottom() - 14.0),
        );

        Self {
            #[cfg(target_os = "macos")]
            title_bar,
            #[cfg(target_os = "macos")]
            window_minimize,
            #[cfg(target_os = "macos")]
            window_close,
            hero,
            activation,
            token_field,
            token_edit,
            device_field,
            device_edit,
            activate_button,
            log_source,
            logs_card,
            logs_view,
            copy_logs,
            debug_toggle,
            update_button,
            diagnostics_summary,
        }
    }
}

struct PortableDesktop {
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
    log_source_error: Option<String>,
    selected_log_entry: Option<String>,
    #[cfg(target_os = "macos")]
    _status_item: Option<MacStatusItem>,
}

impl PortableDesktop {
    fn new(context: DesktopLaunchContext, runtime: Handle) -> Self {
        #[cfg(target_os = "macos")]
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
            log_source_error: None,
            selected_log_entry: None,
            #[cfg(target_os = "macos")]
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
            diagnostics::info("provisioning", "Activation started from desktop UI");

            let result = if current_installation {
                provision_current_installation(token.as_str(), &device_name)
                    .await
                    .map(ActivationOutcome::CurrentInstallation)
            } else {
                install_from_ui(token.as_str(), &device_name)
                    .await
                    .map(|()| ActivationOutcome::InstalledAndLaunched)
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
            Ok(ActivationOutcome::InstalledAndLaunched) => {
                diagnostics::info(
                    "provisioning",
                    "Activation handed off to the stable Collector installation",
                );
                self.exit_requested = true;
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(message) => {
                diagnostics::error("provisioning", message.clone());
                self.activation_error = Some(message);
            }
        }
    }

    fn handle_window_close(&self, context: &egui::Context) {
        #[cfg(target_os = "macos")]
        {
            let close_requested = context.input(|input| input.viewport().close_requested());

            if close_requested && !self.exit_requested {
                context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                macos_native::hide_application();
            }
        }

        #[cfg(not(target_os = "macos"))]
        let _ = context;
    }

    fn copy_selected_log(&self, context: &egui::Context) {
        if let Some(entry) = self.selected_log_entry.as_ref() {
            context.copy_text(entry.clone());
            diagnostics::info("desktop", "Selected log entry copied to clipboard as text");
        }
    }

    fn draw_header(&self, ui: &mut egui::Ui, _context: &egui::Context, _layout: PortableLayout) {
        let icon = Rect::from_min_max(egui::pos2(22.0, 12.0), egui::pos2(66.0, 56.0));
        paint_card(ui, icon, 18, SURFACE, LINE);
        paint_mascot(ui, icon.shrink(2.0));

        paint_text(
            ui,
            egui::pos2(78.0, 14.0),
            Align2::LEFT_TOP,
            "MNEMOS",
            11.0,
            ACCENT,
        );
        paint_text(
            ui,
            egui::pos2(78.0, 34.0),
            Align2::LEFT_TOP,
            "Collector",
            22.0,
            TEXT,
        );

        #[cfg(target_os = "macos")]
        {
            let drag_rect = Rect::from_min_max(
                _layout.title_bar.min,
                egui::pos2(
                    _layout.window_minimize.left() - 8.0,
                    _layout.title_bar.bottom(),
                ),
            );
            let drag = ui.interact(drag_rect, ui.id().with("mnemos-window-drag"), Sense::drag());

            if drag.drag_started() {
                _context.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            if window_button(ui, _layout.window_minimize, "—", false).clicked() {
                _context.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }

            if window_button(ui, _layout.window_close, "×", true).clicked() {
                macos_native::hide_application();
            }
        }
    }

    fn draw_hero(&self, ui: &mut egui::Ui, runtime: &RuntimeSnapshot, layout: PortableLayout) {
        let (title, detail, status_color) = status_copy(self.provisioned, runtime);
        let rect = layout.hero;

        paint_card(ui, rect, CARD_RADIUS, SURFACE, LINE);

        let accent_rect = Rect::from_min_max(
            egui::pos2(rect.left() + 1.0, rect.top() + 24.0),
            egui::pos2(rect.left() + 4.0, rect.top() + 82.0),
        );
        paint_pill(ui, accent_rect, status_color, status_color);

        paint_text(
            ui,
            egui::pos2(rect.left() + 20.0, rect.top() + 14.0),
            Align2::LEFT_TOP,
            "СТАТУС",
            11.0,
            ACCENT,
        );
        paint_text(
            ui,
            egui::pos2(rect.left() + 20.0, rect.top() + 35.0),
            Align2::LEFT_TOP,
            title,
            27.0,
            status_color,
        );
        paint_text(
            ui,
            egui::pos2(rect.left() + 20.0, rect.top() + 70.0),
            Align2::LEFT_TOP,
            detail,
            13.0,
            TEXT_SECONDARY,
        );

        let mascot = Rect::from_min_size(
            egui::pos2(rect.right() - 68.0, rect.top() + 17.0),
            egui::vec2(48.0, 48.0),
        );
        paint_mascot(ui, mascot);
        self.draw_status_tiles(ui, runtime, rect);
    }

    fn draw_status_tiles(&self, ui: &mut egui::Ui, runtime: &RuntimeSnapshot, hero: Rect) {
        let gap = 8.0;
        let left = hero.left() + 20.0;
        let right = hero.right() - 20.0;
        let available = right - left;
        let tile_width = (available - gap * 2.0) / 3.0;
        let bottom = hero.bottom() - STATUS_TILE_BOTTOM_MARGIN;
        let top = bottom - STATUS_TILE_HEIGHT;

        let game = Rect::from_min_max(egui::pos2(left, top), egui::pos2(left + tile_width, bottom));
        let mode = Rect::from_min_max(
            egui::pos2(game.right() + gap, top),
            egui::pos2(game.right() + gap + tile_width, bottom),
        );
        let mnemos = Rect::from_min_max(
            egui::pos2(mode.right() + gap, top),
            egui::pos2(right, bottom),
        );

        draw_status_tile(
            ui,
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
            ui,
            mode,
            "РЕЖИМ",
            game_mode_label(runtime.game_mode.as_str()),
            if is_master_sword(runtime.game_mode.as_str()) {
                ACCENT
            } else {
                WARNING
            },
        );
        draw_status_tile(
            ui,
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

    fn draw_activation(&mut self, ui: &mut egui::Ui, layout: PortableLayout) {
        let Some(activation) = layout.activation else {
            return;
        };

        paint_card(ui, activation, CARD_RADIUS, SURFACE, LINE);
        paint_text(
            ui,
            egui::pos2(activation.left() + 18.0, activation.top() + 14.0),
            Align2::LEFT_TOP,
            if self.current_installation {
                "Подключить Collector"
            } else {
                "Установить Collector"
            },
            19.0,
            TEXT,
        );
        paint_text(
            ui,
            egui::pos2(activation.left() + 18.0, activation.top() + 46.0),
            Align2::LEFT_TOP,
            "Код активации",
            11.0,
            TEXT_MUTED,
        );
        paint_text(
            ui,
            egui::pos2(layout.device_field.left(), activation.top() + 46.0),
            Align2::LEFT_TOP,
            "Устройство",
            11.0,
            TEXT_MUTED,
        );

        paint_pill(ui, layout.token_field, SURFACE_RAISED, LINE_STRONG);
        paint_pill(ui, layout.device_field, SURFACE_RAISED, LINE_STRONG);

        let token_edit = egui::TextEdit::singleline(&mut self.activation_token)
            .password(true)
            .frame(false)
            .font(FontId::proportional(UI_FONT_SIZE))
            .text_color(TEXT)
            .hint_text("Одноразовый код");
        ui.put(layout.token_edit, token_edit);

        let device_edit = egui::TextEdit::singleline(&mut self.device_name)
            .frame(false)
            .font(FontId::proportional(UI_FONT_SIZE))
            .text_color(TEXT);
        ui.put(layout.device_edit, device_edit);

        let label = if self.provisioning {
            "Подключаем..."
        } else {
            "Активировать"
        };
        let button = primary_button_widget(label, self.provisioning);
        let response = ui.add_enabled_ui(!self.provisioning, |ui| {
            ui.put(layout.activate_button, button)
        });

        if response.inner.clicked() {
            self.begin_activation();
        }

        if let Some(error) = self.activation_error.as_deref() {
            paint_text(
                ui,
                egui::pos2(activation.left() + 18.0, activation.bottom() - 23.0),
                Align2::LEFT_TOP,
                error,
                11.0,
                DANGER,
            );
        }
    }

    fn should_draw_log_source(&self, runtime: &RuntimeSnapshot) -> bool {
        log_source_recovery_needed(
            configured_latest_log_path().is_some(),
            runtime.log_path.is_some(),
            self.log_source_error.is_some(),
        )
    }

    fn draw_log_source(&mut self, ui: &mut egui::Ui, runtime: &RuntimeSnapshot, rect: Rect) {
        let configured_path = configured_latest_log_path();
        let active_path = runtime.log_path.as_deref();
        let source_text = if let Some(path) = configured_path.as_deref() {
            format!("Ручной источник: {}", shortened_path(path))
        } else if let Some(path) = active_path {
            format!("Авто: {}", shortened_path(path))
        } else {
            "Автопоиск: лог пока не найден".to_owned()
        };

        paint_card(ui, rect, 18, SURFACE, LINE);
        paint_text(
            ui,
            egui::pos2(rect.left() + 16.0, rect.top() + 10.0),
            Align2::LEFT_TOP,
            "ЛОГ CRISTALIX",
            11.0,
            ACCENT,
        );
        paint_text(
            ui,
            egui::pos2(rect.left() + 16.0, rect.top() + 31.0),
            Align2::LEFT_TOP,
            &source_text,
            13.0,
            TEXT_SECONDARY,
        );

        let right = rect.right() - 14.0;
        #[cfg(target_os = "macos")]
        let right = {
            let choose = Rect::from_min_max(
                egui::pos2(right - 124.0, rect.top() + 17.0),
                egui::pos2(right, rect.top() + 47.0),
            );

            if ui
                .put(choose, secondary_button_widget("Выбрать файл…"))
                .clicked()
            {
                self.select_macos_log_file();
            }

            choose.left() - 10.0
        };

        if configured_path.is_some() {
            let auto = Rect::from_min_max(
                egui::pos2(right - 104.0, rect.top() + 17.0),
                egui::pos2(right, rect.top() + 47.0),
            );

            if ui.put(auto, secondary_button_widget("Автопоиск")).clicked() {
                match clear_configured_latest_log_path() {
                    Ok(()) => {
                        self.log_source_error = None;
                        diagnostics::info(
                            "cristalix",
                            "Manual Cristalix log source cleared; automatic discovery enabled",
                        );
                    }
                    Err(error) => {
                        self.log_source_error =
                            Some(format!("Не удалось вернуть автопоиск: {error}"));
                    }
                }
            }
        }

        if let Some(error) = self.log_source_error.as_deref() {
            paint_text(
                ui,
                egui::pos2(rect.left() + 420.0, rect.top() + 31.0),
                Align2::LEFT_TOP,
                error,
                11.0,
                DANGER,
            );
        }
    }

    #[cfg(target_os = "macos")]
    fn select_macos_log_file(&mut self) {
        match macos_native::pick_log_file() {
            Ok(Some(path)) => match set_configured_latest_log_path(&path) {
                Ok(()) => {
                    self.log_source_error = None;
                    diagnostics::info(
                        "cristalix",
                        format!("Manual Cristalix log source selected: {}", path.display()),
                    );
                }
                Err(error) => {
                    self.log_source_error =
                        Some(format!("Не удалось сохранить выбранный лог: {error}"));
                }
            },
            Ok(None) => {}
            Err(error) => {
                self.log_source_error = Some(format!("Не удалось открыть выбор файла: {error:#}"));
            }
        }
    }

    fn draw_journal(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        runtime: &RuntimeSnapshot,
        layout: PortableLayout,
    ) {
        paint_card(ui, layout.logs_card, CARD_RADIUS, SURFACE, LINE);
        paint_text(
            ui,
            egui::pos2(
                layout.logs_card.left() + 16.0,
                layout.logs_card.top() + 13.0,
            ),
            Align2::LEFT_TOP,
            "Журнал",
            19.0,
            TEXT,
        );

        let summary = diagnostics_summary(runtime);
        paint_text_clipped(
            ui,
            layout.diagnostics_summary,
            &summary,
            11.0,
            if runtime.required_update_version.is_some() {
                DANGER
            } else if runtime.spool_capacity > 0
                && runtime.spool_pending.saturating_mul(10)
                    >= runtime.spool_capacity.saturating_mul(9)
            {
                WARNING
            } else {
                TEXT_MUTED
            },
        );

        if let Some(version) = runtime.available_update_version.as_deref() {
            let busy = runtime.update_installing || runtime.update_waiting_for_slot;
            let label = if runtime.update_installing {
                "УСТАНОВКА...".to_owned()
            } else if runtime.update_waiting_for_slot {
                "ОЖИДАНИЕ СЛОТА...".to_owned()
            } else {
                format!("ОБНОВИТЬ ДО v{version}")
            };

            let response = ui.add_enabled_ui(!busy, |ui| {
                ui.put(layout.update_button, update_button_widget(&label, busy))
            });

            if response.inner.clicked() && !busy {
                diagnostics::request_update_install();
            }
        }

        if ui
            .put(layout.copy_logs, secondary_button_widget("Копировать всё"))
            .clicked()
        {
            context.copy_text(diagnostics::recent_text());
            diagnostics::info("desktop", "Journal copied to clipboard as text");
        }

        if ui
            .put(
                layout.debug_toggle,
                toggle_button_widget("Диагностика", diagnostics::debug_enabled()),
            )
            .clicked()
        {
            diagnostics::set_debug_enabled(!diagnostics::debug_enabled());
        }

        paint_card(ui, layout.logs_view, 18, LOG_SURFACE, LINE);
        let log_text = diagnostics::recent_text();

        if let Some(selected) = self.selected_log_entry.as_ref()
            && !log_text.lines().any(|line| line == selected)
        {
            self.selected_log_entry = None;
        }

        let log_content = Rect::from_min_max(
            egui::pos2(
                layout.logs_view.left() + 12.0,
                layout.logs_view.top() + 10.0,
            ),
            egui::pos2(
                layout.logs_view.right() - 22.0,
                layout.logs_view.bottom() - 10.0,
            ),
        );
        let scroll_output = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(log_content)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    ui.set_clip_rect(log_content);
                    ui.spacing_mut().item_spacing.y = 0.0;

                    egui::ScrollArea::vertical()
                        .id_salt("mnemos-journal-scroll")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_min_width(log_content.width());

                            if log_text.is_empty() {
                                ui.add_sized(
                                    [ui.available_width(), LOG_LINE_HEIGHT],
                                    journal_line_widget("Журнал пока пуст.", TEXT_MUTED, false),
                                );
                            }

                            for line in log_text.lines() {
                                let selected = self.selected_log_entry.as_deref() == Some(line);
                                let response = ui.add_sized(
                                    [ui.available_width(), LOG_LINE_HEIGHT],
                                    journal_line_widget(line, log_line_color(line), selected),
                                );

                                if response.clicked() {
                                    self.selected_log_entry = Some(line.to_owned());
                                }
                            }
                        })
                },
            )
            .inner;

        paint_log_scrollbar(
            ui,
            layout.logs_view,
            scroll_output.content_size.y,
            scroll_output.inner_rect.height(),
            scroll_output.state.offset.y,
        );

        let copy_pressed =
            context.input(|input| input.modifiers.command && input.key_pressed(egui::Key::C));

        if copy_pressed {
            self.copy_selected_log(context);
        }
    }

    fn draw_footer(&self, ui: &mut egui::Ui) {
        paint_text(
            ui,
            egui::pos2(WINDOW_WIDTH - CONTENT_MARGIN, WINDOW_HEIGHT - 8.0),
            Align2::RIGHT_BOTTOM,
            &format!("v{}", env!("CARGO_PKG_VERSION")),
            12.0,
            TEXT_MUTED,
        );
    }
}

impl eframe::App for PortableDesktop {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_activation(context);
        self.handle_window_close(context);
        context.request_repaint_after(REFRESH_INTERVAL);

        let runtime = diagnostics::runtime_snapshot();
        let show_log_source = self.should_draw_log_source(&runtime);
        let layout = PortableLayout::new(
            self.provisioned,
            show_log_source,
            runtime.available_update_version.is_some(),
        );

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(0))
            .show(context, |ui| {
                ui.set_min_size(egui::vec2(WINDOW_WIDTH, WINDOW_HEIGHT));
                self.draw_header(ui, context, layout);
                self.draw_hero(ui, &runtime, layout);
                self.draw_activation(ui, layout);

                if let Some(log_source) = layout.log_source {
                    self.draw_log_source(ui, &runtime, log_source);
                }

                self.draw_journal(ui, context, &runtime, layout);
                self.draw_footer(ui);
            });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BACKGROUND.to_normalized_gamma_f32()
    }
}

fn configure_style(context: &egui::Context) {
    configure_platform_fonts(context);

    let mut visuals = egui::Visuals::dark();

    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = LOG_SURFACE;
    visuals.faint_bg_color = SURFACE;
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    visuals.widgets.inactive.bg_fill = SURFACE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, LINE);
    visuals.widgets.hovered.bg_fill = SURFACE_RAISED;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.widgets.active.bg_fill = ACCENT_DIM;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.selection.bg_fill = ACCENT_DIM;
    visuals.selection.stroke.color = ACCENT;
    visuals.override_text_color = Some(TEXT);
    visuals.window_corner_radius = 9.into();

    context.set_visuals(visuals);
    context.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 7.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = 30.0;
    });
}

#[cfg(target_os = "macos")]
fn configure_platform_fonts(context: &egui::Context) {
    let mut definitions = egui::FontDefinitions::default();

    prepend_system_font(
        &mut definitions,
        "mnemos-sf-ui",
        egui::FontFamily::Proportional,
        &[
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
        ],
    );
    prepend_system_font(
        &mut definitions,
        "mnemos-sf-mono",
        egui::FontFamily::Monospace,
        &[
            "/System/Library/Fonts/SFNSMono.ttf",
            "/System/Library/Fonts/Menlo.ttc",
        ],
    );

    context.set_fonts(definitions);
}

#[cfg(not(target_os = "macos"))]
fn configure_platform_fonts(_context: &egui::Context) {}

#[cfg(target_os = "macos")]
fn prepend_system_font(
    definitions: &mut egui::FontDefinitions,
    name: &str,
    family: egui::FontFamily,
    candidates: &[&str],
) {
    let Some(bytes) = candidates.iter().find_map(|path| std::fs::read(path).ok()) else {
        return;
    };

    definitions
        .font_data
        .insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
    definitions
        .families
        .entry(family)
        .or_default()
        .insert(0, name.to_owned());
}

fn paint_card(ui: &egui::Ui, rect: Rect, diameter: u8, fill: Color32, stroke_color: Color32) {
    ui.painter().rect(
        rect,
        gdi_corner_radius(diameter),
        fill,
        Stroke::new(1.0_f32, stroke_color),
        StrokeKind::Inside,
    );
}

fn gdi_corner_radius(diameter: u8) -> u8 {
    diameter / 2
}

fn paint_pill(ui: &egui::Ui, rect: Rect, fill: Color32, stroke_color: Color32) {
    let radius = (rect.width().min(rect.height()) / 2.0)
        .round()
        .clamp(0.0, u8::MAX as f32) as u8;

    ui.painter().rect(
        rect,
        radius,
        fill,
        Stroke::new(1.0_f32, stroke_color),
        StrokeKind::Inside,
    );
}

fn paint_text(
    ui: &egui::Ui,
    position: egui::Pos2,
    anchor: Align2,
    value: &str,
    requested_size: f32,
    color: Color32,
) {
    let font_size = windows_font_size(requested_size);
    let font = FontId::proportional(font_size);
    let weight = windows_weight_offset(requested_size);

    ui.painter()
        .text(position, anchor, value, font.clone(), color);
    ui.painter().text(
        position + egui::vec2(weight, 0.0),
        anchor,
        value,
        font,
        color,
    );
}

fn paint_text_clipped(ui: &egui::Ui, rect: Rect, value: &str, requested_size: f32, color: Color32) {
    let painter = ui.painter().with_clip_rect(rect);
    let position = egui::pos2(rect.left(), rect.center().y);
    let font = FontId::proportional(windows_font_size(requested_size));

    painter.text(position, Align2::LEFT_CENTER, value, font.clone(), color);
    painter.text(
        position + egui::vec2(0.35, 0.0),
        Align2::LEFT_CENTER,
        value,
        font,
        color,
    );
}

fn windows_font_size(requested_size: f32) -> f32 {
    if requested_size >= 26.0 {
        TITLE_FONT_SIZE
    } else if requested_size >= 18.0 {
        SECTION_FONT_SIZE
    } else {
        UI_FONT_SIZE
    }
}

fn windows_weight_offset(requested_size: f32) -> f32 {
    if requested_size >= 18.0 {
        0.75
    } else if requested_size <= 11.0 {
        0.55
    } else {
        0.35
    }
}

#[derive(Clone, Copy)]
enum CollectorButtonKind {
    Primary { disabled: bool },
    Secondary,
    Toggle { enabled: bool },
    Update { disabled: bool },
    Window { danger: bool },
}

struct CollectorButton<'a> {
    label: &'a str,
    kind: CollectorButtonKind,
}

impl egui::Widget for CollectorButton<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let desired_size = ui.available_size_before_wrap();
        let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());
        let hovered = response.hovered();

        match self.kind {
            CollectorButtonKind::Primary { disabled } => {
                let fill = if disabled { ACCENT_DIM } else { ACCENT };
                let border = if hovered && !disabled {
                    TEXT_SECONDARY
                } else {
                    ACCENT
                };
                let text = if disabled { TEXT_SECONDARY } else { BACKGROUND };

                paint_pill(ui, rect, fill, border);
                paint_centered_button_text(ui, rect, self.label, text, true);
            }
            CollectorButtonKind::Secondary => {
                let fill = if hovered { SURFACE_RAISED } else { SURFACE };
                let border = if hovered { LINE_STRONG } else { LINE };
                let text = if hovered { TEXT } else { TEXT_SECONDARY };

                paint_pill(ui, rect, fill, border);
                paint_centered_button_text(ui, rect, self.label, text, false);
            }
            CollectorButtonKind::Toggle { enabled } => {
                let fill = if enabled { ACCENT_DIM } else { SURFACE_RAISED };
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
                let dot = if enabled { ACCENT } else { TEXT_MUTED };

                paint_pill(ui, rect, fill, border);
                ui.painter().circle_filled(
                    egui::pos2(rect.left() + 13.5, rect.top() + 13.5),
                    3.5,
                    dot,
                );
                paint_left_centered_text(
                    ui,
                    Rect::from_min_max(
                        egui::pos2(rect.left() + 25.0, rect.top()),
                        egui::pos2(rect.right() - 8.0, rect.bottom()),
                    ),
                    self.label,
                    text,
                );
            }
            CollectorButtonKind::Update { disabled } => {
                let fill = if disabled { SURFACE_RAISED } else { ACCENT_DIM };
                let border = if disabled { LINE_STRONG } else { ACCENT };
                let text = if disabled { TEXT_MUTED } else { ACCENT };

                paint_pill(ui, rect, fill, border);
                paint_centered_button_text(ui, rect, self.label, text, true);
            }
            CollectorButtonKind::Window { danger } => {
                let (fill, border, text) = if danger && hovered {
                    (DANGER, DANGER, BACKGROUND)
                } else if danger {
                    (DANGER_DIM, DANGER, DANGER)
                } else if hovered {
                    (SURFACE_RAISED, LINE_STRONG, TEXT)
                } else {
                    (SURFACE, LINE, TEXT_SECONDARY)
                };

                paint_pill(ui, rect, fill, border);
                paint_centered_button_text(ui, rect, self.label, text, false);
            }
        }

        response
    }
}

fn primary_button_widget(label: &str, disabled: bool) -> CollectorButton<'_> {
    CollectorButton {
        label,
        kind: CollectorButtonKind::Primary { disabled },
    }
}

fn secondary_button_widget(label: &str) -> CollectorButton<'_> {
    CollectorButton {
        label,
        kind: CollectorButtonKind::Secondary,
    }
}

fn toggle_button_widget(label: &str, enabled: bool) -> CollectorButton<'_> {
    CollectorButton {
        label,
        kind: CollectorButtonKind::Toggle { enabled },
    }
}

fn update_button_widget(label: &str, disabled: bool) -> CollectorButton<'_> {
    CollectorButton {
        label,
        kind: CollectorButtonKind::Update { disabled },
    }
}

#[cfg(target_os = "macos")]
fn window_button(ui: &mut egui::Ui, rect: Rect, label: &str, danger: bool) -> egui::Response {
    ui.put(
        rect,
        CollectorButton {
            label,
            kind: CollectorButtonKind::Window { danger },
        },
    )
}

fn paint_centered_button_text(
    ui: &egui::Ui,
    rect: Rect,
    value: &str,
    color: Color32,
    emphasized: bool,
) {
    let position = rect.center();
    let font = FontId::proportional(UI_FONT_SIZE);
    let weight = if emphasized { 0.55 } else { 0.35 };

    ui.painter()
        .text(position, Align2::CENTER_CENTER, value, font.clone(), color);
    ui.painter().text(
        position + egui::vec2(weight, 0.0),
        Align2::CENTER_CENTER,
        value,
        font,
        color,
    );
}

fn paint_left_centered_text(ui: &egui::Ui, rect: Rect, value: &str, color: Color32) {
    let position = egui::pos2(rect.left(), rect.center().y);
    let font = FontId::proportional(UI_FONT_SIZE);

    ui.painter()
        .text(position, Align2::LEFT_CENTER, value, font.clone(), color);
    ui.painter().text(
        position + egui::vec2(0.35, 0.0),
        Align2::LEFT_CENTER,
        value,
        font,
        color,
    );
}

fn draw_status_tile(ui: &egui::Ui, rect: Rect, label: &str, value: &str, status_color: Color32) {
    paint_card(ui, rect, 18, SURFACE_RAISED, LINE);
    ui.painter().circle_filled(
        egui::pos2(rect.left() + 14.5, rect.top() + 13.5),
        3.5,
        status_color,
    );

    let label_top = rect.top() + STATUS_LABEL_TOP_PADDING;
    let label_bottom = label_top + STATUS_LABEL_HEIGHT;
    let label_rect = Rect::from_min_max(
        egui::pos2(rect.left() + 24.0, label_top),
        egui::pos2(rect.right() - 10.0, label_bottom),
    );
    let value_rect = Rect::from_min_max(
        egui::pos2(rect.left() + 12.0, label_bottom),
        egui::pos2(
            rect.right() - 10.0,
            rect.bottom() - STATUS_VALUE_BOTTOM_PADDING,
        ),
    );

    paint_left_centered_text(ui, label_rect, label, TEXT_MUTED);
    paint_left_centered_text(ui, value_rect, value, TEXT);
}

struct JournalLine<'a> {
    value: &'a str,
    color: Color32,
    selected: bool,
}

impl egui::Widget for JournalLine<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let desired_size = ui.available_size_before_wrap();
        let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());

        if self.selected {
            ui.painter().rect_filled(rect, 0, ACCENT_DIM);
        }

        ui.painter().text(
            rect.left_top(),
            Align2::LEFT_TOP,
            self.value,
            FontId::monospace(MONO_FONT_SIZE),
            self.color,
        );

        response
    }
}

fn journal_line_widget(value: &str, color: Color32, selected: bool) -> JournalLine<'_> {
    JournalLine {
        value,
        color,
        selected,
    }
}

fn paint_log_scrollbar(
    ui: &egui::Ui,
    logs_view: Rect,
    content_height: f32,
    visible_height: f32,
    scroll_offset: f32,
) {
    if content_height <= visible_height || visible_height <= 0.0 {
        return;
    }

    let track = Rect::from_min_max(
        egui::pos2(logs_view.right() - 12.0, logs_view.top() + 10.0),
        egui::pos2(logs_view.right() - 6.0, logs_view.bottom() - 10.0),
    );
    let track_height = track.height().max(1.0);
    let thumb_height = (track_height * visible_height / content_height).clamp(24.0, track_height);
    let max_offset = (content_height - visible_height).max(1.0);
    let travel = (track_height - thumb_height).max(0.0);
    let fraction = (scroll_offset / max_offset).clamp(0.0, 1.0);
    let thumb_top = track.top() + travel * fraction;
    let thumb = Rect::from_min_max(
        egui::pos2(logs_view.right() - 14.0, thumb_top),
        egui::pos2(logs_view.right() - 4.0, thumb_top + thumb_height),
    );

    paint_pill(ui, track, LINE, LINE);
    paint_pill(ui, thumb, TEXT_MUTED, TEXT_MUTED);
}

fn status_copy(
    provisioned: bool,
    runtime: &RuntimeSnapshot,
) -> (&'static str, &'static str, Color32) {
    if !provisioned {
        return (
            "Требуется активация",
            "Введите одноразовый код из Mnemos, чтобы подключить этот Collector.",
            WARNING,
        );
    }

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
            WARNING,
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
            WARNING,
        );
    }

    if is_master_sword(runtime.game_mode.as_str()) {
        return (
            "Подтверждаем сбор",
            "Сессия активна. Завершаем подключение Collector.",
            WARNING,
        );
    }

    ("Cristalix активен", "Ожидаем переход в Master Sword.", TEXT)
}

fn diagnostics_summary(runtime: &RuntimeSnapshot) -> String {
    if let Some(required) = runtime.required_update_version.as_deref() {
        return format!("ТРЕБУЕТСЯ ОБНОВЛЕНИЕ ДО v{required}");
    }

    let protocol = match (
        runtime.collector_protocol_version,
        runtime.server_protocol_version,
    ) {
        (Some(collector), Some(server)) if collector == server => format!("P{collector}"),
        (Some(collector), Some(server)) => format!("P{collector}/{server}"),
        (Some(collector), None) => format!("P{collector}"),
        _ => "P?".to_owned(),
    };
    let queue = if runtime.spool_capacity == 0 {
        format!("QUEUE {}", runtime.spool_pending)
    } else {
        format!("QUEUE {}/{}", runtime.spool_pending, runtime.spool_capacity)
    };
    let log = format!("LOG {}", age_compact(runtime.last_log_activity_at));
    let realtime = format!("WS {}", age_compact(runtime.last_realtime_message_at));

    format!("{queue}  ·  {protocol}  ·  {log}  ·  {realtime}")
}

fn age_compact(timestamp: Option<chrono::DateTime<Utc>>) -> String {
    let Some(timestamp) = timestamp else {
        return "—".to_owned();
    };
    let seconds = Utc::now()
        .signed_duration_since(timestamp)
        .num_seconds()
        .max(0);

    match seconds {
        0..=4 => "now".to_owned(),
        5..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        _ => format!("{}h", seconds / 3_600),
    }
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

fn log_source_recovery_needed(configured: bool, active: bool, has_error: bool) -> bool {
    configured || !active || has_error
}

fn shortened_path(path: &Path) -> String {
    const MAX_CHARS: usize = 92;

    let value = path.to_string_lossy();
    let count = value.chars().count();

    if count <= MAX_CHARS {
        return value.into_owned();
    }

    let suffix = value
        .chars()
        .skip(count.saturating_sub(MAX_CHARS - 1))
        .collect::<String>();

    format!("…{suffix}")
}

fn log_line_color(line: &str) -> Color32 {
    if line.contains("[ERROR]") {
        DANGER
    } else if line.contains("[WARN ") || line.contains("[WARN]") {
        WARNING
    } else if line.contains("[DEBUG]") {
        TEXT_MUTED
    } else {
        TEXT_SECONDARY
    }
}

fn paint_mascot(ui: &egui::Ui, rect: Rect) {
    let size = rect.width().min(rect.height());
    let center = rect.center() + egui::vec2(0.0, size * 0.05);
    let head_radius = size * 0.31;
    let ear_width = size * 0.19;
    let ear_top = rect.top() + size * 0.08;
    let ear_base = center.y - head_radius * 0.58;
    let transparent_stroke = Stroke::new(0.0_f32, Color32::TRANSPARENT);
    let painter = ui.painter();

    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(center.x - head_radius * 0.78, ear_base),
            egui::pos2(center.x - head_radius * 0.78 - ear_width, ear_top),
            egui::pos2(center.x - head_radius * 0.18, ear_base - size * 0.03),
        ],
        ACCENT,
        transparent_stroke,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(center.x + head_radius * 0.78, ear_base),
            egui::pos2(center.x + head_radius * 0.78 + ear_width, ear_top),
            egui::pos2(center.x + head_radius * 0.18, ear_base - size * 0.03),
        ],
        ACCENT,
        transparent_stroke,
    ));
    painter.circle_filled(center, head_radius, ACCENT);
    painter.circle_filled(
        center + egui::vec2(-head_radius * 0.36, -head_radius * 0.08),
        size * 0.035,
        BACKGROUND,
    );
    painter.circle_filled(
        center + egui::vec2(head_radius * 0.36, -head_radius * 0.08),
        size * 0.035,
        BACKGROUND,
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            center + egui::vec2(-size * 0.04, size * 0.05),
            center + egui::vec2(size * 0.04, size * 0.05),
            center + egui::vec2(0.0, size * 0.11),
        ],
        BACKGROUND,
        transparent_stroke,
    ));
}

fn portable_icon() -> egui::IconData {
    const SIZE: usize = 32;
    let mut rgba = vec![0_u8; SIZE * SIZE * 4];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let index = (y * SIZE + x) * 4;
            let dx = x as i32 - 16;
            let dy = y as i32 - 17;
            let inside_head = dx * dx + dy * dy <= 11 * 11;
            let inside_left_ear = y < 11 && x > 5 && x < 15 && y + 3 > 12 - x / 2;
            let inside_right_ear = y < 11 && x > 17 && x < 27 && y + 3 > x / 2 - 5;
            let accent = inside_head || inside_left_ear || inside_right_ear;
            let color = if accent {
                [0xcb, 0xff, 0x2d, 255]
            } else {
                [0x02, 0x03, 0x02, 255]
            };

            rgba[index..index + 4].copy_from_slice(&color);
        }
    }

    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
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

async fn install_from_ui(token: &str, device_name: &str) -> Result<()> {
    let token = Zeroizing::new(token.to_owned());
    let device_name = device_name.to_owned();

    tokio::task::spawn_blocking(move || {
        Installation::install_and_launch(token.as_str(), Some(&device_name))
            .context("не удалось установить Mnemos Collector")
    })
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
    fn portable_layout_matches_windows_geometry_when_healthy() {
        let layout = PortableLayout::new(true, false, false);

        assert_eq!(layout.hero.top(), 74.0);
        assert_eq!(layout.hero.height(), 154.0);
        assert_eq!(layout.logs_card.top(), 244.0);
        assert_eq!(layout.logs_card.bottom(), 698.0);
        assert_eq!(layout.logs_view.top(), 296.0);
        assert_eq!(layout.logs_view.bottom(), 684.0);
    }

    #[test]
    fn journal_actions_match_windows_widths_and_gaps() {
        let layout = PortableLayout::new(true, false, true);

        assert_eq!(layout.debug_toggle.width(), DEBUG_TOGGLE_WIDTH);
        assert_eq!(layout.copy_logs.width(), COPY_LOGS_WIDTH);
        assert_eq!(layout.update_button.width(), UPDATE_BUTTON_WIDTH);
        assert_eq!(
            layout.debug_toggle.left() - layout.copy_logs.right(),
            LOG_ACTION_GAP
        );
        assert_eq!(
            layout.copy_logs.left() - layout.update_button.right(),
            LOG_ACTION_GAP
        );
    }

    #[test]
    fn gdi_round_rect_diameter_maps_to_half_radius() {
        assert_eq!(gdi_corner_radius(24), 12);
        assert_eq!(gdi_corner_radius(18), 9);
    }

    #[test]
    fn portable_typography_uses_windows_font_heights() {
        assert_eq!(windows_font_size(11.0), UI_FONT_SIZE);
        assert_eq!(windows_font_size(19.0), SECTION_FONT_SIZE);
        assert_eq!(windows_font_size(27.0), TITLE_FONT_SIZE);
    }

    #[test]
    fn portable_icon_has_expected_dimensions() {
        let icon = portable_icon();

        assert_eq!(icon.width, 32);
        assert_eq!(icon.height, 32);
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
    }

    #[test]
    fn log_level_colors_distinguish_diagnostics() {
        assert_eq!(log_line_color("[ERROR] failed"), DANGER);
        assert_eq!(log_line_color("[WARN ] delayed"), WARNING);
        assert_eq!(log_line_color("[DEBUG] detail"), TEXT_MUTED);
        assert_eq!(log_line_color("[INFO ] ready"), TEXT_SECONDARY);
    }

    #[test]
    fn master_sword_label_accepts_runtime_and_human_forms() {
        assert!(is_master_sword("MasterSword"));
        assert!(is_master_sword("Master Sword"));
        assert!(!is_master_sword("Unknown"));
    }

    #[test]
    fn healthy_auto_log_source_does_not_add_an_extra_card() {
        assert!(!log_source_recovery_needed(false, true, false));
        assert!(log_source_recovery_needed(false, false, false));
        assert!(log_source_recovery_needed(true, true, false));
        assert!(log_source_recovery_needed(false, true, true));
    }

    #[test]
    fn diagnostics_summary_matches_windows_information_density() {
        let runtime = RuntimeSnapshot {
            spool_pending: 12,
            spool_capacity: 1_024,
            collector_protocol_version: Some(1),
            server_protocol_version: Some(1),
            ..RuntimeSnapshot::default()
        };
        let summary = diagnostics_summary(&runtime);

        assert!(summary.contains("QUEUE 12/1024"));
        assert!(summary.contains("P1"));
    }
}
