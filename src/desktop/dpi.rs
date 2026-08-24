use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED, SetThreadDpiAwarenessContext,
};

pub(super) fn enable_gdi_scaling_for_thread() {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED);
    }
}
