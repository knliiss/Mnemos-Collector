use std::ffi::c_void;

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
const UPDATE_BUTTON_RIGHT_GAP: i32 = 12;

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
    let right = layout.window_minimize.left - UPDATE_BUTTON_RIGHT_GAP;

    UiRect {
        left: right - UPDATE_BUTTON_WIDTH,
        top: 13,
        right,
        bottom: 47,
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
        RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            18,
            18,
        );
        SelectObject(hdc, previous_pen);
        SelectObject(hdc, previous_brush);
        DeleteObject(pen);
        DeleteObject(brush);
    }

    draw_centered_text(hdc, rect, &label, font, text_color);
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

    use super::{COLLECTOR_VERSION, layout, update_button_contains, update_button_rect};

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
    fn update_button_stays_clear_of_window_controls() {
        let layout = layout(1080, 720, true);
        let update = update_button_rect(layout);

        assert!(update.right < layout.window_minimize.left);
        assert!(update.left > 200);
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
}
