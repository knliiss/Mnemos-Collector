use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};

use anyhow::{Context, Result};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, EndPaint, FillRect,
    InvalidateRect, PAINTSTRUCT, RoundRect, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
    TextOutW, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowW, GWLP_USERDATA,
    GetCursorPos, GetSystemMetrics, GetWindowLongPtrW, IDC_ARROW, LoadCursorW, RegisterClassW,
    SM_CXSCREEN, SM_CYSCREEN, SW_RESTORE, SW_SHOWNOACTIVATE, SetCursor, SetForegroundWindow,
    SetWindowLongPtrW, ShowWindow, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCCREATE,
    WM_NCDESTROY, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::mascot;
use super::theme;

const CLASS_NAME: &str = "MnemosCollectorTrayPopup";
const WIDTH: i32 = 144;
const HEIGHT: i32 = 74;
const MARGIN: i32 = 6;
const ITEM_HEIGHT: i32 = 29;
const ITEM_GAP: i32 = 4;
const POPUP_RADIUS: i32 = 24;
const ITEM_RADIUS: i32 = 17;

struct TrayPopupState {
    owner: HWND,
    font: *mut c_void,
    hover: Option<TrayItem>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrayItem {
    Open,
    Exit,
}

pub(super) fn show(owner: HWND, font: *mut c_void) -> Result<()> {
    unsafe {
        let instance = GetModuleHandleW(null());

        if instance.is_null() {
            return Err(io::Error::last_os_error())
                .context("failed to get tray popup module handle");
        }

        let class_name = wide(CLASS_NAME);
        let existing = FindWindowW(class_name.as_ptr(), null());

        if !existing.is_null() {
            DestroyWindow(existing);
        }

        let cursor = LoadCursorW(null_mut(), IDC_ARROW);
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: cursor,
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };

        if RegisterClassW(&window_class) == 0 {
            let error = io::Error::last_os_error();

            if error.raw_os_error() != Some(1410) {
                return Err(error).context("failed to register tray popup class");
            }
        }

        let mut cursor_position = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor_position);

        let screen_width = GetSystemMetrics(SM_CXSCREEN);
        let screen_height = GetSystemMetrics(SM_CYSCREEN);
        let x = (cursor_position.x - WIDTH).clamp(8, (screen_width - WIDTH - 8).max(8));
        let y = (cursor_position.y - HEIGHT).clamp(8, (screen_height - HEIGHT - 8).max(8));

        let state = Box::new(TrayPopupState {
            owner,
            font,
            hover: None,
        });
        let state_ptr = Box::into_raw(state);

        let popup = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class_name.as_ptr(),
            null(),
            WS_POPUP,
            x,
            y,
            WIDTH,
            HEIGHT,
            null_mut(),
            null_mut(),
            instance,
            state_ptr.cast::<c_void>(),
        );

        if popup.is_null() {
            drop(Box::from_raw(state_ptr));
            return Err(io::Error::last_os_error()).context("failed to create tray popup");
        }

        let region = CreateRoundRectRgn(0, 0, WIDTH + 1, HEIGHT + 1, POPUP_RADIUS, POPUP_RADIUS);

        if !region.is_null() {
            SetWindowRgn(popup, region, 1);
        }

        ShowWindow(popup, SW_SHOWNOACTIVATE);
        UpdateWindow(popup);
        SetCapture(popup);

        if !cursor.is_null() {
            SetCursor(cursor);
        }
    }

    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        let state = unsafe { (*create).lpCreateParams as *mut TrayPopupState };

        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        }
    }

    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayPopupState };

    match message {
        WM_PAINT if !state.is_null() => {
            unsafe {
                paint(hwnd, &*state);
            }
            return 0;
        }
        WM_ERASEBKGND => return 1,
        WM_SETCURSOR => {
            unsafe {
                let cursor = LoadCursorW(null_mut(), IDC_ARROW);

                if !cursor.is_null() {
                    SetCursor(cursor);
                }
            }

            return 1;
        }
        WM_MOUSEMOVE if !state.is_null() => {
            let x = low_word(lparam as usize) as i16 as i32;
            let y = high_word(lparam as usize) as i16 as i32;
            let hover = hit_test(x, y);

            unsafe {
                if (*state).hover != hover {
                    (*state).hover = hover;
                    InvalidateRect(hwnd, null(), 0);
                }
            }

            return 0;
        }
        WM_LBUTTONDOWN if !state.is_null() => {
            let x = low_word(lparam as usize) as i16 as i32;
            let y = high_word(lparam as usize) as i16 as i32;
            let item = hit_test(x, y);
            let owner = unsafe { (*state).owner };

            unsafe {
                ReleaseCapture();
                DestroyWindow(hwnd);
            }

            match item {
                Some(TrayItem::Open) => unsafe {
                    ShowWindow(owner, SW_RESTORE);
                    SetForegroundWindow(owner);
                },
                Some(TrayItem::Exit) => unsafe {
                    DestroyWindow(owner);
                },
                None => {}
            }

            return 0;
        }
        WM_RBUTTONDOWN => {
            unsafe {
                ReleaseCapture();
                DestroyWindow(hwnd);
            }
            return 0;
        }
        WM_NCDESTROY => {
            unsafe {
                ReleaseCapture();

                if !state.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(state));
                }
            }

            return 0;
        }
        _ => {}
    }

    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

