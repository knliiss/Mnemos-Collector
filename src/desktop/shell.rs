use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};

use anyhow::{Context, Result};
use tokio::runtime::Handle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, Ellipse, EndPaint,
    FillRect, InvalidateRect, PAINTSTRUCT, RoundRect, SelectObject, SetBkColor, SetBkMode,
    SetTextColor, TextOutW, UpdateWindow,
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
use super::mascot;
use super::theme;

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

#[derive(Clone, Copy)]
struct UiRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl UiRect {
    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }

    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
}

struct Layout {
    hero: UiRect,
    tray_button: UiRect,
    activation: Option<UiRect>,
    token_edit: UiRect,
    device_edit: UiRect,
    activate_button: UiRect,
    logs_card: UiRect,
    logs_edit: UiRect,
    debug_toggle: UiRect,
}

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

        self.ui_font = unsafe { CreateFontW(-17, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, ui.as_ptr()) };
        self.title_font = unsafe { CreateFontW(-31, 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, ui.as_ptr()) };
        self.section_font = unsafe { CreateFontW(-22, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, ui.as_ptr()) };
        self.mono_font = unsafe { CreateFontW(-15, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, mono.as_ptr()) };
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
        let activation_visibility = if self.provisioned { SW_HIDE } else { SW_SHOW };

        unsafe {
            ShowWindow(self.token_edit, activation_visibility);
            ShowWindow(self.device_edit, activation_visibility);
        }
    }

    fn layout_controls(&self, hwnd: HWND) {
        let layout = window_layout(hwnd, self.provisioned);

        unsafe {
            MoveWindow(
                self.token_edit,
                layout.token_edit.left,
                layout.token_edit.top,
                layout.token_edit.width(),
                layout.token_edit.height(),
                1,
            );
            MoveWindow(
                self.device_edit,
                layout.device_edit.left,
                layout.device_edit.top,
                layout.device_edit.width(),
                layout.device_edit.height(),
                1,
            );
            MoveWindow(
                self.logs_edit,
                layout.logs_edit.left,
                layout.logs_edit.top,
                layout.logs_edit.width(),
                layout.logs_edit.height(),
                1,
            );
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
        }

        let background = unsafe { CreateSolidBrush(theme::BACKGROUND_DEEP) };

        unsafe {
            FillRect(hdc, &client, background);
            DeleteObject(background);
            SetBkMode(hdc, 1);
        }

        let layout = layout(client.right - client.left, client.bottom - client.top, self.provisioned);
        let runtime = diagnostics::runtime_snapshot();

        unsafe {
            draw_header(hdc, self.ui_font, self.title_font);
            draw_hero(
                hdc,
                &runtime,
                layout.hero,
                layout.tray_button,
                self.ui_font,
                self.title_font,
            );

            mascot::draw(hdc, layout.hero.right - 160, layout.hero.top + 16, 122);

            if let Some(activation) = layout.activation {
                draw_activation(
                    hdc,
                    activation,
                    layout,
                    self.current_installation,
                    self.provisioning,
                    self.activation_error.as_deref(),
                    self.ui_font,
                    self.section_font,
                );
            }

            draw_logs_panel(
                hdc,
                layout,
                diagnostics::debug_enabled(),
                self.ui_font,
                self.section_font,
            );

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
            for font in [
                self.ui_font,
                self.title_font,
                self.section_font,
                self.mono_font,
                self.edit_brush,
            ] {
                if !font.is_null() {
                    DeleteObject(font);
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

    layout(client.right - client.left, client.bottom - client.top, provisioned)
}

fn layout(width: i32, height: i32, provisioned: bool) -> Layout {
    let margin = 28;
    let hero = UiRect {
        left: margin,
        top: 94,
        right: (width - margin).max(margin + 620),
        bottom: 276,
    };
    let tray_button = UiRect {
        left: hero.right - 354,
        top: hero.top + 24,
        right: hero.right - 178,
        bottom: hero.top + 60,
    };

    let (activation, logs_top) = if provisioned {
        (None, 306)
    } else {
        (
            Some(UiRect {
                left: margin,
                top: 296,
                right: width - margin,
                bottom: 452,
            }),
            482,
        )
    };

    let activation_rect = activation.unwrap_or(UiRect {
        left: margin,
        top: 0,
        right: width - margin,
        bottom: 0,
    });
    let edit_top = activation_rect.top + 84;
    let available = (activation_rect.width() - 40).max(560);
    let button_width = 156;
    let device_width = 210;
    let gap = 12;
    let token_width = (available - button_width - device_width - gap * 2).max(220);
    let token_edit = UiRect {
        left: activation_rect.left + 20,
        top: edit_top,
        right: activation_rect.left + 20 + token_width,
        bottom: edit_top + 38,
    };
    let device_edit = UiRect {
        left: token_edit.right + gap,
        top: edit_top,
        right: token_edit.right + gap + device_width,
        bottom: edit_top + 38,
    };
    let activate_button = UiRect {
        left: device_edit.right + gap,
        top: edit_top,
        right: activation_rect.right - 20,
        bottom: edit_top + 38,
    };

    let logs_card = UiRect {
        left: margin,
        top: logs_top,
        right: width - margin,
        bottom: (height - margin).max(logs_top + 190),
    };
    let debug_toggle = UiRect {
        left: logs_card.right - 244,
        top: logs_card.top + 14,
        right: logs_card.right - 18,
        bottom: logs_card.top + 46,
    };
    let logs_edit = UiRect {
        left: logs_card.left + 18,
        top: logs_card.top + 58,
        right: logs_card.right - 18,
        bottom: logs_card.bottom - 42,
    };

    Layout {
        hero,
        tray_button,
        activation,
        token_edit,
        device_edit,
        activate_button,
        logs_card,
        logs_edit,
        debug_toggle,
    }
}

unsafe fn draw_header(hdc: *mut c_void, ui_font: *mut c_void, title_font: *mut c_void) {
    unsafe {
        draw_text(hdc, 30, 24, "MNEMOS", ui_font, theme::ACCENT);
        draw_text(hdc, 30, 48, "Collector", title_font, theme::TEXT);
        draw_text(
            hdc,
            168,
            60,
            "Cristalix / Master Sword",
            ui_font,
            theme::TEXT_MUTED,
        );
    }
}

unsafe fn draw_hero(
    hdc: *mut c_void,
    runtime: &RuntimeSnapshot,
    rect: UiRect,
    tray_button: UiRect,
    ui_font: *mut c_void,
    title_font: *mut c_void,
) {
    let (title, detail, status_color) = status_copy(runtime);

    unsafe {
        draw_card(hdc, rect, theme::SURFACE, theme::LINE);
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 18,
            "СОСТОЯНИЕ",
            ui_font,
            theme::ACCENT,
        );
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 45,
            title,
            title_font,
            status_color,
        );
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 82,
            detail,
            ui_font,
            theme::TEXT_SECONDARY,
        );

        draw_secondary_button(hdc, tray_button, "Свернуть в трей", ui_font);

        let chips_top = rect.bottom - 52;
        let chip_width = 142;
        let gap = 8;
        let mut chip_left = rect.left + 20;

        draw_status_chip(
            hdc,
            UiRect {
                left: chip_left,
                top: chips_top,
                right: chip_left + chip_width,
                bottom: chips_top + 34,
            },
            "Cristalix",
            if runtime.cristalix_running { "найден" } else { "ожидание" },
            if runtime.cristalix_running { theme::POSITIVE } else { theme::TEXT_MUTED },
            ui_font,
        );
        chip_left += chip_width + gap;

        let mode = if runtime.game_mode.is_empty() {
            "Unknown"
        } else {
            runtime.game_mode.as_str()
        };
        draw_status_chip(
            hdc,
            UiRect {
                left: chip_left,
                top: chips_top,
                right: chip_left + chip_width,
                bottom: chips_top + 34,
            },
            "Режим",
            mode,
            if runtime.game_mode == "MasterSword" { theme::ACCENT } else { theme::AMBER },
            ui_font,
        );
        chip_left += chip_width + gap;

        draw_status_chip(
            hdc,
            UiRect {
                left: chip_left,
                top: chips_top,
                right: chip_left + chip_width,
                bottom: chips_top + 34,
            },
            "Realtime",
            if runtime.realtime_connected { "online" } else { "offline" },
            if runtime.realtime_connected { theme::POSITIVE } else { theme::DANGER },
            ui_font,
        );
        chip_left += chip_width + gap;

        draw_status_chip(
            hdc,
            UiRect {
                left: chip_left,
                top: chips_top,
                right: chip_left + chip_width,
                bottom: chips_top + 34,
            },
            "Наблюдение",
            if runtime.observing { "активно" } else { "пауза" },
            if runtime.observing { theme::ACCENT } else { theme::TEXT_MUTED },
            ui_font,
        );
    }
}

