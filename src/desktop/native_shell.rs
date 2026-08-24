use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};

use anyhow::{Context, Result};
use tokio::runtime::Handle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, InvalidateRect, PAINTSTRUCT,
    ScreenToClient, SetBkColor, SetTextColor, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow,
    DispatchMessageW, ES_AUTOHSCROLL, ES_PASSWORD, GWLP_USERDATA, GetClientRect, GetMessageW,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT,
    HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, LoadCursorW,
    MINMAXINFO, MSG, MessageBoxW, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassW,
    SIZE_MINIMIZED, SW_HIDE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, SendMessageW, SetForegroundWindow,
    SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, WM_APP, WM_CLOSE,
    WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_NCCREATE, WM_NCHITTEST, WM_PAINT, WM_RBUTTONUP, WM_SETFONT, WM_SIZE,
    WM_TIMER, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP,
    WS_SYSMENU, WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE,
};
use zeroize::Zeroizing;

use crate::application::CollectorApplication;
use crate::diagnostics::{self, RuntimeSnapshot};
use crate::platform::{Autostart, Installation};
use crate::provisioning::{ProvisioningClient, default_device_name};
use crate::security::CredentialStore;

use super::DesktopLaunchContext;
use super::mascot;
use super::theme;
use super::tray_popup;
use super::view::{self, Fonts, InteractiveElement, Layout, ViewState};

const CLASS_NAME: &str = "MnemosCollectorShell";
const WINDOW_TITLE: &str = "Mnemos Collector";
const TIMER_REFRESH: usize = 1;
const REFRESH_INTERVAL_MS: u32 = 750;
const WM_SETICON_MESSAGE: u32 = 0x0080;
const ICON_SMALL_VALUE: usize = 0;
const ICON_BIG_VALUE: usize = 1;
const WM_TRAY: u32 = WM_APP + 1;
const WM_ACTIVATION_RESULT: u32 = WM_APP + 2;
const WM_COLLECTOR_STOPPED: u32 = WM_APP + 3;
const TRAY_ID: u32 = 1;
const RESIZE_BORDER: i32 = 7;
const MIN_WINDOW_WIDTH: i32 = 760;
const MIN_PROVISIONED_WINDOW_HEIGHT: i32 = 460;
const MIN_ACTIVATION_WINDOW_HEIGHT: i32 = 610;
const DWM_WINDOW_CORNER_PREFERENCE_ATTRIBUTE: u32 = 33;
const DWM_WINDOW_CORNER_PREFERENCE_ROUND: u32 = 2;

pub fn run(context: DesktopLaunchContext, runtime: Handle) -> Result<()> {
    unsafe {
        let instance = GetModuleHandleW(null());

        if instance.is_null() {
            return Err(io::Error::last_os_error())
                .context("failed to get collector module handle");
        }

        let app_icon = mascot::create_icon(32);

        if app_icon.is_null() {
            return Err(io::Error::last_os_error()).context("failed to create collector app icon");
        }

        let class_name = wide(CLASS_NAME);
        let cursor = LoadCursorW(null_mut(), IDC_ARROW);
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: cursor,
            hIcon: app_icon,
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };

        if RegisterClassW(&window_class) == 0 {
            let error = io::Error::last_os_error();

            if error.raw_os_error() != Some(1410) {
                DestroyIcon(app_icon);
                return Err(error).context("failed to register collector window class");
            }
        }

        let state = Box::new(DesktopWindow::new(context, runtime, app_icon));
        let state_ptr = Box::into_raw(state);
        let title = wide(WINDOW_TITLE);
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP
                | WS_THICKFRAME
                | WS_SYSMENU
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            820,
            610,
            null_mut(),
            null_mut(),
            instance,
            state_ptr.cast::<c_void>(),
        );

        if hwnd.is_null() {
            drop(Box::from_raw(state_ptr));
            return Err(io::Error::last_os_error()).context("failed to create collector window");
        }

        apply_window_chrome(hwnd);
        SendMessageW(
            hwnd,
            WM_SETICON_MESSAGE,
            ICON_SMALL_VALUE,
            app_icon as isize,
        );
        SendMessageW(hwnd, WM_SETICON_MESSAGE, ICON_BIG_VALUE, app_icon as isize);

        (*state_ptr).initialize_controls(hwnd, instance)?;
        (*state_ptr).install_tray_icon(hwnd)?;
        (*state_ptr).start_collector_if_ready(hwnd);

        SetTimer(hwnd, TIMER_REFRESH, REFRESH_INTERVAL_MS, None);
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut message: MSG = std::mem::zeroed();

        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        drop(Box::from_raw(state_ptr));
    }

    Ok(())
}

