use std::ffi::c_void;

use windows_sys::Win32::Foundation::{RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, Ellipse, FillRect, GetTextExtentPoint32W,
    IntersectClipRect, RestoreDC, RoundRect, SaveDC, SelectObject, SetBkMode, SetTextColor,
    TextOutW,
};

use crate::diagnostics::RuntimeSnapshot;

use super::mascot;
use super::theme;

const LOG_LINE_HEIGHT: i32 = 18;
const LOG_CHAR_WIDTH: i32 = 8;
const CONTENT_MARGIN: i32 = 22;
const HEADER_HEIGHT: i32 = 68;
const CARD_RADIUS: i32 = 24;

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

    fn inset(self, horizontal: i32, vertical: i32) -> Self {
        Self {
            left: self.left + horizontal,
            top: self.top + vertical,
            right: self.right - horizontal,
            bottom: self.bottom - vertical,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub title_bar: UiRect,
    pub window_minimize: UiRect,
    pub window_close: UiRect,
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
    let content_right = (width - CONTENT_MARGIN).max(CONTENT_MARGIN + 680);
    let title_bar = UiRect {
        left: 0,
        top: 0,
        right: width,
        bottom: HEADER_HEIGHT,
    };
    let window_close = UiRect {
        left: width - 56,
        top: 13,
        right: width - 18,
        bottom: 47,
    };
    let window_minimize = UiRect {
        left: window_close.left - 46,
        top: 13,
        right: window_close.left - 8,
        bottom: 47,
    };
    let hero_top = HEADER_HEIGHT + 6;
    let hero_height = 154;

    let hero = UiRect {
        left: CONTENT_MARGIN,
        top: hero_top,
        right: content_right,
        bottom: hero_top + hero_height,
    };

    let (activation, logs_top) = if provisioned {
        (None, hero.bottom + 16)
    } else {
        let activation = UiRect {
            left: CONTENT_MARGIN,
            top: hero.bottom + 16,
            right: content_right,
            bottom: hero.bottom + 150,
        };

        (Some(activation), activation.bottom + 16)
    };

    let activation_rect = activation.unwrap_or(UiRect {
        left: CONTENT_MARGIN,
        top: 0,
        right: content_right,
        bottom: 0,
    });
    let edit_top = activation_rect.top + 72;
    let inner_width = (activation_rect.width() - 36).max(540);
    let device_width = 176;
    let button_width = 132;
    let gap = 10;
    let token_width = (inner_width - device_width - button_width - gap * 2).max(210);

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
        left: CONTENT_MARGIN,
        top: logs_top,
        right: content_right,
        bottom: (height - CONTENT_MARGIN).max(logs_top + 190),
    };
    let debug_toggle = UiRect {
        left: logs_card.right - 136,
        top: logs_card.top + 13,
        right: logs_card.right - 14,
        bottom: logs_card.top + 41,
    };
    let logs_view = UiRect {
        left: logs_card.left + 14,
        top: logs_card.top + 52,
        right: logs_card.right - 14,
        bottom: logs_card.bottom - 14,
    };

    Layout {
        title_bar,
        window_minimize,
        window_close,
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
        draw_header(hdc, layout, fonts);
        draw_hero(hdc, runtime, layout, fonts);

        if let Some(activation) = layout.activation {
            draw_activation(hdc, activation, layout, fonts, &state);
        }

        draw_logs_panel(hdc, layout, fonts, &state);
    }
}

unsafe fn draw_header(hdc: *mut c_void, layout: Layout, fonts: Fonts) {
    let icon = UiRect {
        left: CONTENT_MARGIN,
        top: 12,
        right: CONTENT_MARGIN + 44,
        bottom: 56,
    };

    unsafe {
        draw_card_with_radius(hdc, icon, theme::SURFACE, theme::LINE, 18);
        mascot::draw(hdc, icon.left + 2, icon.top + 2, 40);

        draw_text_emphasis(
            hdc,
            UiRect {
                left: 78,
                top: 14,
                right: layout.window_minimize.left - 20,
                bottom: 34,
            },
            "MNEMOS",
            fonts.ui,
            theme::ACCENT,
        );
        draw_text_emphasis(
            hdc,
            UiRect {
                left: 78,
                top: 34,
                right: layout.window_minimize.left - 20,
                bottom: 60,
            },
            "Collector",
            fonts.section,
            theme::TEXT,
        );

        draw_window_button(
            hdc,
            layout.window_minimize,
            "—",
            false,
            fonts.ui,
        );
        draw_window_button(hdc, layout.window_close, "×", true, fonts.ui);
    }
}

