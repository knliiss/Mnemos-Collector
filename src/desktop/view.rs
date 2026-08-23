use std::ffi::c_void;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, Ellipse, RoundRect, SelectObject, SetBkMode,
    SetTextColor, TextOutW,
};

use crate::diagnostics::{self, RuntimeSnapshot};

use super::mascot;
use super::theme;

#[derive(Clone, Copy)]
pub(super) struct UiRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl UiRect {
    pub fn width(self) -> i32 {
        self.right - self.left
    }

    pub fn height(self) -> i32 {
        self.bottom - self.top
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
}

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub hero: UiRect,
    pub tray_button: UiRect,
    pub activation: Option<UiRect>,
    pub token_edit: UiRect,
    pub device_edit: UiRect,
    pub activate_button: UiRect,
    pub logs_card: UiRect,
    pub logs_edit: UiRect,
    pub debug_toggle: UiRect,
}

#[derive(Clone, Copy)]
pub(super) struct Fonts {
    pub ui: *mut c_void,
    pub title: *mut c_void,
    pub section: *mut c_void,
}

pub(super) struct ViewState<'a> {
    pub current_installation: bool,
    pub provisioning: bool,
    pub activation_error: Option<&'a str>,
    pub debug_enabled: bool,
}

pub(super) fn layout(width: i32, height: i32, provisioned: bool) -> Layout {
    let margin = 28;
    let hero = UiRect {
        left: margin,
        top: 94,
        right: (width - margin).max(margin + 720),
        bottom: 276,
    };
    let tray_button = UiRect {
        left: hero.right - 364,
        top: hero.top + 22,
        right: hero.right - 176,
        bottom: hero.top + 60,
    };

    let (activation, logs_top) = if provisioned {
        (None, 306)
    } else {
        (
            Some(UiRect {
                left: margin,
                top: 296,
                right: width - margin,
                bottom: 452,
            }),
            482,
        )
    };

    let activation_rect = activation.unwrap_or(UiRect {
        left: margin,
        top: 0,
        right: width - margin,
        bottom: 0,
    });
    let edit_top = activation_rect.top + 84;
    let available = (activation_rect.width() - 40).max(560);
    let button_width = 156;
    let device_width = 210;
    let gap = 12;
    let token_width = (available - button_width - device_width - gap * 2).max(220);
    let token_edit = UiRect {
        left: activation_rect.left + 20,
        top: edit_top,
        right: activation_rect.left + 20 + token_width,
        bottom: edit_top + 38,
    };
    let device_edit = UiRect {
        left: token_edit.right + gap,
        top: edit_top,
        right: token_edit.right + gap + device_width,
        bottom: edit_top + 38,
    };
    let activate_button = UiRect {
        left: device_edit.right + gap,
        top: edit_top,
        right: activation_rect.right - 20,
        bottom: edit_top + 38,
    };

    let logs_card = UiRect {
        left: margin,
        top: logs_top,
        right: width - margin,
        bottom: (height - margin).max(logs_top + 190),
    };
    let debug_toggle = UiRect {
        left: logs_card.right - 244,
        top: logs_card.top + 14,
        right: logs_card.right - 18,
        bottom: logs_card.top + 46,
    };
    let logs_edit = UiRect {
        left: logs_card.left + 18,
        top: logs_card.top + 58,
        right: logs_card.right - 18,
        bottom: logs_card.bottom - 42,
    };

    Layout {
        hero,
        tray_button,
        activation,
        token_edit,
        device_edit,
        activate_button,
        logs_card,
        logs_edit,
        debug_toggle,
    }
}

pub(super) unsafe fn draw(
    hdc: *mut c_void,
    runtime: &RuntimeSnapshot,
    layout: Layout,
    fonts: Fonts,
    state: ViewState<'_>,
) {
    unsafe {
        draw_header(hdc, fonts);
        draw_hero(hdc, runtime, layout, fonts);

        mascot::draw(hdc, layout.hero.right - 160, layout.hero.top + 16, 122);

        if let Some(activation) = layout.activation {
            draw_activation(hdc, activation, layout, fonts, &state);
        }

        draw_logs_panel(hdc, layout, fonts, state.debug_enabled);
    }
}

unsafe fn draw_header(hdc: *mut c_void, fonts: Fonts) {
    unsafe {
        draw_text(hdc, 30, 24, "MNEMOS", fonts.ui, theme::ACCENT);
        draw_text(hdc, 30, 48, "Collector", fonts.title, theme::TEXT);
        draw_text(
            hdc,
            168,
            60,
            "Cristalix / Master Sword",
            fonts.ui,
            theme::TEXT_MUTED,
        );
    }
}

