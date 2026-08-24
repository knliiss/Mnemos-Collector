use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText};
use tokio::runtime::Handle;
use zeroize::Zeroizing;

use crate::application::CollectorApplication;
use crate::diagnostics::{self, RuntimeSnapshot};
use crate::platform::{Autostart, Installation};
use crate::provisioning::{ProvisioningClient, default_device_name};
use crate::security::CredentialStore;

use super::DesktopLaunchContext;

const WINDOW_WIDTH: f32 = 1080.0;
const WINDOW_HEIGHT: f32 = 720.0;
const REFRESH_INTERVAL: Duration = Duration::from_millis(500);

const BACKGROUND: Color32 = Color32::from_rgb(12, 16, 18);
const SURFACE: Color32 = Color32::from_rgb(22, 28, 31);
const SURFACE_RAISED: Color32 = Color32::from_rgb(29, 36, 40);
const TEXT: Color32 = Color32::from_rgb(238, 244, 241);
const TEXT_MUTED: Color32 = Color32::from_rgb(151, 164, 158);
const ACCENT: Color32 = Color32::from_rgb(125, 235, 157);
const WARNING: Color32 = Color32::from_rgb(245, 190, 87);
const DANGER: Color32 = Color32::from_rgb(242, 113, 113);

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
    activation_token: String,
    device_name: String,
    activation_error: Option<String>,
    activation_receiver: Option<Receiver<Result<ActivationOutcome, String>>>,
    selected_log_entry: Option<String>,
}

impl PortableDesktop {
    fn new(context: DesktopLaunchContext, runtime: Handle) -> Self {
        let mut desktop = Self {
            runtime,
            current_installation: context.current_installation,
            provisioned: context.access_key.is_some(),
            worker_started: false,
            provisioning: false,
            activation_token: String::new(),
            device_name: default_device_name(),
            activation_error: None,
            activation_receiver: None,
            selected_log_entry: None,
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
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(message) => {
                diagnostics::error("provisioning", message.clone());
                self.activation_error = Some(message);
            }
        }
    }

    fn copy_selected_log(&self, context: &egui::Context) {
        if let Some(entry) = self.selected_log_entry.as_ref() {
            context.copy_text(entry.clone());
            diagnostics::info("desktop", "Selected log entry copied to clipboard");
        }
    }