pub fn show_fatal_error(message: &str) {
    let title = wide("Mnemos Collector — ошибка");
    let message = wide(message);

    unsafe {
        MessageBoxW(null_mut(), message.as_ptr(), title.as_ptr(), 0x00000010);
    }
}

struct DesktopWindow {
    runtime: Handle,
    current_installation: bool,
    access_key: Option<String>,
    provisioned: bool,
    provisioning: bool,
    worker_started: bool,
    activation_error: Option<String>,
    token_edit: HWND,
    device_edit: HWND,
    ui_font: *mut c_void,
    title_font: *mut c_void,
    section_font: *mut c_void,
    mono_font: *mut c_void,
    edit_brush: *mut c_void,
    app_icon: *mut c_void,
    hovered: Option<InteractiveElement>,
    tracking_mouse_leave: bool,
    last_log_text: String,
    log_scroll_from_bottom: usize,
    last_runtime: RuntimeSnapshot,
}

impl DesktopWindow {
    fn new(context: DesktopLaunchContext, runtime: Handle, app_icon: *mut c_void) -> Self {
        Self {
            runtime,
            current_installation: context.current_installation,
            provisioned: context.access_key.is_some(),
            access_key: context.access_key,
            provisioning: false,
            worker_started: false,
            activation_error: None,
            token_edit: null_mut(),
            device_edit: null_mut(),
            ui_font: null_mut(),
            title_font: null_mut(),
            section_font: null_mut(),
            mono_font: null_mut(),
            edit_brush: null_mut(),
            app_icon,
            hovered: None,
            tracking_mouse_leave: false,
            last_log_text: String::new(),
            log_scroll_from_bottom: 0,
            last_runtime: diagnostics::runtime_snapshot(),
        }
    }

    unsafe fn initialize_controls(&mut self, hwnd: HWND, instance: *mut c_void) -> Result<()> {
        let ui = wide("Segoe UI");
        let display = wide("Segoe UI");
        let mono = wide("Cascadia Mono");

        self.ui_font =
            unsafe { CreateFontW(-16, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, ui.as_ptr()) };
        self.title_font =
            unsafe { CreateFontW(-29, 0, 0, 0, 750, 0, 0, 0, 1, 0, 0, 5, 0, display.as_ptr()) };
        self.section_font =
            unsafe { CreateFontW(-21, 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, display.as_ptr()) };
        self.mono_font =
            unsafe { CreateFontW(-14, 0, 0, 0, 500, 0, 0, 0, 1, 0, 0, 5, 0, mono.as_ptr()) };
        self.edit_brush = unsafe { CreateSolidBrush(theme::SURFACE_RAISED) };

        self.token_edit = unsafe { create_text_edit(hwnd, instance, self.ui_font, true) };
        self.device_edit = unsafe { create_text_edit(hwnd, instance, self.ui_font, false) };

        if self.token_edit.is_null() || self.device_edit.is_null() {
            return Err(io::Error::last_os_error())
                .context("failed to create collector UI controls");
        }

        let device_name = wide(&default_device_name());

        unsafe {
            SetWindowTextW(self.device_edit, device_name.as_ptr());
        }

        self.update_control_visibility();
        self.layout_controls(hwnd);
        self.refresh_logs(hwnd);

        if !self.provisioned {
            unsafe {
                SetFocus(self.token_edit);
            }
        }

        Ok(())
    }

    fn start_collector_if_ready(&mut self, hwnd: HWND) {
        if self.worker_started {
            return;
        }

        let Some(access_key) = self.access_key.take() else {
            return;
        };

        self.worker_started = true;
        spawn_collector(self.runtime.clone(), access_key, hwnd);
    }

    unsafe fn install_tray_icon(&self, hwnd: HWND) -> Result<()> {
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };

        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ID;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = self.app_icon;
        write_wide_array(&mut data.szTip, "Mnemos Collector");

