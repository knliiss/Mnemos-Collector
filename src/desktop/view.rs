use std::ffi::c_void;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, Ellipse, RoundRect, SelectObject, SetBkMode,
    SetTextColor, TextOutW,
};

use crate::diagnostics::RuntimeSnapshot;

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
    let margin = 24;
    let content_right = (width - margin).max(margin + 700);
    let hero = UiRect {
        left: margin,
        top: 92,
        right: content_right,
        bottom: 244,
    };

    let (activation, logs_top) = if provisioned {
        (None, 264)
    } else {
        (
            Some(UiRect {
                left: margin,
                top: 264,
                right: content_right,
                bottom: 420,
            }),
            440,
        )
    };

    let activation_rect = activation.unwrap_or(UiRect {
        left: margin,
        top: 0,
        right: content_right,
        bottom: 0,
    });
    let edit_top = activation_rect.top + 84;
    let available = (activation_rect.width() - 40).max(560);
    let button_width = 148;
    let device_width = 196;
    let gap = 10;
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
        right: content_right,
        bottom: (height - margin).max(logs_top + 210),
    };
    let debug_toggle = UiRect {
        left: logs_card.right - 164,
        top: logs_card.top + 14,
        right: logs_card.right - 16,
        bottom: logs_card.top + 46,
    };
    let logs_edit = UiRect {
        left: logs_card.left + 16,
        top: logs_card.top + 58,
        right: logs_card.right - 16,
        bottom: logs_card.bottom - 16,
    };

    Layout {
        hero,
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

        if let Some(activation) = layout.activation {
            draw_activation(hdc, activation, layout, fonts, &state);
        }

        draw_logs_panel(hdc, layout, fonts, state.debug_enabled);
    }
}

unsafe fn draw_header(hdc: *mut c_void, fonts: Fonts) {
    let icon = UiRect {
        left: 24,
        top: 18,
        right: 74,
        bottom: 68,
    };

    unsafe {
        draw_card(hdc, icon, theme::SURFACE, theme::LINE);
        mascot::draw(hdc, icon.left + 1, icon.top + 1, 48);
        draw_text(hdc, 88, 22, "MNEMOS", fonts.ui, theme::ACCENT);
        draw_text(hdc, 88, 44, "Collector", fonts.section, theme::TEXT);
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
            rect.top + 17,
            "СТАТУС",
            fonts.ui,
            theme::ACCENT,
        );
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 42,
            title,
            fonts.title,
            status_color,
        );
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 78,
            detail,
            fonts.ui,
            theme::TEXT_SECONDARY,
        );
        draw_status_chips(hdc, runtime, rect, fonts.ui);
    }
}