unsafe fn draw_hero(hdc: *mut c_void, runtime: &RuntimeSnapshot, layout: Layout, fonts: Fonts) {
    let (title, detail, status_color) = status_copy(runtime);
    let rect = layout.hero;
    let mascot_size = 48;
    let mascot_left = rect.right - 68;
    let text_right = mascot_left - 14;

    unsafe {
        draw_card_with_radius(hdc, rect, theme::SURFACE, theme::LINE, CARD_RADIUS);
        draw_accent_bar(hdc, rect, status_color);

        draw_text_emphasis(
            hdc,
            UiRect {
                left: rect.left + 20,
                top: rect.top + 14,
                right: text_right,
                bottom: rect.top + 34,
            },
            "СТАТУС",
            fonts.ui,
            theme::ACCENT,
        );
        draw_text_clipped(
            hdc,
            UiRect {
                left: rect.left + 20,
                top: rect.top + 35,
                right: text_right,
                bottom: rect.top + 70,
            },
            title,
            fonts.title,
            status_color,
        );
        draw_text_clipped(
            hdc,
            UiRect {
                left: rect.left + 20,
                top: rect.top + 70,
                right: text_right,
                bottom: rect.top + 94,
            },
            detail,
            fonts.ui,
            theme::TEXT_SECONDARY,
        );

        mascot::draw(hdc, mascot_left, rect.top + 17, mascot_size);
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
    let top = rect.bottom - 50;
    let bottom = rect.bottom - 12;

    let game = UiRect {
        left,
        top,
        right: left + tile_width,
        bottom,
    };
    let mode = UiRect {
        left: game.right + gap,
        top,
        right: game.right + gap + tile_width,
        bottom,
    };
    let mnemos = UiRect {
        left: mode.right + gap,
        top,
        right,
        bottom,
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
    let label_rect = UiRect {
        left: rect.left + 24,
        top: rect.top + 4,
        right: rect.right - 10,
        bottom: rect.top + 20,
    };
    let value_rect = UiRect {
        left: rect.left + 12,
        top: rect.top + 19,
        right: rect.right - 10,
        bottom: rect.bottom - 3,
    };

    unsafe {
        draw_card_with_radius(hdc, rect, theme::SURFACE_RAISED, theme::LINE, 18);
        draw_dot(hdc, rect.left + 11, rect.top + 10, status_color);
        draw_text_clipped(hdc, label_rect, label, fonts.ui, theme::TEXT_MUTED);
        draw_text_emphasis(hdc, value_rect, value, fonts.ui, theme::TEXT);
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
        draw_card_with_radius(hdc, activation, theme::SURFACE, theme::LINE, CARD_RADIUS);

        draw_text_emphasis(
            hdc,
            UiRect {
                left: activation.left + 18,
                top: activation.top + 14,
                right: activation.right - 18,
                bottom: activation.top + 40,
            },
            if state.current_installation {
                "Подключить Collector"
            } else {
                "Установить Collector"
            },
            fonts.section,
            theme::TEXT,
        );

        draw_text_clipped(
            hdc,
            UiRect {
                left: activation.left + 18,
                top: activation.top + 46,
                right: layout.token_edit.right,
                bottom: activation.top + 65,
            },
            "Код активации",
            fonts.ui,
            theme::TEXT_MUTED,
        );
        draw_text_clipped(
            hdc,
            UiRect {
                left: layout.device_edit.left,
                top: activation.top + 46,
                right: layout.device_edit.right,
                bottom: activation.top + 65,
            },
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
            draw_text_clipped(
                hdc,
                UiRect {
                    left: activation.left + 18,
                    top: activation.bottom - 23,
                    right: activation.right - 18,
                    bottom: activation.bottom - 5,
                },
                error,
                fonts.ui,
                theme::DANGER,
            );
        }
    }
}

unsafe fn draw_logs_panel(hdc: *mut c_void, layout: Layout, fonts: Fonts, state: &ViewState<'_>) {
    unsafe {
        draw_card_with_radius(
            hdc,
            layout.logs_card,
            theme::SURFACE,
            theme::LINE,
            CARD_RADIUS,
        );

        draw_text_emphasis(
            hdc,
            UiRect {
                left: layout.logs_card.left + 16,
                top: layout.logs_card.top + 13,
                right: layout.debug_toggle.left - 12,
                bottom: layout.logs_card.top + 42,
            },
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
        draw_card_with_radius(hdc, rect, theme::LOG_SURFACE, theme::LINE, 18);
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
            draw_text_clipped(
                hdc,
                UiRect {
                    left: text_rect.left,
                    top: y,
                    right: text_rect.right,
                    bottom: y + LOG_LINE_HEIGHT,
                },
                &line.text,
                font,
                line.color,
            );
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
    let label_rect = UiRect {
        left: rect.left + 25,
        top: rect.top,
        right: rect.right - 8,
        bottom: rect.bottom,
    };

    unsafe {
        draw_pill(hdc, rect, fill, border);
        draw_dot(
            hdc,
            rect.left + 10,
            rect.top + 10,
            if enabled {
                theme::ACCENT
            } else {
                theme::TEXT_MUTED
            },
        );
        draw_text_centered_vertically(
            hdc,
            label_rect,
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
        draw_text_centered(hdc, rect.inset(8, 2), label, font, theme::BACKGROUND_DEEP);
    }
}

unsafe fn draw_window_button(
    hdc: *mut c_void,
    rect: UiRect,
    label: &str,
    danger: bool,
    font: *mut c_void,
) {
    let fill = if danger {
        theme::DANGER_DIM
    } else {
        theme::SURFACE
    };
    let border = if danger { theme::DANGER } else { theme::LINE };
    let text = if danger {
        theme::DANGER
    } else {
        theme::TEXT_SECONDARY
    };

    unsafe {
        draw_pill(hdc, rect, fill, border);
        draw_text_centered(hdc, rect.inset(6, 2), label, font, text);
    }
}

unsafe fn draw_accent_bar(hdc: *mut c_void, rect: UiRect, color: u32) {
    let bar = UiRect {
        left: rect.left + 1,
        top: rect.top + 24,
        right: rect.left + 4,
        bottom: rect.top + 82,
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

unsafe fn draw_text_clipped(
    hdc: *mut c_void,
    rect: UiRect,
    text: &str,
    font: *mut c_void,
    color: u32,
) {
    let text = text.encode_utf16().collect::<Vec<_>>();
    let saved = unsafe { SaveDC(hdc) };
    let previous_font = unsafe { SelectObject(hdc, font) };

    unsafe {
        IntersectClipRect(hdc, rect.left, rect.top, rect.right, rect.bottom);
        SetTextColor(hdc, color);
        SetBkMode(hdc, 1);
        TextOutW(hdc, rect.left, rect.top, text.as_ptr(), text.len() as i32);
        SelectObject(hdc, previous_font);

        if saved != 0 {
            RestoreDC(hdc, saved);
        }
    }
}

unsafe fn draw_text_emphasis(
    hdc: *mut c_void,
    rect: UiRect,
    text: &str,
    font: *mut c_void,
    color: u32,
) {
    unsafe {
        draw_text_clipped(hdc, rect, text, font, color);
        draw_text_clipped(
            hdc,
            UiRect {
                left: rect.left + 1,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            },
            text,
            font,
            color,
        );
    }
}

unsafe fn draw_text_centered(
    hdc: *mut c_void,
    rect: UiRect,
    text: &str,
    font: *mut c_void,
    color: u32,
) {
    let text_utf16 = text.encode_utf16().collect::<Vec<_>>();
    let previous_font = unsafe { SelectObject(hdc, font) };
    let mut size = SIZE { cx: 0, cy: 0 };

    unsafe {
        GetTextExtentPoint32W(hdc, text_utf16.as_ptr(), text_utf16.len() as i32, &mut size);
        SelectObject(hdc, previous_font);
    }

    let x = rect.left + ((rect.width() - size.cx).max(0) / 2);
    let y = rect.top + ((rect.height() - size.cy).max(0) / 2);

    unsafe {
        draw_text_clipped(
            hdc,
            UiRect {
                left: x,
                top: y,
                right: rect.right,
                bottom: rect.bottom,
            },
            text,
            font,
            color,
        );
    }
}

unsafe fn draw_text_centered_vertically(
    hdc: *mut c_void,
    rect: UiRect,
    text: &str,
    font: *mut c_void,
    color: u32,
) {
    let text_utf16 = text.encode_utf16().collect::<Vec<_>>();
    let previous_font = unsafe { SelectObject(hdc, font) };
    let mut size = SIZE { cx: 0, cy: 0 };

    unsafe {
        GetTextExtentPoint32W(hdc, text_utf16.as_ptr(), text_utf16.len() as i32, &mut size);
        SelectObject(hdc, previous_font);
    }

    let y = rect.top + ((rect.height() - size.cy).max(0) / 2);

    unsafe {
        draw_text_clipped(
            hdc,
            UiRect {
                left: rect.left,
                top: y,
                right: rect.right,
                bottom: rect.bottom,
            },
            text,
            font,
            color,
        );
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