        if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
            return Err(io::Error::last_os_error()).context("failed to create collector tray icon");
        }

        Ok(())
    }

    fn remove_tray_icon(&self, hwnd: HWND) {
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };

        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ID;

        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }

    fn begin_activation(&mut self, hwnd: HWND) {
        if self.provisioned || self.provisioning {
            return;
        }

        let token = unsafe { window_text(self.token_edit) };
        let device_name = unsafe { window_text(self.device_edit) };

        if token.trim().is_empty() {
            self.activation_error = Some("Введите одноразовый код активации из Mnemos.".to_owned());
            self.invalidate(hwnd);
            return;
        }

        self.provisioning = true;
        self.activation_error = None;

        let empty = wide("");

        unsafe {
            SetWindowTextW(self.token_edit, empty.as_ptr());
        }

        self.invalidate(hwnd);

        let runtime = self.runtime.clone();
        let current_installation = self.current_installation;
        let hwnd_value = hwnd as usize;
        let token = Zeroizing::new(token);
        let worker_runtime = runtime.clone();

        runtime.spawn(async move {
            diagnostics::info("provisioning", "Activation started from desktop UI");

            let result = if current_installation {
                provision_current_installation(token.as_str(), &device_name).await
            } else {
                install_from_ui(token.as_str(), &device_name).await
            };

            let hwnd = hwnd_value as HWND;

            match result {
                Ok(access_key) => {
                    diagnostics::info("provisioning", "Activation completed successfully");
                    let result_code = if current_installation { 1 } else { 2 };

                    unsafe {
                        PostMessageW(hwnd, WM_ACTIVATION_RESULT, result_code, 0);
                    }

                    if let Some(access_key) = access_key {
                        spawn_collector(worker_runtime, access_key, hwnd);
                    }
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    diagnostics::error("provisioning", message.clone());
                    let raw = Box::into_raw(Box::new(message));

                    unsafe {
                        PostMessageW(hwnd, WM_ACTIVATION_RESULT, 0, raw as isize);
                    }
                }
            }
        });
    }

    fn activation_completed(&mut self, hwnd: HWND, result_code: usize, message: isize) {
        self.provisioning = false;

        match result_code {
            1 => {
                self.provisioned = true;
                self.worker_started = true;
                self.activation_error = None;
                self.hovered = None;
                self.update_control_visibility();
                self.layout_controls(hwnd);
            }
            2 => unsafe {
                DestroyWindow(hwnd);
                return;
            },
            _ => {
                if message != 0 {
                    let message = unsafe { Box::from_raw(message as *mut String) };
                    self.activation_error = Some(*message);
                } else {
                    self.activation_error = Some("Не удалось активировать Collector.".to_owned());
                }
            }
        }

        self.invalidate(hwnd);
    }

    fn update_control_visibility(&self) {
        let visibility = if self.provisioned { SW_HIDE } else { SW_SHOW };

        unsafe {
            ShowWindow(self.token_edit, visibility);
            ShowWindow(self.device_edit, visibility);
        }
    }

    fn layout_controls(&self, hwnd: HWND) {
        let layout = window_layout(hwnd, self.provisioned);

        unsafe {
            move_control(self.token_edit, layout.token_edit);
            move_control(self.device_edit, layout.device_edit);
        }
    }

    fn refresh(&mut self, hwnd: HWND) {
        self.refresh_logs(hwnd);

        let runtime = diagnostics::runtime_snapshot();

        if runtime != self.last_runtime {
            self.last_runtime = runtime;
            self.invalidate(hwnd);
        }
    }

    fn refresh_logs(&mut self, hwnd: HWND) {
        let text = diagnostics::recent_text();

        if text == self.last_log_text {
            return;
        }

        self.last_log_text = text;

        let layout = window_layout(hwnd, self.provisioned);
        let limit = view::log_scroll_limit(&self.last_log_text, layout.logs_view);
        self.log_scroll_from_bottom = self.log_scroll_from_bottom.min(limit);

        self.invalidate(hwnd);
    }

    fn scroll_logs(&mut self, hwnd: HWND, x: i32, y: i32, delta: i16) {
        let layout = window_layout(hwnd, self.provisioned);

        if !layout.logs_view.contains(x, y) {
            return;
        }

        let lines = ((delta.unsigned_abs() as usize) / 120).max(1) * 3;
        let limit = view::log_scroll_limit(&self.last_log_text, layout.logs_view);

        if delta > 0 {
            self.log_scroll_from_bottom = (self.log_scroll_from_bottom + lines).min(limit);
        } else if delta < 0 {
            self.log_scroll_from_bottom = self.log_scroll_from_bottom.saturating_sub(lines);
        }

        self.invalidate(hwnd);
    }

    fn mouse_move(&mut self, hwnd: HWND, x: i32, y: i32) {
        let layout = window_layout(hwnd, self.provisioned);
        let hovered = view::interactive_element_at(layout, self.provisioned, x, y);

        if self.hovered != hovered {
            self.hovered = hovered;
            self.invalidate(hwnd);
        }

        if !self.tracking_mouse_leave {
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };

            if unsafe { TrackMouseEvent(&mut tracking) } != 0 {
                self.tracking_mouse_leave = true;
            }
        }
    }

    fn mouse_leave(&mut self, hwnd: HWND) {
        self.tracking_mouse_leave = false;

        if self.hovered.take().is_some() {
            self.invalidate(hwnd);
        }
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
        let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
        let mut client: RECT = unsafe { std::mem::zeroed() };

        unsafe {
            GetClientRect(hwnd, &mut client);
            view::fill_background(hdc, &client);
        }

        let layout = view::layout(
            client.right - client.left,
            client.bottom - client.top,
            self.provisioned,
        );
        let runtime = diagnostics::runtime_snapshot();
        let fonts = Fonts {
            ui: self.ui_font,
            title: self.title_font,
            section: self.section_font,
            mono: self.mono_font,
        };
        let state = ViewState {
            current_installation: self.current_installation,
            provisioning: self.provisioning,
            activation_error: self.activation_error.as_deref(),
            debug_enabled: diagnostics::debug_enabled(),
            hovered: self.hovered,
            log_text: &self.last_log_text,
            log_scroll_from_bottom: self.log_scroll_from_bottom,
        };

        unsafe {
            view::draw(hdc, &runtime, layout, fonts, state);
            EndPaint(hwnd, &paint);
        }
    }

    fn click(&mut self, hwnd: HWND, x: i32, y: i32) {
        let layout = window_layout(hwnd, self.provisioned);

        match view::interactive_element_at(layout, self.provisioned, x, y) {
            Some(InteractiveElement::WindowClose) => unsafe {
                ShowWindow(hwnd, SW_HIDE);
            },
            Some(InteractiveElement::WindowMinimize) => unsafe {
                ShowWindow(hwnd, SW_MINIMIZE);
            },
            Some(InteractiveElement::DebugToggle) => {
                diagnostics::set_debug_enabled(!diagnostics::debug_enabled());
                self.invalidate(hwnd);
            }
            Some(InteractiveElement::ActivateButton) => {
                self.begin_activation(hwnd);
            }
            None => {}
        }
    }

    fn invalidate(&self, hwnd: HWND) {
        unsafe {
            InvalidateRect(hwnd, null(), 0);
        }
    }

    fn show_tray_popup(&self, hwnd: HWND) {
        if let Err(error) = tray_popup::show(hwnd, self.ui_font) {
            diagnostics::error("desktop", format!("Tray popup failed: {error:#}"));
        }
    }
}