unsafe fn draw_hero(hdc: *mut c_void, runtime: &RuntimeSnapshot, layout: Layout, fonts: Fonts) {
    let (title, detail, status_color) = status_copy(runtime);
    let rect = layout.hero;

    unsafe {
        draw_card(hdc, rect, theme::SURFACE, theme::LINE);
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 18,
            "СОСТОЯНИЕ",
            fonts.ui,
            theme::ACCENT,
        );
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 45,
            title,
            fonts.title,
            status_color,
        );
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 82,
            detail,
            fonts.ui,
            theme::TEXT_SECONDARY,
        );

        draw_secondary_button(hdc, layout.tray_button, "Свернуть в трей", fonts.ui);
        draw_status_chips(hdc, runtime, rect, fonts.ui);
    }
}

unsafe fn draw_status_chips(
    hdc: *mut c_void,
    runtime: &RuntimeSnapshot,
    rect: UiRect,
    font: *mut c_void,
) {
    let chips_top = rect.bottom - 52;
    let chip_width = 142;
    let gap = 8;
    let mut chip_left = rect.left + 20;

    unsafe {
        draw_status_chip(
            hdc,
            chip_rect(chip_left, chips_top, chip_width),
            "Cristalix",
            if runtime.cristalix_running {
                "найден"
            } else {
                "ожидание"
            },
            if runtime.cristalix_running {
                theme::POSITIVE
            } else {
                theme::TEXT_MUTED
            },
            font,
        );
        chip_left += chip_width + gap;

        let mode = if runtime.game_mode.is_empty() {
            "Unknown"
        } else {
            runtime.game_mode.as_str()
        };
        draw_status_chip(
            hdc,
            chip_rect(chip_left, chips_top, chip_width),
            "Режим",
            mode,
            if runtime.game_mode == "MasterSword" {
                theme::ACCENT
            } else {
                theme::AMBER
            },
            font,
        );
        chip_left += chip_width + gap;

        draw_status_chip(
            hdc,
            chip_rect(chip_left, chips_top, chip_width),
            "Realtime",
            if runtime.realtime_connected {
                "online"
            } else {
                "offline"
            },
            if runtime.realtime_connected {
                theme::POSITIVE
            } else {
                theme::DANGER
            },
            font,
        );
        chip_left += chip_width + gap;

        draw_status_chip(
            hdc,
            chip_rect(chip_left, chips_top, chip_width),
            "Наблюдение",
            if runtime.observing {
                "активно"
            } else {
                "пауза"
            },
            if runtime.observing {
                theme::ACCENT
            } else {
                theme::TEXT_MUTED
            },
            font,
        );
    }
}

fn chip_rect(left: i32, top: i32, width: i32) -> UiRect {
    UiRect {
        left,
        top,
        right: left + width,
        bottom: top + 34,
    }
}

fn status_copy(runtime: &RuntimeSnapshot) -> (&'static str, &'static str, u32) {
    if runtime.observing {
        return (
            "Наблюдение активно",
            "Master Sword распознан, realtime-service подтвердил OBSERVING.",
            theme::ACCENT,
        );
    }

    if !runtime.cristalix_running {
        return (
            "Ожидание Cristalix",
            "Collector работает в фоне и автоматически подхватит уже открытый режим.",
            theme::TEXT,
        );
    }

    if runtime.game_mode == "MasterSword" && !runtime.realtime_connected {
        return (
            "Master Sword найден",
            "Режим распознан. Восстанавливаем соединение с realtime-service.",
            theme::AMBER,
        );
    }

    if runtime.game_mode == "MasterSword" {
        return (
            "Master Sword найден",
            "Режим распознан. Ожидаем подтверждение OBSERVING от realtime-service.",
            theme::AMBER,
        );
    }

    (
        "Cristalix найден",
        "Collector анализирует latest.log и восстанавливает контекст без перезахода.",
        theme::TEXT,
    )
}

unsafe fn draw_activation(
    hdc: *mut c_void,
    activation: UiRect,
    layout: Layout,
    fonts: Fonts,
    state: &ViewState<'_>,
) {
    unsafe {
        draw_card(hdc, activation, theme::SURFACE, theme::LINE);
        draw_text(
            hdc,
            activation.left + 20,
            activation.top + 17,
            if state.current_installation {
                "Подключить Collector"
            } else {
                "Установить и подключить Collector"
            },
            fonts.section,
            theme::TEXT,
        );
        draw_text(
            hdc,
            activation.left + 20,
            activation.top + 49,
            "Одноразовый код из Mnemos",
            fonts.ui,
            theme::TEXT_MUTED,
        );
        draw_text(
            hdc,
            layout.device_edit.left,
            activation.top + 49,
            "Имя устройства",
            fonts.ui,
            theme::TEXT_MUTED,
        );
        draw_primary_button(
            hdc,
            layout.activate_button,
            if state.provisioning {
                "Подключаем..."
            } else {
                "Активировать"
            },
            fonts.ui,
        );

        if let Some(error) = state.activation_error {
            draw_text(
                hdc,
                activation.left + 20,
                activation.bottom - 27,
                error,
                fonts.ui,
                theme::DANGER,
            );
        }
    }
}