fn status_copy(runtime: &RuntimeSnapshot) -> (&'static str, &'static str, u32) {
    if runtime.observing {
        return (
            "Наблюдение активно",
            "Master Sword распознан, realtime-service подтвердил OBSERVING.",
            theme::ACCENT,
        );
    }

    if !runtime.cristalix_running {
        return (
            "Ожидание Cristalix",
            "Collector работает в фоне и автоматически подхватит уже открытый режим.",
            theme::TEXT,
        );
    }

    if runtime.game_mode == "MasterSword" && !runtime.realtime_connected {
        return (
            "Master Sword найден",
            "Режим распознан. Восстанавливаем соединение с realtime-service.",
            theme::AMBER,
        );
    }

    if runtime.game_mode == "MasterSword" {
        return (
            "Master Sword найден",
            "Режим распознан. Ожидаем подтверждение OBSERVING от realtime-service.",
            theme::AMBER,
        );
    }

    (
        "Cristalix найден",
        "Collector анализирует latest.log и восстанавливает текущий контекст без перезахода.",
        theme::TEXT,
    )
}

unsafe fn draw_activation(
    hdc: *mut c_void,
    activation: UiRect,
    layout: Layout,
    current_installation: bool,
    provisioning: bool,
    error: Option<&str>,
    ui_font: *mut c_void,
    section_font: *mut c_void,
) {
    unsafe {
        draw_card(hdc, activation, theme::SURFACE, theme::LINE);
        draw_text(
            hdc,
            activation.left + 20,
            activation.top + 17,
            if current_installation {
                "Подключить Collector"
            } else {
                "Установить и подключить Collector"
            },
            section_font,
            theme::TEXT,
        );
        draw_text(
            hdc,
            activation.left + 20,
            activation.top + 49,
            "Одноразовый код из Mnemos",
            ui_font,
            theme::TEXT_MUTED,
        );
        draw_text(
            hdc,
            layout.device_edit.left,
            activation.top + 49,
            "Имя устройства",
            ui_font,
            theme::TEXT_MUTED,
        );
        draw_primary_button(
            hdc,
            layout.activate_button,
            if provisioning { "Подключаем..." } else { "Активировать" },
            ui_font,
        );

        if let Some(error) = error {
            draw_text(
                hdc,
                activation.left + 20,
                activation.bottom - 27,
                error,
                ui_font,
                theme::DANGER,
            );
        }
    }
}

