use std::ffi::c_void;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, Ellipse, FillRect, RoundRect, SelectObject,
    SetBkMode, SetTextColor, TextOutW,
};

use crate::diagnostics::RuntimeSnapshot;

use super::mascot;
use super::theme;

const LOG_LINE_HEIGHT: i32 = 18;
const LOG_CHAR_WIDTH: i32 = 8;

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
    pub logs_view: UiRect,
    pub debug_toggle: UiRect,
}

#[derive(Clone, Copy)]
pub(super) struct Fonts {
    pub ui: *mut c_void,
    pub title: *mut c_void,
    pub section: *mut c_void,
    pub mono: *mut c_void,
}

pub(super) struct ViewState<'a> {
    pub current_installation: bool,
    pub provisioning: bool,
    pub activation_error: Option<&'a str>,
    pub debug_enabled: bool,
    pub log_text: &'a str,
    pub log_scroll_from_bottom: usize,
}

struct LogVisualLine {
    text: String,
    color: u32,
}

pub(super) fn layout(width: i32, height: i32, provisioned: bool) -> Layout {
    let margin = 22;
    let content_right = (width - margin).max(margin + 680);

    let hero = UiRect {
        left: margin,
        top: 76,
        right: content_right,
        bottom: 210,
    };

    let (activation, logs_top) = if provisioned {
        (None, 226)
    } else {
        (
            Some(UiRect {
                left: margin,
                top: 226,
                right: content_right,
                bottom: 356,
            }),
            372,
        )
    };

    let activation_rect = activation.unwrap_or(UiRect {
        left: margin,
        top: 0,
        right: content_right,
        bottom: 0,
    });
    let edit_top = activation_rect.top + 70;
    let available = (activation_rect.width() - 36).max(540);
    let button_width = 138;
    let device_width = 184;
    let gap = 10;
    let token_width = (available - button_width - device_width - gap * 2).max(210);

    let token_edit = UiRect {
        left: activation_rect.left + 18,
        top: edit_top,
        right: activation_rect.left + 18 + token_width,
        bottom: edit_top + 34,
    };
    let device_edit = UiRect {
        left: token_edit.right + gap,
        top: edit_top,
        right: token_edit.right + gap + device_width,
        bottom: edit_top + 34,
    };
    let activate_button = UiRect {
        left: device_edit.right + gap,
        top: edit_top,
        right: activation_rect.right - 18,
        bottom: edit_top + 34,
    };

    let logs_card = UiRect {
        left: margin,
        top: logs_top,
        right: content_right,
        bottom: (height - margin).max(logs_top + 190),
    };
    let debug_toggle = UiRect {
        left: logs_card.right - 138,
        top: logs_card.top + 12,
        right: logs_card.right - 14,
        bottom: logs_card.top + 40,
    };
    let logs_view = UiRect {
        left: logs_card.left + 14,
        top: logs_card.top + 50,
        right: logs_card.right - 14,
        bottom: logs_card.bottom - 14,
    };

    Layout {
        hero,
        activation,
        token_edit,
        device_edit,
        activate_button,
        logs_card,
        logs_view,
        debug_toggle,
    }
}

pub(super) fn log_scroll_limit(text: &str, rect: UiRect) -> usize {
    let max_chars = log_chars_per_line(rect);
    let total_lines = wrapped_log_line_count(text, max_chars);
    let visible_lines = log_visible_line_count(rect);

    total_lines.saturating_sub(visible_lines)
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

        draw_logs_panel(hdc, layout, fonts, &state);
    }
}

unsafe fn draw_header(hdc: *mut c_void, fonts: Fonts) {
    let icon = UiRect {
        left: 22,
        top: 14,
        right: 64,
        bottom: 56,
    };

    unsafe {
        draw_card(hdc, icon, theme::SURFACE, theme::LINE);
        mascot::draw(hdc, icon.left + 1, icon.top + 1, 40);

        draw_text(hdc, 76, 16, "MNEMOS", fonts.ui, theme::ACCENT);
        draw_text(hdc, 76, 36, "Collector", fonts.section, theme::TEXT);
    }
}

unsafe fn draw_hero(hdc: *mut c_void, runtime: &RuntimeSnapshot, layout: Layout, fonts: Fonts) {
    let (title, detail, status_color) = status_copy(runtime);
    let rect = layout.hero;

    unsafe {
        draw_card(hdc, rect, theme::SURFACE, theme::LINE);
        draw_accent_bar(hdc, rect, status_color);

        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 15,
            "СТАТУС",
            fonts.ui,
            theme::ACCENT,
        );
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 36,
            title,
            fonts.title,
            status_color,
        );
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 68,
            detail,
            fonts.ui,
            theme::TEXT_SECONDARY,
        );

        mascot::draw(hdc, rect.right - 72, rect.top + 16, 52);
        draw_status_tiles(hdc, runtime, rect, fonts);
    }
}