unsafe fn draw_logs_panel(hdc: *mut c_void, layout: Layout, fonts: Fonts, debug_enabled: bool) {
    unsafe {
        draw_card(hdc, layout.logs_card, theme::SURFACE, theme::LINE);
        draw_text(
            hdc,
            layout.logs_card.left + 18,
            layout.logs_card.top + 16,
            "Логи Collector",
            fonts.section,
            theme::TEXT,
        );
        draw_toggle(
            hdc,
            layout.debug_toggle,
            "Подробная диагностика",
            debug_enabled,
            fonts.ui,
        );

        if let Some(path) = diagnostics::log_file_path() {
            draw_text(
                hdc,
                layout.logs_card.left + 18,
                layout.logs_card.bottom - 29,
                &format!("Файл: {}", path.display()),
                fonts.ui,
                theme::TEXT_MUTED,
            );
        }
    }
}

unsafe fn draw_status_chip(
    hdc: *mut c_void,
    rect: UiRect,
    label: &str,
    value: &str,
    status_color: u32,
    font: *mut c_void,
) {
    unsafe {
        draw_card(hdc, rect, theme::SURFACE_RAISED, theme::LINE);
        draw_dot(hdc, rect.left + 11, rect.top + 13, status_color);
        draw_text(
            hdc,
            rect.left + 25,
            rect.top + 5,
            label,
            font,
            theme::TEXT_MUTED,
        );
        draw_text(hdc, rect.left + 25, rect.top + 18, value, font, theme::TEXT);
    }
}

unsafe fn draw_toggle(
    hdc: *mut c_void,
    rect: UiRect,
    label: &str,
    enabled: bool,
    font: *mut c_void,
) {
    unsafe {
        draw_card(hdc, rect, theme::SURFACE_RAISED, theme::LINE);
        draw_dot(
            hdc,
            rect.left + 14,
            rect.top + 13,
            if enabled {
                theme::ACCENT
            } else {
                theme::TEXT_MUTED
            },
        );
        draw_text(
            hdc,
            rect.left + 30,
            rect.top + 8,
            label,
            font,
            if enabled {
                theme::TEXT
            } else {
                theme::TEXT_MUTED
            },
        );
    }
}

unsafe fn draw_primary_button(hdc: *mut c_void, rect: UiRect, label: &str, font: *mut c_void) {
    unsafe {
        draw_card(hdc, rect, theme::ACCENT, theme::ACCENT);
        draw_text(
            hdc,
            rect.left + 16,
            rect.top + 9,
            label,
            font,
            theme::BACKGROUND_DEEP,
        );
    }
}

unsafe fn draw_secondary_button(hdc: *mut c_void, rect: UiRect, label: &str, font: *mut c_void) {
    unsafe {
        draw_card(hdc, rect, theme::SURFACE_RAISED, theme::LINE_STRONG);
        draw_text(hdc, rect.left + 15, rect.top + 9, label, font, theme::TEXT);
    }
}

unsafe fn draw_dot(hdc: *mut c_void, x: i32, y: i32, color: u32) {
    let brush = unsafe { CreateSolidBrush(color) };
    let pen = unsafe { CreatePen(0, 1, color) };
    let previous_brush = unsafe { SelectObject(hdc, brush) };
    let previous_pen = unsafe { SelectObject(hdc, pen) };

    unsafe {
        Ellipse(hdc, x, y, x + 8, y + 8);
        SelectObject(hdc, previous_pen);
        SelectObject(hdc, previous_brush);
        DeleteObject(pen);
        DeleteObject(brush);
    }
}

unsafe fn draw_card(hdc: *mut c_void, rect: UiRect, fill: u32, border: u32) {
    let brush = unsafe { CreateSolidBrush(fill) };
    let pen = unsafe { CreatePen(0, 1, border) };
    let previous_brush = unsafe { SelectObject(hdc, brush) };
    let previous_pen = unsafe { SelectObject(hdc, pen) };

    unsafe {
        RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, 16, 16);
        SelectObject(hdc, previous_pen);
        SelectObject(hdc, previous_brush);
        DeleteObject(pen);
        DeleteObject(brush);
    }
}

unsafe fn draw_text(hdc: *mut c_void, x: i32, y: i32, text: &str, font: *mut c_void, color: u32) {
    let text = text.encode_utf16().collect::<Vec<_>>();
    let previous_font = unsafe { SelectObject(hdc, font) };

    unsafe {
        SetTextColor(hdc, color);
        SetBkMode(hdc, 1);
        TextOutW(hdc, x, y, text.as_ptr(), text.len() as i32);
        SelectObject(hdc, previous_font);
    }
}

pub(super) unsafe fn fill_background(hdc: *mut c_void, client: &RECT) {
    let background = unsafe { CreateSolidBrush(theme::BACKGROUND_DEEP) };

    unsafe {
        windows_sys::Win32::Graphics::Gdi::FillRect(hdc, client, background);
        DeleteObject(background);
        SetBkMode(hdc, 1);
    }
}