unsafe fn draw_logs_panel(
    hdc: *mut c_void,
    layout: Layout,
    debug_enabled: bool,
    ui_font: *mut c_void,
    section_font: *mut c_void,
) {
    unsafe {
        draw_card(hdc, layout.logs_card, theme::SURFACE, theme::LINE);
        draw_text(
            hdc,
            layout.logs_card.left + 18,
            layout.logs_card.top + 16,
            "Логи Collector",
            section_font,
            theme::TEXT,
        );

        draw_toggle(
            hdc,
            layout.debug_toggle,
            "Подробная диагностика",
            debug_enabled,
            ui_font,
        );

        if let Some(path) = diagnostics::log_file_path() {
            draw_text(
                hdc,
                layout.logs_card.left + 18,
                layout.logs_card.bottom - 29,
                &format!("Файл: {}", path.display()),
                ui_font,
                theme::TEXT_MUTED,
            );
        }
    }
}

unsafe fn draw_status_chip(
    hdc: *mut c_void,
    rect: UiRect,
    label: &str,
    value: &str,
    status_color: u32,
    font: *mut c_void,
) {
    unsafe {
        draw_card(hdc, rect, theme::SURFACE_RAISED, theme::LINE);
        draw_dot(hdc, rect.left + 11, rect.top + 13, status_color);
        draw_text(hdc, rect.left + 25, rect.top + 5, label, font, theme::TEXT_MUTED);
        draw_text(hdc, rect.left + 25, rect.top + 18, value, font, theme::TEXT);
    }
}