impl Drop for DesktopWindow {
    fn drop(&mut self) {
        unsafe {
            for object in [
                self.ui_font,
                self.title_font,
                self.section_font,
                self.mono_font,
                self.edit_brush,
            ] {
                if !object.is_null() {
                    DeleteObject(object);
                }
            }

            if !self.app_icon.is_null() {
                DestroyIcon(self.app_icon);
            }
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        let state = unsafe { (*create).lpCreateParams as *mut DesktopWindow };

        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        }
    }

    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DesktopWindow };

    match message {
        WM_PAINT => {
            if !state.is_null() {
                unsafe {
                    (*state).paint(hwnd);
                }
                return 0;
            }
        }
        WM_ERASEBKGND => return 1,
        WM_NCHITTEST if !state.is_null() => {
            let mut point = POINT {
                x: low_word(lparam as usize) as i16 as i32,
                y: high_word(lparam as usize) as i16 as i32,
            };

            unsafe {
                ScreenToClient(hwnd, &mut point);
                return hit_test_window(hwnd, (*state).provisioned, point);
            }
        }
        WM_GETMINMAXINFO => {
            if lparam != 0 {
                let limits = unsafe { &mut *(lparam as *mut MINMAXINFO) };
                let provisioned = !state.is_null() && unsafe { (*state).provisioned };

                limits.ptMinTrackSize.x = MIN_WINDOW_WIDTH;
                limits.ptMinTrackSize.y = minimum_window_height(provisioned);
            }
            return 0;
        }
        WM_SIZE => {
            if wparam as u32 == SIZE_MINIMIZED {
                unsafe {
                    ShowWindow(hwnd, SW_HIDE);
                }
                return 0;
            }

            if !state.is_null() {
                unsafe {
                    (*state).hovered = None;
                    (*state).layout_controls(hwnd);
                    (*state).invalidate(hwnd);
                }
            }
        }
        WM_TIMER => {
            if wparam == TIMER_REFRESH && !state.is_null() {
                unsafe {
                    (*state).refresh(hwnd);
                }
            }
            return 0;
        }
        WM_LBUTTONUP => {
            if !state.is_null() {
                let x = low_word(lparam as usize) as i16 as i32;
                let y = high_word(lparam as usize) as i16 as i32;

                unsafe {
                    (*state).click(hwnd, x, y);
                }
            }
            return 0;
        }
        WM_MOUSEMOVE => {
            if !state.is_null() {
                let x = low_word(lparam as usize) as i16 as i32;
                let y = high_word(lparam as usize) as i16 as i32;

                unsafe {
                    (*state).mouse_move(hwnd, x, y);
                }
            }
            return 0;
        }
        WM_MOUSELEAVE => {
            if !state.is_null() {
                unsafe {
                    (*state).mouse_leave(hwnd);
                }
            }
            return 0;
        }
        WM_MOUSEWHEEL => {
            if !state.is_null() {
                let mut point = POINT {
                    x: low_word(lparam as usize) as i16 as i32,
                    y: high_word(lparam as usize) as i16 as i32,
                };
                let delta = high_word(wparam) as i16;

                unsafe {
                    ScreenToClient(hwnd, &mut point);
                    (*state).scroll_logs(hwnd, point.x, point.y, delta);
                }
            }
            return 0;
        }
        WM_TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP => unsafe {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                },
                WM_RBUTTONUP if !state.is_null() => unsafe {
                    (*state).show_tray_popup(hwnd);
                },
                _ => {}
            }
            return 0;
        }
        WM_ACTIVATION_RESULT => {
            if !state.is_null() {
                unsafe {
                    (*state).activation_completed(hwnd, wparam, lparam);
                }
            }
            return 0;
        }
        WM_COLLECTOR_STOPPED => unsafe {
            DestroyWindow(hwnd);
            return 0;
        },
        WM_CTLCOLOREDIT => {
            if !state.is_null() {
                unsafe {
                    let hdc = wparam as *mut c_void;
                    SetTextColor(hdc, theme::TEXT);
                    SetBkColor(hdc, theme::SURFACE_RAISED);
                    return (*state).edit_brush as isize;
                }
            }
        }
        WM_CLOSE => unsafe {
            ShowWindow(hwnd, SW_HIDE);
            return 0;
        },
        WM_DESTROY => {
            if !state.is_null() {
                unsafe {
                    (*state).remove_tray_icon(hwnd);
                }
            }

            unsafe {
                PostQuitMessage(0);
            }
            return 0;
        }
        _ => {}
    }

    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn hit_test_window(hwnd: HWND, provisioned: bool, point: POINT) -> LRESULT {
    let mut client: RECT = unsafe { std::mem::zeroed() };

    unsafe {
        GetClientRect(hwnd, &mut client);
    }

    let width = client.right - client.left;
    let height = client.bottom - client.top;
    let left = point.x < RESIZE_BORDER;
    let right = point.x >= width - RESIZE_BORDER;
    let top = point.y < RESIZE_BORDER;
    let bottom = point.y >= height - RESIZE_BORDER;

    if top && left {
        return HTTOPLEFT as isize;
    }
    if top && right {
        return HTTOPRIGHT as isize;
    }
    if bottom && left {
        return HTBOTTOMLEFT as isize;
    }
    if bottom && right {
        return HTBOTTOMRIGHT as isize;
    }
    if left {
        return HTLEFT as isize;
    }
    if right {
        return HTRIGHT as isize;
    }
    if top {
        return HTTOP as isize;
    }
    if bottom {
        return HTBOTTOM as isize;
    }

    let layout = view::layout(width, height, provisioned);

    if layout.window_minimize.contains(point.x, point.y)
        || layout.window_close.contains(point.x, point.y)
    {
        return HTCLIENT as isize;
    }

    if layout.title_bar.contains(point.x, point.y) {
        return HTCAPTION as isize;
    }

    HTCLIENT as isize
}