    fn draw_header(&self, ui: &mut egui::Ui, runtime: &RuntimeSnapshot) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("MNEMOS").size(12.0).color(ACCENT).strong());
                ui.label(RichText::new("Collector").size(28.0).color(TEXT).strong());
                ui.label(
                    RichText::new("Cristalix Master Sword telemetry")
                        .size(13.0)
                        .color(TEXT_MUTED),
                );
            });

            ui.add_space(28.0);
            status_card(
                ui,
                "ИГРА",
                if runtime.cristalix_running {
                    "Cristalix"
                } else {
                    "Не запущена"
                },
                runtime.cristalix_running,
            );
            status_card(
                ui,
                "РЕЖИМ",
                if runtime.game_mode.eq_ignore_ascii_case("unknown") {
                    "—"
                } else {
                    runtime.game_mode.as_str()
                },
                runtime.game_mode.eq_ignore_ascii_case("master sword"),
            );
            status_card(
                ui,
                "MNEMOS",
                if runtime.observing {
                    "Подключён"
                } else if runtime.realtime_connected {
                    "Подключение"
                } else {
                    "Отключён"
                },
                runtime.observing,
            );
        });
    }

    fn draw_activation(&mut self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.heading("Активация Collector");
            ui.label(
                RichText::new(if self.current_installation {
                    "Введите одноразовый код из Mnemos. Credential будет сохранён в системном хранилище."
                } else {
                    "Введите одноразовый код из Mnemos. Collector установит себя в стабильное пользовательское расположение и запустится оттуда."
                })
                .color(TEXT_MUTED),
            );
            ui.add_space(10.0);

            ui.add_enabled_ui(!self.provisioning, |ui| {
                ui.label(RichText::new("Код активации").color(TEXT_MUTED));
                ui.add_sized(
                    [ui.available_width(), 36.0],
                    egui::TextEdit::singleline(&mut self.activation_token)
                        .password(true)
                        .hint_text("Одноразовый код"),
                );
                ui.add_space(8.0);
                ui.label(RichText::new("Устройство").color(TEXT_MUTED));
                ui.add_sized(
                    [ui.available_width(), 36.0],
                    egui::TextEdit::singleline(&mut self.device_name),
                );
            });

            ui.add_space(12.0);

            let button_text = if self.provisioning {
                "Активация…"
            } else {
                "Активировать"
            };
            let activated = ui
                .add_enabled(
                    !self.provisioning,
                    egui::Button::new(RichText::new(button_text).strong())
                        .min_size(egui::vec2(160.0, 36.0)),
                )
                .clicked();

            if activated {
                self.begin_activation();
            }

            if let Some(error) = self.activation_error.as_deref() {
                ui.add_space(8.0);
                ui.colored_label(DANGER, error);
            }
        });
    }

    fn draw_journal(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.heading("Журнал Collector");
            ui.add_space(10.0);

            if ui.button("Копировать всё").clicked() {
                context.copy_text(diagnostics::recent_text());
                diagnostics::info("desktop", "Journal copied to clipboard as text");
            }

            let diagnostics_label = if diagnostics::debug_enabled() {
                "Диагностика: вкл"
            } else {
                "Диагностика: выкл"
            };

            if ui.button(diagnostics_label).clicked() {
                diagnostics::set_debug_enabled(!diagnostics::debug_enabled());
            }
        });

        let log_text = diagnostics::recent_text();

        if let Some(selected) = self.selected_log_entry.as_ref()
            && !log_text.lines().any(|line| line == selected)
        {
            self.selected_log_entry = None;
        }

        ui.group(|ui| {
            ui.set_min_height(if self.provisioned { 430.0 } else { 245.0 });
            ui.set_min_width(ui.available_width());

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if log_text.is_empty() {
                        ui.label(RichText::new("Журнал пока пуст.").color(TEXT_MUTED));
                    }

                    for line in log_text.lines() {
                        let selected = self.selected_log_entry.as_deref() == Some(line);
                        let text = RichText::new(line).monospace().color(log_line_color(line));
                        let response = ui.selectable_label(selected, text);

                        if response.clicked() {
                            self.selected_log_entry = Some(line.to_owned());
                        }
                    }
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
        context.request_repaint_after(REFRESH_INTERVAL);

        let runtime = diagnostics::runtime_snapshot();

        egui::CentralPanel::default().show(context, |ui| {
            ui.visuals_mut().panel_fill = BACKGROUND;
            self.draw_header(ui, &runtime);

            if !self.provisioned {
                self.draw_activation(ui);
            }

            self.draw_journal(ui, context);

            if let Some(error) = runtime.last_error.as_deref() {
                ui.add_space(8.0);
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
    visuals.extreme_bg_color = SURFACE_RAISED;
    visuals.faint_bg_color = SURFACE;
    visuals.widgets.inactive.bg_fill = SURFACE_RAISED;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(39, 51, 45);
    visuals.widgets.active.bg_fill = Color32::from_rgb(47, 66, 54);
    visuals.selection.bg_fill = Color32::from_rgb(55, 104, 70);
    visuals.selection.stroke.color = ACCENT;
    visuals.override_text_color = Some(TEXT);

    context.set_visuals(visuals);
    context.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
    });
}

fn status_card(ui: &mut egui::Ui, label: &str, value: &str, healthy: bool) {
    ui.group(|ui| {
        ui.set_min_width(210.0);
        ui.set_min_height(66.0);
        ui.label(RichText::new(label).size(11.0).color(TEXT_MUTED).strong());
        ui.label(
            RichText::new(value)
                .size(17.0)
                .color(if healthy { ACCENT } else { WARNING })
                .strong(),
        );
    });
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
                [125, 235, 157, 255]
            } else {
                [12, 16, 18, 255]
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
}