unsafe fn draw_toggle(
    hdc: *mut c_void,
    rect: UiRect,
    label: &str,
    enabled: bool,
    font: *mut c_void,
) {
    unsafe {
        draw_card(hdc, rect, theme::SURFACE_RAISED, theme::LINE);
        draw_dot(
            hdc,
            rect.left + 14,
            rect.top + 13,
            if enabled { theme::ACCENT } else { theme::TEXT_MUTED },
        );
        draw_text(
            hdc,
            rect.left + 30,
            rect.top + 8,
            label,
            font,
            if enabled { theme::TEXT } else { theme::TEXT_MUTED },
        );
    }
}

unsafe fn draw_primary_button(hdc: *mut c_void, rect: UiRect, label: &str, font: *mut c_void) {
    unsafe {
        draw_card(hdc, rect, theme::ACCENT, theme::ACCENT);
        draw_text(hdc, rect.left + 16, rect.top + 9, label, font, theme::BACKGROUND_DEEP);
    }
}

unsafe fn draw_secondary_button(hdc: *mut c_void, rect: UiRect, label: &str, font: *mut c_void) {
    unsafe {
        draw_card(hdc, rect, theme::SURFACE_RAISED, theme::LINE_STRONG);
        draw_text(hdc, rect.left + 15, rect.top + 9, label, font, theme::TEXT);
    }
}

unsafe fn draw_dot(hdc: *mut c_void, x: i32, y: i32, color: u32) {
    let brush = unsafe { CreateSolidBrush(color) };
    let pen = unsafe { CreatePen(0, 1, color) };
    let previous_brush = unsafe { SelectObject(hdc, brush) };
    let previous_pen = unsafe { SelectObject(hdc, pen) };

    unsafe {
        Ellipse(hdc, x, y, x + 8, y + 8);
        SelectObject(hdc, previous_pen);
        SelectObject(hdc, previous_brush);
        DeleteObject(pen);
        DeleteObject(brush);
    }
}

unsafe fn draw_card(hdc: *mut c_void, rect: UiRect, fill: u32, border: u32) {
    let brush = unsafe { CreateSolidBrush(fill) };
    let pen = unsafe { CreatePen(0, 1, border) };
    let previous_brush = unsafe { SelectObject(hdc, brush) };
    let previous_pen = unsafe { SelectObject(hdc, pen) };

    unsafe {
        RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, 16, 16);
        SelectObject(hdc, previous_pen);
        SelectObject(hdc, previous_brush);
        DeleteObject(pen);
        DeleteObject(brush);
    }
}

unsafe fn draw_text(
    hdc: *mut c_void,
    x: i32,
    y: i32,
    text: &str,
    font: *mut c_void,
    color: u32,
) {
    let text = text.encode_utf16().collect::<Vec<_>>();
    let previous_font = unsafe { SelectObject(hdc, font) };

    unsafe {
        SetTextColor(hdc, color);
        SetBkMode(hdc, 1);
        TextOutW(hdc, x, y, text.as_ptr(), text.len() as i32);
        SelectObject(hdc, previous_font);
    }
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