fn minimum_window_height(provisioned: bool) -> i32 {
    if provisioned {
        MIN_PROVISIONED_WINDOW_HEIGHT
    } else {
        MIN_ACTIVATION_WINDOW_HEIGHT
    }
}

async fn provision_current_installation(token: &str, device_name: &str) -> Result<Option<String>> {
    ProvisioningClient::new()?
        .provision(token, device_name)
        .await
        .context("не удалось активировать Collector")?;

    Autostart::ensure_enabled().context("не удалось включить автозапуск Collector")?;

    let access_key = CredentialStore
        .load()?
        .context("активация завершилась без сохранённого credential")?;

    Ok(Some(access_key))
}

async fn install_from_ui(token: &str, device_name: &str) -> Result<Option<String>> {
    let token = token.to_owned();
    let device_name = device_name.to_owned();

    tokio::task::spawn_blocking(move || {
        Installation::install_and_launch(&token, Some(&device_name))
            .context("не удалось установить Mnemos Collector")
    })
    .await
    .context("задача установки Collector завершилась аварийно")??;

    Ok(None)
}

fn spawn_collector(runtime: Handle, access_key: String, hwnd: HWND) {
    let hwnd_value = hwnd as usize;

    runtime.spawn(async move {
        diagnostics::clear_last_error();
        diagnostics::info("runtime", "Collector worker starting");

        let result = async {
            let application = CollectorApplication::new(access_key).await?;
            application.run().await
        }
        .await;

        match result {
            Ok(()) => {
                diagnostics::info("runtime", "Collector worker stopped cleanly");

                unsafe {
                    PostMessageW(hwnd_value as HWND, WM_COLLECTOR_STOPPED, 0, 0);
                }
            }
            Err(error) => {
                diagnostics::error("runtime", format!("Collector worker failed: {error:#}"));
            }
        }
    });
}

