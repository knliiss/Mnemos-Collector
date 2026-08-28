use std::ffi::c_void;

use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Graphics::Gdi::{
    GetTextExtentPoint32W, SelectObject, SetBkMode, SetTextColor, TextOutW,
};

pub(super) use super::base_view::{Fonts, InteractiveElement, Layout, UiRect, ViewState};
use super::{base_view, theme};

const COLLECTOR_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
const VERSION_TOP_MARGIN: i32 = 3;

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
    runtime: &crate::diagnostics::RuntimeSnapshot,
    layout: Layout,
    fonts: Fonts,
    state: ViewState<'_>,
) {
    unsafe {
        base_view::draw(hdc, runtime, layout, fonts, state);
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
    use super::{COLLECTOR_VERSION, layout};

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
}