unsafe fn draw_status_tiles(
    hdc: *mut c_void,
    runtime: &RuntimeSnapshot,
    rect: UiRect,
    fonts: Fonts,
) {
    let gap = 8;
    let left = rect.left + 20;
    let right = rect.right - 20;
    let available = right - left;
    let tile_width = (available - gap * 2) / 3;
    let top = rect.bottom - 44;

    let game = UiRect {
        left,
        top,
        right: left + tile_width,
        bottom: rect.bottom - 14,
    };
    let mode = UiRect {
        left: game.right + gap,
        top,
        right: game.right + gap + tile_width,
        bottom: rect.bottom - 14,
    };
    let mnemos = UiRect {
        left: mode.right + gap,
        top,
        right,
        bottom: rect.bottom - 14,
    };

    unsafe {
        draw_status_tile(
            hdc,
            game,
            "ИГРА",
            if runtime.cristalix_running {
                "Cristalix"
            } else {
                "Ожидание"
            },
            if runtime.cristalix_running {
                theme::POSITIVE
            } else {
                theme::TEXT_MUTED
            },
            fonts,
        );

        draw_status_tile(
            hdc,
            mode,
            "РЕЖИМ",
            if runtime.game_mode == "MasterSword" {
                "Master Sword"
            } else {
                "Не определён"
            },
            if runtime.game_mode == "MasterSword" {
                theme::ACCENT
            } else {
                theme::AMBER
            },
            fonts,
        );

        draw_status_tile(
            hdc,
            mnemos,
            "MNEMOS",
            if runtime.realtime_connected {
                "Подключён"
            } else {
                "Нет связи"
            },
            if runtime.realtime_connected {
                theme::POSITIVE
            } else {
                theme::DANGER
            },
            fonts,
        );
    }
}

unsafe fn draw_status_tile(
    hdc: *mut c_void,
    rect: UiRect,
    label: &str,
    value: &str,
    status_color: u32,
    fonts: Fonts,
) {
    unsafe {
        draw_card_with_radius(hdc, rect, theme::SURFACE_RAISED, theme::LINE, 18);
        draw_dot(hdc, rect.left + 11, rect.top + 11, status_color);
        draw_text(
            hdc,
            rect.left + 23,
            rect.top + 4,
            label,
            fonts.ui,
            theme::TEXT_MUTED,
        );
        draw_text(
            hdc,
            rect.left + 84,
            rect.top + 4,
            value,
            fonts.ui,
            theme::TEXT,
        );
    }
}

fn status_copy(runtime: &RuntimeSnapshot) -> (&'static str, &'static str, u32) {
    if runtime.observing {
        return (
            "Сбор активен",
            "Master Sword распознан. Новые события отправляются в Mnemos.",
            theme::ACCENT,
        );
    }

    if runtime.game_mode == "MasterSword" && !runtime.cristalix_running {
        return (
            "Master Sword найден",
            "Ждём свежую активность в логе, чтобы подтвердить текущую сессию.",
            theme::AMBER,
        );
    }

    if !runtime.cristalix_running {
        return (
            "Ожидаем Cristalix",
            "Collector готов и сам подхватит игру после появления свежего лога.",
            theme::TEXT,
        );
    }

    if runtime.game_mode == "MasterSword" && !runtime.realtime_connected {
        return (
            "Подключаем Mnemos",
            "Master Sword активен. Восстанавливаем соединение.",
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
            activation.left + 18,
            activation.top + 15,
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
            activation.left + 18,
            activation.top + 45,
            "Код активации",
            fonts.ui,
            theme::TEXT_MUTED,
        );
        draw_text(
            hdc,
            layout.device_edit.left,
            activation.top + 45,
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
                activation.left + 18,
                activation.bottom - 22,
                error,
                fonts.ui,
                theme::DANGER,
            );
        }
    }
}

unsafe fn draw_logs_panel(hdc: *mut c_void, layout: Layout, fonts: Fonts, state: &ViewState<'_>) {
    unsafe {
        draw_card(hdc, layout.logs_card, theme::SURFACE, theme::LINE);

        draw_text(
            hdc,
            layout.logs_card.left + 16,
            layout.logs_card.top + 14,
            "Журнал",
            fonts.section,
            theme::TEXT,
        );

        draw_toggle(
            hdc,
            layout.debug_toggle,
            "Диагностика",
            state.debug_enabled,
            fonts.ui,
        );

        draw_log_view(
            hdc,
            layout.logs_view,
            state.log_text,
            state.log_scroll_from_bottom,
            fonts.mono,
        );
    }
}

unsafe fn draw_log_view(
    hdc: *mut c_void,
    rect: UiRect,
    text: &str,
    scroll_from_bottom: usize,
    font: *mut c_void,
) {
    unsafe {
        draw_card_with_radius(hdc, rect, theme::LOG_SURFACE, theme::LINE, 16);
    }

    let text_rect = UiRect {
        left: rect.left + 12,
        top: rect.top + 10,
        right: rect.right - 22,
        bottom: rect.bottom - 10,
    };
    let max_chars = log_chars_per_line(text_rect);
    let lines = wrapped_log_lines(text, max_chars);
    let visible_count = log_visible_line_count(text_rect);
    let total = lines.len();
    let clamped_scroll = scroll_from_bottom.min(total.saturating_sub(visible_count));
    let end = total.saturating_sub(clamped_scroll);
    let start = end.saturating_sub(visible_count);
    let mut y = text_rect.top;

    for line in &lines[start..end] {
        unsafe {
            draw_text(hdc, text_rect.left, y, &line.text, font, line.color);
        }
        y += LOG_LINE_HEIGHT;
    }

    unsafe {
        draw_log_scrollbar(hdc, rect, total, visible_count, start);
    }
}

