use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use eframe::egui::{self, Align, Color32, RichText, Sense, Stroke};
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

    fn handle_window_close(&self, _context: &egui::Context) {
        #[cfg(target_os = "macos")]
        {
            let close_requested = _context.input(|input| input.viewport().close_requested());

            if close_requested && !self.exit_requested {
                _context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                macos_native::hide_application();
            }
        }
    }

    fn copy_selected_log(&self, context: &egui::Context) {
        if let Some(entry) = self.selected_log_entry.as_ref() {
            context.copy_text(entry.clone());
            diagnostics::info("desktop", "Selected log entry copied to clipboard");
        }
    }

    fn draw_header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0_f32, LINE))
                .corner_radius(18)
                .inner_margin(2)
                .show(ui, |ui| {
                    draw_mascot(ui, 40.0);
                });

            ui.add_space(8.0);

            ui.vertical(|ui| {
                ui.label(RichText::new("MNEMOS").size(11.0).color(ACCENT).strong());
                ui.label(RichText::new("Collector").size(22.0).color(TEXT).strong());
            });

            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .size(12.0)
                        .color(TEXT_MUTED),
                );
            });
        });
    }

    fn draw_hero(&self, ui: &mut egui::Ui, runtime: &RuntimeSnapshot) {
        let (title, detail, status_color) = status_copy(self.provisioned, runtime);
        let response = card_frame(24).show(ui, |ui| {
            ui.set_min_height(122.0);

            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("СТАТУС").size(11.0).color(ACCENT).strong());
                    ui.label(RichText::new(title).size(27.0).color(status_color).strong());
                    ui.label(RichText::new(detail).size(13.0).color(TEXT_SECONDARY));
                });

                ui.with_layout(egui::Layout::right_to_left(Align::TOP), |ui| {
                    draw_mascot(ui, 48.0);
                });
            });

            ui.add_space(12.0);

            ui.columns(3, |columns| {
                status_tile(
                    &mut columns[0],
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
                status_tile(
                    &mut columns[1],
                    "РЕЖИМ",
                    game_mode_label(runtime.game_mode.as_str()),
                    if is_master_sword(runtime.game_mode.as_str()) {
                        ACCENT
                    } else {
                        WARNING
                    },
                );
                status_tile(
                    &mut columns[2],
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
            });
        });

        let rect = response.response.rect;
        let accent_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 1.0, rect.top() + 24.0),
            egui::pos2(rect.left() + 4.0, rect.top() + 82.0),
        );
        ui.painter().rect_filled(accent_rect, 2.0, status_color);
    }

    fn draw_activation(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0_f32, LINE))
            .corner_radius(24)
            .inner_margin(18)
            .show(ui, |ui| {
                ui.set_min_height(98.0);

                ui.label(
                    RichText::new(if self.current_installation {
                        "Подключить Collector"
                    } else {
                        "Установить Collector"
                    })
                    .size(19.0)
                    .color(TEXT)
                    .strong(),
                );
                ui.add_space(8.0);

                let gap = 10.0;
                let device_width = 176.0;
                let button_width = 132.0;
                let available_width = ui.available_width();
                let token_width =
                    (available_width - device_width - button_width - gap * 2.0).max(210.0);

                ui.horizontal_top(|ui| {
                    ui.set_width(available_width);

                    ui.vertical(|ui| {
                        ui.set_width(token_width);
                        ui.label(
                            RichText::new("Код активации")
                                .size(11.0)
                                .color(TEXT_MUTED),
                        );
                        ui.add_enabled_ui(!self.provisioning, |ui| {
                            styled_text_edit(
                                ui,
                                &mut self.activation_token,
                                token_width,
                                true,
                                Some("Одноразовый код"),
                            );
                        });
                    });

                    ui.add_space(gap - ui.spacing().item_spacing.x);

                    ui.vertical(|ui| {
                        ui.set_width(device_width);
                        ui.label(
                            RichText::new("Устройство")
                                .size(11.0)
                                .color(TEXT_MUTED),
                        );
                        ui.add_enabled_ui(!self.provisioning, |ui| {
                            styled_text_edit(
                                ui,
                                &mut self.device_name,
                                device_width,
                                false,
                                None,
                            );
                        });
                    });

                    ui.add_space(gap - ui.spacing().item_spacing.x);

                    ui.vertical(|ui| {
                        ui.set_width(button_width);
                        ui.add_space(18.0);

                        let button_text = if self.provisioning {
                            "Подключаем..."
                        } else {
                            "Активировать"
                        };
                        let activate = primary_button(
                            ui,
                            button_text,
                            button_width,
                            self.provisioning,
                        )
                        .clicked();

                        if activate {
                            self.begin_activation();
                        }
                    });
                });

                if let Some(error) = self.activation_error.as_deref() {
                    ui.add_space(6.0);
                    ui.colored_label(DANGER, error);
                }
            });
    }

    fn draw_log_source(&mut self, ui: &mut egui::Ui, runtime: &RuntimeSnapshot) {
        let configured_path = configured_latest_log_path();
        let active_path = runtime.log_path.as_deref();
        let source_text = if let Some(path) = configured_path.as_deref() {
            format!("Ручной источник: {}", shortened_path(path))
        } else if let Some(path) = active_path {
            format!("Авто: {}", shortened_path(path))
        } else {
            "Автопоиск: лог пока не найден".to_owned()
        };

        card_frame(18).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("ЛОГ CRISTALIX").size(10.0).color(ACCENT).strong());
                    ui.label(RichText::new(source_text).size(12.0).color(TEXT_SECONDARY));
                });

                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    #[cfg(target_os = "macos")]
                    if secondary_button(ui, "Выбрать файл…", 124.0).clicked() {
                        self.select_macos_log_file();
                    }

                    if configured_path.is_some()
                        && secondary_button(ui, "Автопоиск", 104.0).clicked()
                    {
                        match clear_configured_latest_log_path() {
                            Ok(()) => {
                                self.log_source_error = None;
                                diagnostics::info(
                                    "cristalix",
                                    "Manual Cristalix log source cleared; automatic discovery enabled",
                                );
                            }
                            Err(error) => {
                                self.log_source_error = Some(format!(
                                    "Не удалось вернуть автопоиск: {error}"
                                ));
                            }
                        }
                    }
                });
            });

            if let Some(error) = self.log_source_error.as_deref() {
                ui.add_space(4.0);
                ui.colored_label(DANGER, error);
            }
        });
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

    fn draw_journal(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let desired_height = ui.available_height().max(180.0);

        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0_f32, LINE))
            .corner_radius(24)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_min_height((desired_height - 30.0).max(150.0));

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Журнал").size(19.0).color(TEXT).strong());

                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if toggle_button(
                            ui,
                            "Диагностика",
                            diagnostics::debug_enabled(),
                            154.0,
                        )
                        .clicked()
                        {
                            diagnostics::set_debug_enabled(!diagnostics::debug_enabled());
                        }

                        if secondary_button(ui, "Копировать всё", 156.0).clicked() {
                            context.copy_text(diagnostics::recent_text());
                            diagnostics::info("desktop", "Journal copied to clipboard as text");
                        }
                    });
                });

                ui.add_space(7.0);

                let log_text = diagnostics::recent_text();

                if let Some(selected) = self.selected_log_entry.as_ref()
                    && !log_text.lines().any(|line| line == selected)
                {
                    self.selected_log_entry = None;
                }

                egui::Frame::new()
                    .fill(LOG_SURFACE)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .corner_radius(18)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_min_height((ui.available_height() - 2.0).max(120.0));

                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                if log_text.is_empty() {
                                    ui.label(
                                        RichText::new("Журнал пока пуст.")
                                            .size(12.0)
                                            .color(TEXT_MUTED),
                                    );
                                }

                                for line in log_text.lines() {
                                    let selected = self.selected_log_entry.as_deref() == Some(line);
                                    let text = RichText::new(line)
                                        .monospace()
                                        .size(11.5)
                                        .color(log_line_color(line));
                                    let response = ui.selectable_label(selected, text);

                                    if response.clicked() {
                                        self.selected_log_entry = Some(line.to_owned());
                                    }
                                }
                            });
                    });
            });

        let copy_pressed =
            context.input(|input| input.modifiers.command && input.key_pressed(egui::Key::C));

        if copy_pressed {
            self.copy_selected_log(context);
        }
    }
}