unsafe fn draw_status_chips(
    hdc: *mut c_void,
    runtime: &RuntimeSnapshot,
    rect: UiRect,
    font: *mut c_void,
) {
    let chips_top = rect.bottom - 42;
    let chip_width = 138;
    let gap = 8;
    let mut chip_left = rect.left + 20;

    unsafe {
        draw_status_chip(
            hdc,
            chip_rect(chip_left, chips_top, chip_width),
            if runtime.cristalix_running {
                "Cristalix"
            } else {
                "Ждём Cristalix"
            },
            if runtime.cristalix_running {
                theme::POSITIVE
            } else {
                theme::TEXT_MUTED
            },
            font,
        );
        chip_left += chip_width + gap;

        draw_status_chip(
            hdc,
            chip_rect(chip_left, chips_top, chip_width),
            if runtime.game_mode == "MasterSword" {
                "Master Sword"
            } else {
                "Режим не найден"
            },
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
            if runtime.realtime_connected {
                "Mnemos подключён"
            } else {
                "Нет связи"
            },
            if runtime.realtime_connected {
                theme::POSITIVE
            } else {
                theme::DANGER
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
        bottom: top + 28,
    }
}

fn status_copy(runtime: &RuntimeSnapshot) -> (&'static str, &'static str, u32) {
    if runtime.observing {
        return (
            "Сбор активен",
            "Master Sword распознан. События передаются в Mnemos.",
            theme::ACCENT,
        );
    }

    if runtime.game_mode == "MasterSword" && !runtime.cristalix_running {
        return (
            "Master Sword найден",
            "Ждём активность в игре, чтобы подтвердить текущую сессию.",
            theme::AMBER,
        );
    }

    if !runtime.cristalix_running {
        return (
            "Ожидаем игру",
            "Collector готов и начнёт работу автоматически после запуска Cristalix.",
            theme::TEXT,
        );
    }

    if runtime.game_mode == "MasterSword" && !runtime.realtime_connected {
        return (
            "Подключаемся к Mnemos",
            "Сессия Master Sword активна. Восстанавливаем соединение.",
            theme::AMBER,
        );
    }

    if runtime.game_mode == "MasterSword" {
        return (
            "Подтверждаем сбор",
            "Сессия активна. Завершаем подключение Collector.",
            theme::AMBER,
        );
    }

    (
        "Cristalix активен",
        "Ожидаем переход в Master Sword.",
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
                "Установить Collector"
            },
            fonts.section,
            theme::TEXT,
        );
        draw_text(
            hdc,
            activation.left + 20,
            activation.top + 49,
            "Код активации",
            fonts.ui,
            theme::TEXT_MUTED,
        );
        draw_text(
            hdc,
            layout.device_edit.left,
            activation.top + 49,
            "Устройство",
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
            layout.logs_card.left + 16,
            layout.logs_card.top + 16,
            "Журнал",
            fonts.section,
            theme::TEXT,
        );
        draw_toggle(
            hdc,
            layout.debug_toggle,
            "Диагностика",
            debug_enabled,
            fonts.ui,
        );
    }
}

unsafe fn draw_status_chip(
    hdc: *mut c_void,
    rect: UiRect,
    label: &str,
    status_color: u32,
    font: *mut c_void,
) {
    unsafe {
        draw_pill(hdc, rect, theme::SURFACE_RAISED, theme::LINE);
        draw_dot(hdc, rect.left + 10, rect.top + 10, status_color);
        draw_text(hdc, rect.left + 24, rect.top + 5, label, font, theme::TEXT);
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
        draw_pill(hdc, rect, theme::SURFACE_RAISED, theme::LINE);
        draw_dot(
            hdc,
            rect.left + 12,
            rect.top + 12,
            if enabled {
                theme::ACCENT
            } else {
                theme::TEXT_MUTED
            },
        );
        draw_text(
            hdc,
            rect.left + 28,
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
        draw_pill(hdc, rect, theme::ACCENT, theme::ACCENT);
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

unsafe fn draw_dot(hdc: *mut c_void, x: i32, y: i32, color: u32) {
    let brush = unsafe { CreateSolidBrush(color) };
    let pen = unsafe { CreatePen(0, 1, color) };
    let previous_brush = unsafe { SelectObject(hdc, brush) };
    let previous_pen = unsafe { SelectObject(hdc, pen) };

    unsafe {
        Ellipse(hdc, x, y, x + 7, y + 7);
        SelectObject(hdc, previous_pen);
        SelectObject(hdc, previous_brush);
        DeleteObject(pen);
        DeleteObject(brush);
    }
}

unsafe fn draw_card(hdc: *mut c_void, rect: UiRect, fill: u32, border: u32) {
    unsafe {
        draw_rounded_rect(hdc, rect, fill, border, 30);
    }
}

unsafe fn draw_pill(hdc: *mut c_void, rect: UiRect, fill: u32, border: u32) {
    let radius = rect.height().max(1);

    unsafe {
        draw_rounded_rect(hdc, rect, fill, border, radius);
    }
}

unsafe fn draw_rounded_rect(
    hdc: *mut c_void,
    rect: UiRect,
    fill: u32,
    border: u32,
    radius: i32,
) {
    let brush = unsafe { CreateSolidBrush(fill) };
    let pen = unsafe { CreatePen(0, 1, border) };
    let previous_brush = unsafe { SelectObject(hdc, brush) };
    let previous_pen = unsafe { SelectObject(hdc, pen) };

    unsafe {
        RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        );
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