unsafe fn draw_log_scrollbar(
    hdc: *mut c_void,
    rect: UiRect,
    total_lines: usize,
    visible_lines: usize,
    start_line: usize,
) {
    if total_lines <= visible_lines || visible_lines == 0 {
        return;
    }

    let track = UiRect {
        left: rect.right - 10,
        top: rect.top + 10,
        right: rect.right - 6,
        bottom: rect.bottom - 10,
    };
    let track_height = track.height().max(1);
    let thumb_height = ((track_height as f32 * visible_lines as f32 / total_lines as f32) as i32)
        .clamp(24, track_height);
    let max_start = total_lines.saturating_sub(visible_lines).max(1);
    let travel = (track_height - thumb_height).max(0);
    let thumb_top =
        track.top + ((travel as f32 * start_line as f32 / max_start as f32) as i32).min(travel);

    let thumb = UiRect {
        left: track.left,
        top: thumb_top,
        right: track.right,
        bottom: thumb_top + thumb_height,
    };

    unsafe {
        draw_pill(hdc, track, theme::LINE, theme::LINE);
        draw_pill(hdc, thumb, theme::TEXT_MUTED, theme::TEXT_MUTED);
    }
}

fn wrapped_log_lines(text: &str, max_chars: usize) -> Vec<LogVisualLine> {
    let mut output = Vec::new();

    for logical_line in text.lines() {
        let color = log_line_color(logical_line);
        let chars = logical_line.chars().collect::<Vec<_>>();

        if chars.is_empty() {
            output.push(LogVisualLine {
                text: String::new(),
                color,
            });
            continue;
        }

        for chunk in chars.chunks(max_chars.max(1)) {
            output.push(LogVisualLine {
                text: chunk.iter().collect(),
                color,
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

fn log_line_color(line: &str) -> u32 {
    if line.contains("[ERROR]") {
        return theme::DANGER;
    }

    if line.contains("[WARN") {
        return theme::AMBER;
    }

    if line.contains("[DEBUG]") {
        return theme::TEXT_MUTED;
    }

    theme::TEXT_SECONDARY
}

fn log_chars_per_line(rect: UiRect) -> usize {
    ((rect.width().max(LOG_CHAR_WIDTH) / LOG_CHAR_WIDTH) as usize).max(20)
}

fn log_visible_line_count(rect: UiRect) -> usize {
    ((rect.height().max(LOG_LINE_HEIGHT) / LOG_LINE_HEIGHT) as usize).max(1)
}

unsafe fn draw_toggle(
    hdc: *mut c_void,
    rect: UiRect,
    label: &str,
    enabled: bool,
    font: *mut c_void,
) {
    let fill = if enabled {
        theme::ACCENT_DIM
    } else {
        theme::SURFACE_RAISED
    };
    let border = if enabled { theme::ACCENT } else { theme::LINE };

    unsafe {
        draw_pill(hdc, rect, fill, border);
        draw_dot(
            hdc,
            rect.left + 11,
            rect.top + 10,
            if enabled {
                theme::ACCENT
            } else {
                theme::TEXT_MUTED
            },
        );
        draw_text(
            hdc,
            rect.left + 26,
            rect.top + 5,
            label,
            font,
            if enabled {
                theme::TEXT
            } else {
                theme::TEXT_SECONDARY
            },
        );
    }
}

unsafe fn draw_primary_button(hdc: *mut c_void, rect: UiRect, label: &str, font: *mut c_void) {
    unsafe {
        draw_pill(hdc, rect, theme::ACCENT, theme::ACCENT);
        draw_text(
            hdc,
            rect.left + 14,
            rect.top + 6,
            label,
            font,
            theme::BACKGROUND_DEEP,
        );
    }
}

unsafe fn draw_accent_bar(hdc: *mut c_void, rect: UiRect, color: u32) {
    let bar = UiRect {
        left: rect.left + 1,
        top: rect.top + 22,
        right: rect.left + 4,
        bottom: rect.top + 76,
    };

    unsafe {
        draw_pill(hdc, bar, color, color);
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
        draw_card_with_radius(hdc, rect, fill, border, 22);
    }
}

unsafe fn draw_pill(hdc: *mut c_void, rect: UiRect, fill: u32, border: u32) {
    let radius = rect.height().max(1);

    unsafe {
        draw_card_with_radius(hdc, rect, fill, border, radius);
    }
}

unsafe fn draw_card_with_radius(
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
        FillRect(hdc, client, background);
        DeleteObject(background);
        SetBkMode(hdc, 1);
    }
}