impl eframe::App for PortableDesktop {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_activation(context);
        self.handle_window_close(context);
        context.request_repaint_after(REFRESH_INTERVAL);

        let runtime = diagnostics::runtime_snapshot();

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(22))
            .show(context, |ui| {
                self.draw_header(ui);
                ui.add_space(12.0);
                self.draw_hero(ui, &runtime);
                ui.add_space(16.0);

                if !self.provisioned {
                    self.draw_activation(ui);
                    ui.add_space(16.0);
                }

                self.draw_log_source(ui, &runtime);
                ui.add_space(16.0);
                self.draw_journal(ui, context);

                if let Some(error) = runtime.last_error.as_deref() {
                    ui.add_space(6.0);
                    ui.colored_label(DANGER, error);
                }
            });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BACKGROUND.to_normalized_gamma_f32()
    }
}

fn configure_style(context: &egui::Context) {
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
    visuals.window_corner_radius = 18.into();

    context.set_visuals(visuals);
    context.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 7.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = 32.0;
    });
}

fn card_frame(radius: u8) -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(radius)
        .inner_margin(16)
}

fn styled_text_edit(
    ui: &mut egui::Ui,
    value: &mut String,
    width: f32,
    password: bool,
    hint: Option<&str>,
) -> egui::Response {
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .corner_radius(17)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            let mut edit = egui::TextEdit::singleline(value)
                .frame(false)
                .desired_width((width - 20.0).max(40.0));

            if password {
                edit = edit.password(true);
            }

            if let Some(hint) = hint {
                edit = edit.hint_text(hint);
            }

            ui.add_sized([(width - 20.0).max(40.0), 24.0], edit)
        })
        .inner
}