unsafe fn create_text_edit(
    hwnd: HWND,
    instance: *mut c_void,
    font: *mut c_void,
    password: bool,
) -> HWND {
    let class = wide("EDIT");
    let empty = wide("");
    let mut style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32;

    if password {
        style |= ES_PASSWORD as u32;
    }

    let edit = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            empty.as_ptr(),
            style,
            0,
            0,
            100,
            34,
            hwnd,
            null_mut(),
            instance,
            null(),
        )
    };

    if !edit.is_null() {
        unsafe {
            SendMessageW(edit, WM_SETFONT, font as usize, 1);
        }
    }

    edit
}

unsafe fn move_control(hwnd: HWND, rect: view::UiRect) {
    unsafe {
        MoveWindow(hwnd, rect.left, rect.top, rect.width(), rect.height(), 1);
    }
}

unsafe fn window_text(hwnd: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(hwnd) };

    if length <= 0 {
        return String::new();
    }

    let mut buffer = vec![0_u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };

    String::from_utf16_lossy(&buffer[..copied as usize])
}

fn window_layout(hwnd: HWND, provisioned: bool) -> Layout {
    let mut client: RECT = unsafe { std::mem::zeroed() };

    unsafe {
        GetClientRect(hwnd, &mut client);
    }

    view::layout(
        client.right - client.left,
        client.bottom - client.top,
        provisioned,
    )
}

fn apply_window_chrome(hwnd: HWND) {
    let dark_mode: i32 = 1;
    let caption_color = theme::BACKGROUND_DEEP;
    let border_color = theme::LINE;
    let corner_preference = DWM_WINDOW_CORNER_PREFERENCE_ROUND;

    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark_mode as *const i32).cast::<c_void>(),
            std::mem::size_of::<i32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            (&caption_color as *const u32).cast::<c_void>(),
            std::mem::size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            (&border_color as *const u32).cast::<c_void>(),
            std::mem::size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWM_WINDOW_CORNER_PREFERENCE_ATTRIBUTE,
            (&corner_preference as *const u32).cast::<c_void>(),
            std::mem::size_of::<u32>() as u32,
        );
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn write_wide_array(target: &mut [u16], value: &str) {
    for (destination, source) in target
        .iter_mut()
        .zip(value.encode_utf16().chain(std::iter::once(0)))
    {
        *destination = source;
    }
}

fn low_word(value: usize) -> u16 {
    (value & 0xffff) as u16
}

fn high_word(value: usize) -> u16 {
    ((value >> 16) & 0xffff) as u16
}
