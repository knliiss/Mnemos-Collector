use std::ffi::c_void;

use chrono::Utc;
use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, GetTextExtentPoint32W, RoundRect, SelectObject,
    SetBkMode, SetTextColor, TextOutW,
};

use crate::diagnostics::RuntimeSnapshot;

pub(super) use super::base_view::{Fonts, InteractiveElement, Layout, UiRect, ViewState};
use super::{base_view, theme};

const COLLECTOR_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
const VERSION_TOP_MARGIN: i32 = 3;
const UPDATE_BUTTON_WIDTH: i32 = 184;
const UPDATE_BUTTON_GAP: i32 = 10;
const DIAGNOSTICS_LEFT_OFFSET: i32 = 178;
const DIAGNOSTICS_RIGHT_GAP: i32 = 8;

pub(super) fn layout(width: i32, height: i32, provisioned: bool) -> Layout {
    base_view::layout(width, height, provisioned)
}

pub(super) fn interactive_element_at(
    layout: Layout,
    provisioned: bool,
    x: i32,
    y: i32,
) -> Option<InteractiveElement> {
    base_view::interactive_element_at(layout, provisioned, x, y)
}

pub(super) fn update_button_contains(
    layout: Layout,
    runtime: &RuntimeSnapshot,
    x: i32,
    y: i32,
) -> bool {
    runtime.available_update_version.is_some()
        && !runtime.update_installing
        && update_button_rect(layout).contains(x, y)
}

pub(super) fn log_scroll_limit(text: &str, rect: UiRect) -> usize {
    base_view::log_scroll_limit(text, rect)
}

pub(super) fn log_entry_at(
    text: &str,
    rect: UiRect,
    scroll_from_bottom: usize,
    x: i32,
    y: i32,
) -> Option<usize> {
    base_view::log_entry_at(text, rect, scroll_from_bottom, x, y)
}

pub(super) fn log_entry_text(text: &str, entry_index: usize) -> Option<&str> {
    base_view::log_entry_text(text, entry_index)
}

pub(super) unsafe fn draw(
    hdc: *mut c_void,
    runtime: &RuntimeSnapshot,
    layout: Layout,
    fonts: Fonts,
    state: ViewState<'_>,
) {
    unsafe {
        base_view::draw(hdc, runtime, layout, fonts, state);
        draw_diagnostics_summary(hdc, runtime, layout, fonts.ui);
        draw_update_button(hdc, runtime, layout, fonts.ui);
        draw_version(hdc, layout, fonts.ui);
    }
}

pub(super) unsafe fn fill_background(
    hdc: *mut c_void,
    client: &windows_sys::Win32::Foundation::RECT,
) {
    unsafe {
        base_view::fill_background(hdc, client);
    }
}

fn update_button_rect(layout: Layout) -> UiRect {
    let right = layout.copy_logs.left - UPDATE_BUTTON_GAP;

    UiRect {
        left: right - UPDATE_BUTTON_WIDTH,
        top: layout.copy_logs.top,
        right,
        bottom: layout.copy_logs.bottom,
    }
}