unsafe fn paint(hwnd: HWND, state: &TrayPopupState) {
    let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
    let client = RECT {
        left: 0,
        top: 0,
        right: WIDTH,
        bottom: HEIGHT,
    };
    let background = unsafe { CreateSolidBrush(theme::SURFACE) };

    unsafe {
        FillRect(hdc, &client, background);
        DeleteObject(background);

        draw_popup_border(hdc, client);
        draw_item(hdc, open_rect(), TrayItem::Open, state.hover, state.font);
        draw_item(hdc, exit_rect(), TrayItem::Exit, state.hover, state.font);

        EndPaint(hwnd, &paint);
    }
}

unsafe fn draw_popup_border(hdc: *mut c_void, rect: RECT) {
    let brush = unsafe { CreateSolidBrush(theme::SURFACE) };
    let pen = unsafe { CreatePen(0, 1, theme::LINE_STRONG) };
    let previous_brush = unsafe { SelectObject(hdc, brush) };
    let previous_pen = unsafe { SelectObject(hdc, pen) };

    unsafe {
        RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right - 1,
            rect.bottom - 1,
            POPUP_RADIUS,
            POPUP_RADIUS,
        );
        SelectObject(hdc, previous_pen);
        SelectObject(hdc, previous_brush);
        DeleteObject(pen);
        DeleteObject(brush);
    }
}

unsafe fn draw_item(
    hdc: *mut c_void,
    rect: RECT,
    item: TrayItem,
    hover: Option<TrayItem>,
    font: *mut c_void,
) {
    if hover == Some(item) {
        let fill = if item == TrayItem::Exit {
            theme::DANGER_DIM
        } else {
            theme::SURFACE_RAISED
        };
        let border = if item == TrayItem::Exit {
            theme::DANGER
        } else {
            theme::LINE_STRONG
        };
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
                ITEM_RADIUS,
                ITEM_RADIUS,
            );
            SelectObject(hdc, previous_pen);
            SelectObject(hdc, previous_brush);
            DeleteObject(pen);
            DeleteObject(brush);
        }
    }

    unsafe {
        SetBkMode(hdc, 1);
        SelectObject(hdc, font);
    }

    match item {
        TrayItem::Open => unsafe {
            mascot::draw(hdc, rect.left + 7, rect.top + 5, 18);
            draw_text(hdc, rect.left + 31, rect.top + 5, "Открыть", theme::TEXT);
        },
        TrayItem::Exit => unsafe {
            draw_text(hdc, rect.left + 10, rect.top + 4, "×", theme::DANGER);
            draw_text(hdc, rect.left + 31, rect.top + 5, "Выйти", theme::DANGER);
        },
    }
}

unsafe fn draw_text(hdc: *mut c_void, x: i32, y: i32, text: &str, color: u32) {
    let text = text.encode_utf16().collect::<Vec<_>>();

    unsafe {
        SetTextColor(hdc, color);
        TextOutW(hdc, x, y, text.as_ptr(), text.len() as i32);
    }
}

fn hit_test(x: i32, y: i32) -> Option<TrayItem> {
    if contains(open_rect(), x, y) {
        return Some(TrayItem::Open);
    }

    if contains(exit_rect(), x, y) {
        return Some(TrayItem::Exit);
    }

    None
}

fn open_rect() -> RECT {
    RECT {
        left: MARGIN,
        top: MARGIN,
        right: WIDTH - MARGIN,
        bottom: MARGIN + ITEM_HEIGHT,
    }
}

fn exit_rect() -> RECT {
    RECT {
        left: MARGIN,
        top: MARGIN + ITEM_HEIGHT + ITEM_GAP,
        right: WIDTH - MARGIN,
        bottom: HEIGHT - MARGIN,
    }
}

fn contains(rect: RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn low_word(value: usize) -> u16 {
    (value & 0xffff) as u16
}

fn high_word(value: usize) -> u16 {
    ((value >> 16) & 0xffff) as u16
}
