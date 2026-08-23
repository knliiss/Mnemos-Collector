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
    SetBkColor, SetTextColor, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CREATESTRUCTW, CW_USEDEFAULT, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, EM_SCROLLCARET, EM_SETSEL, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_PASSWORD, ES_READONLY, GWLP_USERDATA, GetClientRect,
    GetCursorPos, GetMessageW, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, IDC_ARROW,
    IDI_APPLICATION, LoadCursorW, LoadIconW, MF_STRING, MSG, MessageBoxW, MoveWindow, PostMessageW,
    PostQuitMessage, RegisterClassW, SIZE_MINIMIZED, SW_HIDE, SW_RESTORE, SW_SHOW, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TrackPopupMenu, TranslateMessage, WM_APP, WM_CLOSE, WM_COMMAND, WM_CTLCOLOREDIT,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONUP, WM_NCCREATE, WM_PAINT,
    WM_RBUTTONUP, WM_SETFONT, WM_SIZE, WM_TIMER, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use zeroize::Zeroizing;

use crate::application::CollectorApplication;
use crate::diagnostics::{self, RuntimeSnapshot};
use crate::platform::{Autostart, Installation};
use crate::provisioning::{ProvisioningClient, default_device_name};
use crate::security::CredentialStore;

use super::DesktopLaunchContext;
use super::theme;
use super::view::{self, Fonts, Layout, ViewState};

const CLASS_NAME: &str = "MnemosCollectorShell";
const WINDOW_TITLE: &str = "Mnemos Collector";
const TIMER_REFRESH: usize = 1;
const REFRESH_INTERVAL_MS: u32 = 750;
const WM_TRAY: u32 = WM_APP + 1;
const WM_ACTIVATION_RESULT: u32 = WM_APP + 2;
const WM_COLLECTOR_STOPPED: u32 = WM_APP + 3;
const TRAY_ID: u32 = 1;
const MENU_OPEN: usize = 4101;
const MENU_EXIT: usize = 4102;

pub fn run(context: DesktopLaunchContext, runtime: Handle) -> Result<()> {
    unsafe {
        let instance = GetModuleHandleW(null());

        if instance.is_null() {
            return Err(io::Error::last_os_error())
                .context("failed to get collector module handle");
        }

        let class_name = wide(CLASS_NAME);
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
                return Err(error).context("failed to register collector window class");
            }
        }

        let state = Box::new(DesktopWindow::new(context, runtime));
        let state_ptr = Box::into_raw(state);
        let title = wide(WINDOW_TITLE);
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1040,
            780,
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
    logs_edit: HWND,
    ui_font: *mut c_void,
    title_font: *mut c_void,
    section_font: *mut c_void,
    mono_font: *mut c_void,
    edit_brush: *mut c_void,
    last_log_text: String,
    last_runtime: RuntimeSnapshot,
}