fn diagnostics_summary_rect(layout: Layout, runtime: &RuntimeSnapshot) -> UiRect {
    let right = if runtime.available_update_version.is_some() {
        update_button_rect(layout).left - DIAGNOSTICS_RIGHT_GAP
    } else {
        layout.copy_logs.left - DIAGNOSTICS_RIGHT_GAP
    };

    UiRect {
        left: layout.logs_card.left + DIAGNOSTICS_LEFT_OFFSET,
        top: layout.copy_logs.top,
        right,
        bottom: layout.copy_logs.bottom,
    }
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

fn diagnostics_summary_color(runtime: &RuntimeSnapshot) -> u32 {
    if runtime.required_update_version.is_some() {
        return theme::DANGER;
    }

    if runtime.spool_capacity > 0
        && runtime.spool_pending.saturating_mul(10) >= runtime.spool_capacity.saturating_mul(9)
    {
        return theme::AMBER;
    }

    theme::TEXT_MUTED
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

unsafe fn draw_diagnostics_summary(
    hdc: *mut c_void,
    runtime: &RuntimeSnapshot,
    layout: Layout,
    font: *mut c_void,
) {
    let rect = diagnostics_summary_rect(layout, runtime);

    if rect.width() < 40 {
        return;
    }

    unsafe {
        draw_clipped_text(
            hdc,
            rect,
            &diagnostics_summary(runtime),
            font,
            diagnostics_summary_color(runtime),
        );
    }
}

unsafe fn draw_update_button(
    hdc: *mut c_void,
    runtime: &RuntimeSnapshot,
    layout: Layout,
    font: *mut c_void,
) {
    let Some(version) = runtime.available_update_version.as_deref() else {
        return;
    };
    let rect = update_button_rect(layout);
    let (background, border, text_color, label) = if runtime.update_installing {
        (
            theme::SURFACE_RAISED,
            theme::LINE_STRONG,
            theme::TEXT_MUTED,
            "ОБНОВЛЕНИЕ...".to_owned(),
        )
    } else {
        (
            theme::ACCENT_DIM,
            theme::ACCENT,
            theme::ACCENT,
            format!("ОБНОВИТЬ ДО v{version}"),
        )
    };
    let brush = unsafe { CreateSolidBrush(background) };
    let pen = unsafe { CreatePen(0, 1, border) };
    let previous_brush = unsafe { SelectObject(hdc, brush) };
    let previous_pen = unsafe { SelectObject(hdc, pen) };

    unsafe {
        RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, 18, 18);
        SelectObject(hdc, previous_pen);
        SelectObject(hdc, previous_brush);
        DeleteObject(pen);
        DeleteObject(brush);
        draw_centered_text(hdc, rect, &label, font, text_color);
    }
}

unsafe fn draw_centered_text(
    hdc: *mut c_void,
    rect: UiRect,
    value: &str,
    font: *mut c_void,
    color: u32,
) {
    let text = value.encode_utf16().collect::<Vec<_>>();
    let previous_font = unsafe { SelectObject(hdc, font) };
    let mut size = SIZE { cx: 0, cy: 0 };

    unsafe {
        GetTextExtentPoint32W(hdc, text.as_ptr(), text.len() as i32, &mut size);
        SetTextColor(hdc, color);
        SetBkMode(hdc, 1);
        TextOutW(
            hdc,
            rect.left + (rect.width() - size.cx) / 2,
            rect.top + (rect.height() - size.cy) / 2,
            text.as_ptr(),
            text.len() as i32,
        );
        SelectObject(hdc, previous_font);
    }
}

unsafe fn draw_clipped_text(
    hdc: *mut c_void,
    rect: UiRect,
    value: &str,
    font: *mut c_void,
    color: u32,
) {
    let mut rendered = value.to_owned();
    let previous_font = unsafe { SelectObject(hdc, font) };
    let mut size = SIZE { cx: 0, cy: 0 };

    loop {
        let text = rendered.encode_utf16().collect::<Vec<_>>();

        unsafe {
            GetTextExtentPoint32W(hdc, text.as_ptr(), text.len() as i32, &mut size);
        }

        if size.cx <= rect.width() || rendered.chars().count() <= 4 {
            unsafe {
                SetTextColor(hdc, color);
                SetBkMode(hdc, 1);
                TextOutW(
                    hdc,
                    rect.left,
                    rect.top + (rect.height() - size.cy) / 2,
                    text.as_ptr(),
                    text.len() as i32,
                );
                SelectObject(hdc, previous_font);
            }
            return;
        }

        rendered.pop();

        while !rendered.is_char_boundary(rendered.len()) {
            rendered.pop();
        }

        rendered = format!("{}…", rendered.trim_end_matches('…'));
    }
}

unsafe fn draw_version(hdc: *mut c_void, layout: Layout, font: *mut c_void) {
    let text = COLLECTOR_VERSION.encode_utf16().collect::<Vec<_>>();
    let previous_font = unsafe { SelectObject(hdc, font) };
    let mut size = SIZE { cx: 0, cy: 0 };

    unsafe {
        GetTextExtentPoint32W(hdc, text.as_ptr(), text.len() as i32, &mut size);
        SetTextColor(hdc, theme::TEXT_MUTED);
        SetBkMode(hdc, 1);
        TextOutW(
            hdc,
            layout.logs_card.right - size.cx,
            layout.logs_card.bottom + VERSION_TOP_MARGIN,
            text.as_ptr(),
            text.len() as i32,
        );
        SelectObject(hdc, previous_font);
    }
}

#[cfg(test)]
mod tests {
    use crate::diagnostics::RuntimeSnapshot;

    use super::{
        COLLECTOR_VERSION, diagnostics_summary, layout, update_button_contains,
        update_button_rect,
    };

    #[test]
    fn version_uses_short_release_format() {
        assert!(COLLECTOR_VERSION.starts_with('v'));
        assert_eq!(COLLECTOR_VERSION, concat!("v", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn version_footer_stays_below_journal() {
        let layout = layout(1080, 720, true);

        assert_eq!(layout.logs_card.bottom, 698);
        assert!(layout.logs_card.bottom + 3 < 720);
    }

    #[test]
    fn update_button_aligns_with_journal_actions() {
        let layout = layout(1080, 720, true);
        let update = update_button_rect(layout);

        assert!(update.right < layout.copy_logs.left);
        assert_eq!(update.top, layout.copy_logs.top);
        assert_eq!(update.bottom, layout.copy_logs.bottom);
        assert!(update.left > layout.logs_card.left);
    }

    #[test]
    fn update_button_is_clickable_only_for_available_idle_update() {
        let layout = layout(1080, 720, true);
        let rect = update_button_rect(layout);
        let x = (rect.left + rect.right) / 2;
        let y = (rect.top + rect.bottom) / 2;
        let mut runtime = RuntimeSnapshot::default();

        assert!(!update_button_contains(layout, &runtime, x, y));

        runtime.available_update_version = Some("0.1.4".to_owned());
        assert!(update_button_contains(layout, &runtime, x, y));

        runtime.update_installing = true;
        assert!(!update_button_contains(layout, &runtime, x, y));
    }

    #[test]
    fn diagnostics_summary_surfaces_queue_and_protocol() {
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

    #[test]
    fn diagnostics_summary_prioritizes_forced_update() {
        let runtime = RuntimeSnapshot {
            required_update_version: Some("0.2.0".to_owned()),
            ..RuntimeSnapshot::default()
        };

        assert_eq!(
            diagnostics_summary(&runtime),
            "ТРЕБУЕТСЯ ОБНОВЛЕНИЕ ДО v0.2.0"
        );
    }
}
