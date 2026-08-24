use std::io;
use std::ptr::null_mut;

use anyhow::{Context, Result};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CBT_CREATEWNDW, CallNextHookEx, GetCursorPos, GetSystemMetrics, HCBT_CREATEWND, HHOOK,
    SM_CXSCREEN, SM_CYSCREEN, SetWindowsHookExW, UnhookWindowsHookEx, WH_CBT,
};

const COLLECTOR_WINDOW_WIDTH: i32 = 1080;
const COLLECTOR_WINDOW_HEIGHT: i32 = 720;

pub(super) struct StartupPlacementHook {
    hook: HHOOK,
}

impl StartupPlacementHook {
    pub(super) fn install() -> Result<Self> {
        let hook = unsafe {
            SetWindowsHookExW(
                WH_CBT,
                Some(startup_placement_hook),
                null_mut(),
                GetCurrentThreadId(),
            )
        };

        if hook.is_null() {
            return Err(io::Error::last_os_error())
                .context("failed to install collector startup placement hook");
        }

        Ok(Self { hook })
    }
}

impl Drop for StartupPlacementHook {
    fn drop(&mut self) {
        if !self.hook.is_null() {
            unsafe {
                UnhookWindowsHookEx(self.hook);
            }
        }
    }
}

unsafe extern "system" fn startup_placement_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HCBT_CREATEWND as i32 {
        unsafe {
            center_collector_creation(lparam);
        }
    }

    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe fn center_collector_creation(lparam: LPARAM) {
    if lparam == 0 {
        return;
    }

    let create_window = unsafe { &mut *(lparam as *mut CBT_CREATEWNDW) };

    if create_window.lpcs.is_null() {
        return;
    }

    let create = unsafe { &mut *create_window.lpcs };

    if !create.hwndParent.is_null()
        || create.cx != COLLECTOR_WINDOW_WIDTH
        || create.cy != COLLECTOR_WINDOW_HEIGHT
    {
        return;
    }

    let mut cursor = POINT { x: 0, y: 0 };

    unsafe {
        GetCursorPos(&mut cursor);
    }

    let work_area = monitor_work_area(cursor);
    let position = centered_position(work_area, create.cx, create.cy);

    create.x = position.x;
    create.y = position.y;
}

fn monitor_work_area(cursor: POINT) -> RECT {
    unsafe {
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);

        if !monitor.is_null() {
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;

            if GetMonitorInfoW(monitor, &mut info) != 0 {
                return info.rcWork;
            }
        }

        RECT {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN),
        }
    }
}

fn centered_position(work_area: RECT, width: i32, height: i32) -> POINT {
    let available_width = work_area.right - work_area.left;
    let available_height = work_area.bottom - work_area.top;

    POINT {
        x: work_area.left + (available_width - width).max(0) / 2,
        y: work_area.top + (available_height - height).max(0) / 2,
    }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Foundation::RECT;

    use super::centered_position;

    #[test]
    fn startup_position_centers_inside_primary_work_area() {
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };

        let position = centered_position(work_area, 1080, 720);

        assert_eq!(position.x, 420);
        assert_eq!(position.y, 160);
    }

    #[test]
    fn startup_position_supports_negative_monitor_coordinates() {
        let work_area = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };

        let position = centered_position(work_area, 1080, 720);

        assert_eq!(position.x, -1500);
        assert_eq!(position.y, 160);
    }

    #[test]
    fn startup_position_stays_at_work_area_origin_when_window_is_larger() {
        let work_area = RECT {
            left: 100,
            top: 50,
            right: 900,
            bottom: 650,
        };

        let position = centered_position(work_area, 1080, 720);

        assert_eq!(position.x, work_area.left);
        assert_eq!(position.y, work_area.top);
    }
}