impl DesktopWindow {
    fn new(context: DesktopLaunchContext, runtime: Handle) -> Self {
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
            logs_edit: null_mut(),
            ui_font: null_mut(),
            title_font: null_mut(),
            section_font: null_mut(),
            mono_font: null_mut(),
            edit_brush: null_mut(),
            last_log_text: String::new(),
            last_runtime: diagnostics::runtime_snapshot(),
        }
    }

    unsafe fn initialize_controls(&mut self, hwnd: HWND, instance: *mut c_void) -> Result<()> {
        let ui = wide("Segoe UI Variable Text");
        let mono = wide("Cascadia Mono");

        self.ui_font =
            unsafe { CreateFontW(-17, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, ui.as_ptr()) };
        self.title_font =
            unsafe { CreateFontW(-31, 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, ui.as_ptr()) };
        self.section_font =
            unsafe { CreateFontW(-22, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, ui.as_ptr()) };
        self.mono_font =
            unsafe { CreateFontW(-15, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, mono.as_ptr()) };
        self.edit_brush = unsafe { CreateSolidBrush(theme::SURFACE_RAISED) };

        self.token_edit = unsafe { create_text_edit(hwnd, instance, self.ui_font, true) };
        self.device_edit = unsafe { create_text_edit(hwnd, instance, self.ui_font, false) };
        self.logs_edit = unsafe { create_log_edit(hwnd, instance, self.mono_font) };

        if self.token_edit.is_null() || self.device_edit.is_null() || self.logs_edit.is_null() {
            return Err(io::Error::last_os_error())
                .context("failed to create collector UI controls");
        }

        let device_name = wide(&default_device_name());

        unsafe {
            SetWindowTextW(self.device_edit, device_name.as_ptr());
        }

        self.update_control_visibility();
        self.layout_controls(hwnd);
        self.refresh_logs();

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
        let icon = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) };

        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ID;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = icon;
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
            move_control(self.logs_edit, layout.logs_edit);
        }
    }

    fn refresh(&mut self, hwnd: HWND) {
        self.refresh_logs();

        let runtime = diagnostics::runtime_snapshot();

        if runtime != self.last_runtime {
            self.last_runtime = runtime;
            self.invalidate(hwnd);
        }
    }

    fn refresh_logs(&mut self) {
        let text = diagnostics::recent_text();

        if text == self.last_log_text {
            return;
        }

        let follow_tail = unsafe { GetFocus() != self.logs_edit };
        self.last_log_text = text.clone();
        let text = wide(&text);

        unsafe {
            SetWindowTextW(self.logs_edit, text.as_ptr());

            if follow_tail {
                let length = GetWindowTextLengthW(self.logs_edit).max(0) as usize;
                SendMessageW(self.logs_edit, EM_SETSEL, length, length as isize);
                SendMessageW(self.logs_edit, EM_SCROLLCARET, 0, 0);
            }
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
        };
        let state = ViewState {
            current_installation: self.current_installation,
            provisioning: self.provisioning,
            activation_error: self.activation_error.as_deref(),
            debug_enabled: diagnostics::debug_enabled(),
        };

        unsafe {
            view::draw(hdc, &runtime, layout, fonts, state);
            EndPaint(hwnd, &paint);
        }
    }

    fn click(&mut self, hwnd: HWND, x: i32, y: i32) {
        let layout = window_layout(hwnd, self.provisioned);

        if layout.tray_button.contains(x, y) {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
            return;
        }

        if layout.debug_toggle.contains(x, y) {
            diagnostics::set_debug_enabled(!diagnostics::debug_enabled());
            self.invalidate(hwnd);
            return;
        }

        if !self.provisioned && layout.activate_button.contains(x, y) {
            self.begin_activation(hwnd);
        }
    }

    fn invalidate(&self, hwnd: HWND) {
        unsafe {
            InvalidateRect(hwnd, null(), 0);
        }
    }

    fn show_tray_menu(&self, hwnd: HWND) {
        unsafe {
            let menu = CreatePopupMenu();

            if menu.is_null() {
                return;
            }

            let open = wide("Открыть Mnemos Collector");
            let exit = wide("Выйти");

            AppendMenuW(menu, MF_STRING, MENU_OPEN, open.as_ptr());
            AppendMenuW(menu, MF_STRING, MENU_EXIT, exit.as_ptr());

            let mut cursor = POINT { x: 0, y: 0 };
            GetCursorPos(&mut cursor);
            SetForegroundWindow(hwnd);
            TrackPopupMenu(
                menu,
                TPM_BOTTOMALIGN | TPM_LEFTALIGN,
                cursor.x,
                cursor.y,
                0,
                hwnd,
                null(),
            );
            DestroyMenu(menu);
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
        WM_SIZE => {
            if wparam as u32 == SIZE_MINIMIZED {
                unsafe {
                    ShowWindow(hwnd, SW_HIDE);
                }
                return 0;
            }

            if !state.is_null() {
                unsafe {
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
        WM_TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP => unsafe {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                },
                WM_RBUTTONUP if !state.is_null() => unsafe {
                    (*state).show_tray_menu(hwnd);
                },
                _ => {}
            }
            return 0;
        }
        WM_COMMAND => {
            let command = low_word(wparam) as usize;

            match command {
                MENU_OPEN => unsafe {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                },
                MENU_EXIT => unsafe {
                    DestroyWindow(hwnd);
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
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
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
            38,
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

unsafe fn create_log_edit(hwnd: HWND, instance: *mut c_void, font: *mut c_void) -> HWND {
    let class = wide("EDIT");
    let empty = wide("");
    let style = WS_CHILD
        | WS_VISIBLE
        | WS_VSCROLL
        | ES_MULTILINE as u32
        | ES_AUTOVSCROLL as u32
        | ES_READONLY as u32;
    let edit = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            empty.as_ptr(),
            style,
            0,
            0,
            100,
            100,
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