fn primary_button(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
    disabled: bool,
) -> egui::Response {
    let fill = if disabled { ACCENT_DIM } else { ACCENT };
    let text = if disabled { TEXT_SECONDARY } else { BACKGROUND };
    let border = if disabled { ACCENT_DIM } else { ACCENT };

    ui.add_sized(
        [width, 34.0],
        egui::Button::new(RichText::new(label).size(13.0).color(text).strong())
            .fill(fill)
            .stroke(Stroke::new(1.0_f32, border))
            .corner_radius(17),
    )
}

fn secondary_button(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    ui.add_sized(
        [width, 30.0],
        egui::Button::new(RichText::new(label).size(12.0).color(TEXT_SECONDARY))
            .fill(SURFACE)
            .stroke(Stroke::new(1.0_f32, LINE))
            .corner_radius(15),
    )
}

fn toggle_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    width: f32,
) -> egui::Response {
    let fill = if enabled { ACCENT_DIM } else { SURFACE_RAISED };
    let border = if enabled { ACCENT } else { LINE };
    let dot = if enabled { "●" } else { "○" };
    let text = format!("{dot}  {label}");

    ui.add_sized(
        [width, 30.0],
        egui::Button::new(RichText::new(text).size(12.0).color(TEXT_SECONDARY))
            .fill(fill)
            .stroke(Stroke::new(1.0_f32, border))
            .corner_radius(15),
    )
}

fn status_tile(ui: &mut egui::Ui, label: &str, value: &str, color: Color32) {
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(18)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_min_height(46.0);

            ui.horizontal(|ui| {
                let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 3.5, color);
                ui.label(RichText::new(label).size(10.0).color(TEXT_MUTED).strong());
            });

            ui.label(RichText::new(value).size(13.0).color(TEXT).strong());
        });
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
        TEXT
    }
}

fn draw_mascot(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let painter = ui.painter();
    let center = rect.center() + egui::vec2(0.0, size * 0.05);
    let head_radius = size * 0.31;
    let ear_width = size * 0.19;
    let ear_top = rect.top() + size * 0.08;
    let ear_base = center.y - head_radius * 0.58;
    let transparent_stroke = Stroke::new(0.0_f32, Color32::TRANSPARENT);

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
        assert_eq!(log_line_color("[INFO ] ready"), TEXT);
    }

    #[test]
    fn master_sword_label_accepts_runtime_and_human_forms() {
        assert!(is_master_sword("MasterSword"));
        assert!(is_master_sword("Master Sword"));
        assert!(!is_master_sword("Unknown"));
    }
}
