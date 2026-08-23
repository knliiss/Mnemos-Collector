use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};

use anyhow::{Context, Result};
use tokio::runtime::Handle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, EndPaint, FillRect,
    InvalidateRect, PAINTSTRUCT, RoundRect, SelectObject, SetBkColor, SetBkMode, SetTextColor,
    TextOutW, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, GWLP_USERDATA, GetClientRect, GetCursorPos,
    GetMessageW, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, IDC_ARROW,
    IDI_APPLICATION, LoadCursorW, LoadIconW, MF_STRING, MSG, MessageBoxW, MoveWindow, PostMessageW,
    PostQuitMessage, RegisterClassW, SIZE_MINIMIZED, SW_HIDE, SW_RESTORE, SW_SHOW, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TrackPopupMenu, TranslateMessage, WM_APP, WM_CLOSE, WM_COMMAND, WM_CTLCOLOREDIT,
    WM_DESTROY, WM_LBUTTONUP, WM_NCCREATE, WM_PAINT, WM_RBUTTONUP, WM_SETFONT, WM_SIZE, WM_TIMER,
    WNDCLASSW, WS_CHILD, WS_HSCROLL, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};
use zeroize::Zeroizing;

use crate::application::CollectorApplication;
use crate::diagnostics;
use crate::platform::{Autostart, Installation};
use crate::provisioning::{ProvisioningClient, default_device_name};
use crate::security::CredentialStore;

use super::DesktopLaunchContext;
use super::mascot;

const CLASS_NAME: &str = "MnemosCollectorWindow";
const WINDOW_TITLE: &str = "Mnemos Collector";
const TIMER_REFRESH: usize = 1;
const WM_TRAY: u32 = WM_APP + 1;
const WM_ACTIVATION_RESULT: u32 = WM_APP + 2;
const WM_COLLECTOR_STOPPED: u32 = WM_APP + 3;
const TRAY_ID: u32 = 1;
const MENU_OPEN: usize = 4101;
const MENU_EXIT: usize = 4102;

const COLOR_BACKGROUND: u32 = 0x000b0d0b;
const COLOR_CARD: u32 = 0x00111411;
const COLOR_CARD_ALT: u32 = 0x00161b13;
const COLOR_BORDER: u32 = 0x00303a23;
const COLOR_ACCENT: u32 = 0x002fffbe;
const COLOR_TEXT: u32 = 0x00f1f4ee;
const COLOR_MUTED: u32 = 0x00949d91;
const COLOR_DANGER: u32 = 0x006b6bff;

#[derive(Clone, Copy)]
struct UiRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl UiRect {
    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
}

struct Layout {
    hero: UiRect,
    activation: Option<UiRect>,
    token_edit: UiRect,
    device_edit: UiRect,
    activate_button: UiRect,
    tray_button: UiRect,
    debug_toggle: UiRect,
    logs: UiRect,
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
            style: CS_HREDRAW | CS_VREDRAW,
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
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            980,
            760,
            null_mut(),
            null_mut(),
            instance,
            state_ptr.cast::<c_void>(),
        );

        if hwnd.is_null() {
            drop(Box::from_raw(state_ptr));
            return Err(io::Error::last_os_error()).context("failed to create collector window");
        }

        (*state_ptr).initialize_controls(hwnd, instance)?;
        (*state_ptr).install_tray_icon(hwnd)?;
        (*state_ptr).start_collector_if_ready(hwnd);

        SetTimer(hwnd, TIMER_REFRESH, 500, None);
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
    mono_font: *mut c_void,
    edit_brush: *mut c_void,
    last_log_text: String,
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
            mono_font: null_mut(),
            edit_brush: null_mut(),
            last_log_text: String::new(),
        }
    }

    unsafe fn initialize_controls(&mut self, hwnd: HWND, instance: *mut c_void) -> Result<()> {
        let segoe = wide("Segoe UI");
        let mono = wide("Cascadia Mono");

        self.ui_font =
            unsafe { CreateFontW(-18, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, segoe.as_ptr()) };
        self.title_font =
            unsafe { CreateFontW(-30, 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, segoe.as_ptr()) };
        self.mono_font =
            unsafe { CreateFontW(-16, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, mono.as_ptr()) };
        self.edit_brush = unsafe { CreateSolidBrush(COLOR_CARD_ALT) };

        self.token_edit = unsafe { create_edit(hwnd, instance, false, self.ui_font) };
        self.device_edit = unsafe { create_edit(hwnd, instance, false, self.ui_font) };
        self.logs_edit = unsafe { create_edit(hwnd, instance, true, self.mono_font) };

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
            unsafe {
                InvalidateRect(hwnd, null(), 0);
            }
            return;
        }

        self.provisioning = true;
        self.activation_error = None;

        let empty = wide("");
        unsafe {
            SetWindowTextW(self.token_edit, empty.as_ptr());
            InvalidateRect(hwnd, null(), 0);
        }

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

        unsafe {
            InvalidateRect(hwnd, null(), 0);
        }
    }

    fn update_control_visibility(&self) {
        let activation_visibility = if self.provisioned { SW_HIDE } else { SW_SHOW };

        unsafe {
            ShowWindow(self.token_edit, activation_visibility);
            ShowWindow(self.device_edit, activation_visibility);
        }
    }

    fn layout_controls(&self, hwnd: HWND) {
        let mut client: RECT = unsafe { std::mem::zeroed() };

        unsafe {
            GetClientRect(hwnd, &mut client);
        }

        let layout = layout(
            client.right - client.left,
            client.bottom - client.top,
            self.provisioned,
        );

        unsafe {
            MoveWindow(
                self.token_edit,
                layout.token_edit.left,
                layout.token_edit.top,
                layout.token_edit.right - layout.token_edit.left,
                layout.token_edit.bottom - layout.token_edit.top,
                1,
            );
            MoveWindow(
                self.device_edit,
                layout.device_edit.left,
                layout.device_edit.top,
                layout.device_edit.right - layout.device_edit.left,
                layout.device_edit.bottom - layout.device_edit.top,
                1,
            );
            MoveWindow(
                self.logs_edit,
                layout.logs.left,
                layout.logs.top,
                layout.logs.right - layout.logs.left,
                layout.logs.bottom - layout.logs.top,
                1,
            );
        }
    }

    fn refresh_logs(&mut self) {
        let text = diagnostics::recent_text();

        if text == self.last_log_text {
            return;
        }

        self.last_log_text = text.clone();
        let text = wide(&text);

        unsafe {
            SetWindowTextW(self.logs_edit, text.as_ptr());
        }
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
        let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
        let mut client: RECT = unsafe { std::mem::zeroed() };

        unsafe {
            GetClientRect(hwnd, &mut client);
        }

        let background = unsafe { CreateSolidBrush(COLOR_BACKGROUND) };
        unsafe {
            FillRect(hdc, &client, background);
            DeleteObject(background);
            SetBkMode(hdc, 1);
        }

        let width = client.right - client.left;
        let height = client.bottom - client.top;
        let layout = layout(width, height, self.provisioned);
        let runtime = diagnostics::runtime_snapshot();

        unsafe {
            draw_text(hdc, 32, 26, "MNEMOS", self.ui_font, COLOR_ACCENT);
            draw_text(hdc, 32, 50, "Collector", self.title_font, COLOR_TEXT);
            draw_text(
                hdc,
                32,
                86,
                "Нативный клиент наблюдения за Cristalix / Master Sword",
                self.ui_font,
                COLOR_MUTED,
            );

            draw_card(hdc, layout.hero, COLOR_CARD, COLOR_BORDER);
            draw_runtime_status(hdc, &runtime, layout.hero, self.ui_font, self.title_font);
            mascot::draw(hdc, layout.hero.right - 175, layout.hero.top + 4, 145);

            draw_button(
                hdc,
                layout.tray_button,
                "Свернуть в трей",
                false,
                self.ui_font,
            );

            if let Some(activation) = layout.activation {
                draw_card(hdc, activation, COLOR_CARD, COLOR_BORDER);
                draw_text(
                    hdc,
                    activation.left + 18,
                    activation.top + 16,
                    if self.current_installation {
                        "Подключить Collector"
                    } else {
                        "Установить и подключить Collector"
                    },
                    self.title_font,
                    COLOR_TEXT,
                );
                draw_text(
                    hdc,
                    activation.left + 18,
                    activation.top + 52,
                    "Одноразовый код активации",
                    self.ui_font,
                    COLOR_MUTED,
                );
                draw_text(
                    hdc,
                    layout.device_edit.left,
                    activation.top + 52,
                    "Имя устройства",
                    self.ui_font,
                    COLOR_MUTED,
                );
                draw_button(
                    hdc,
                    layout.activate_button,
                    if self.provisioning {
                        "Подключаем..."
                    } else {
                        "Активировать"
                    },
                    true,
                    self.ui_font,
                );

                if let Some(error) = self.activation_error.as_deref() {
                    draw_text(
                        hdc,
                        activation.left + 18,
                        activation.bottom - 28,
                        error,
                        self.ui_font,
                        COLOR_DANGER,
                    );
                }
            }

            let logs_title_y = layout.logs.top - 38;
            draw_text(
                hdc,
                layout.logs.left,
                logs_title_y,
                "Логи Collector",
                self.title_font,
                COLOR_TEXT,
            );

            let debug_label = if diagnostics::debug_enabled() {
                "● Подробная диагностика"
            } else {
                "○ Подробная диагностика"
            };
            draw_text(
                hdc,
                layout.debug_toggle.left + 8,
                layout.debug_toggle.top + 7,
                debug_label,
                self.ui_font,
                if diagnostics::debug_enabled() {
                    COLOR_ACCENT
                } else {
                    COLOR_MUTED
                },
            );

            if let Some(path) = diagnostics::log_file_path() {
                draw_text(
                    hdc,
                    layout.logs.left,
                    layout.logs.bottom + 8,
                    &format!("Файл: {}", path.display()),
                    self.ui_font,
                    COLOR_MUTED,
                );
            }

            EndPaint(hwnd, &paint);
        }
    }

    fn click(&mut self, hwnd: HWND, x: i32, y: i32) {
        let mut client: RECT = unsafe { std::mem::zeroed() };

        unsafe {
            GetClientRect(hwnd, &mut client);
        }

        let layout = layout(client.right, client.bottom, self.provisioned);

        if layout.tray_button.contains(x, y) {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
            return;
        }

        if layout.debug_toggle.contains(x, y) {
            diagnostics::set_debug_enabled(!diagnostics::debug_enabled());
            unsafe {
                InvalidateRect(hwnd, null(), 0);
            }
            return;
        }

        if !self.provisioned && layout.activate_button.contains(x, y) {
            self.begin_activation(hwnd);
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
            if !self.ui_font.is_null() {
                DeleteObject(self.ui_font);
            }
            if !self.title_font.is_null() {
                DeleteObject(self.title_font);
            }
            if !self.mono_font.is_null() {
                DeleteObject(self.mono_font);
            }
            if !self.edit_brush.is_null() {
                DeleteObject(self.edit_brush);
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
                }
            }
        }
        WM_TIMER => {
            if wparam == TIMER_REFRESH && !state.is_null() {
                unsafe {
                    (*state).refresh_logs();
                    InvalidateRect(hwnd, null(), 0);
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
        WM_CTLCOLOREDIT => {
            if !state.is_null() {
                unsafe {
                    let hdc = wparam as *mut c_void;
                    SetTextColor(hdc, COLOR_TEXT);
                    SetBkColor(hdc, COLOR_CARD_ALT);
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

unsafe fn create_edit(hwnd: HWND, instance: *mut c_void, logs: bool, font: *mut c_void) -> HWND {
    let class = wide("EDIT");
    let empty = wide("");
    let style = if logs {
        WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WS_HSCROLL
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32
            | ES_AUTOHSCROLL as u32
            | ES_READONLY as u32
    } else {
        WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32
    };
    let edit = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            empty.as_ptr(),
            style,
            0,
            0,
            100,
            30,
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

fn layout(width: i32, height: i32, provisioned: bool) -> Layout {
    let margin = 28;
    let hero = UiRect {
        left: margin,
        top: 112,
        right: width - margin,
        bottom: 260,
    };
    let tray_button = UiRect {
        left: hero.right - 330,
        top: hero.bottom - 48,
        right: hero.right - 185,
        bottom: hero.bottom - 16,
    };

    let (activation, logs_top) = if provisioned {
        (None, 320)
    } else {
        (
            Some(UiRect {
                left: margin,
                top: 280,
                right: width - margin,
                bottom: 430,
            }),
            490,
        )
    };

    let activation_rect = activation.unwrap_or(UiRect {
        left: margin,
        top: 0,
        right: width - margin,
        bottom: 0,
    });
    let inner_width = activation_rect.right - activation_rect.left - 36;
    let device_width = 210;
    let button_width = 150;
    let gap = 12;
    let token_width = (inner_width - device_width - button_width - gap * 2).max(220);
    let edit_top = activation_rect.top + 76;
    let token_edit = UiRect {
        left: activation_rect.left + 18,
        top: edit_top,
        right: activation_rect.left + 18 + token_width,
        bottom: edit_top + 36,
    };
    let device_edit = UiRect {
        left: token_edit.right + gap,
        top: edit_top,
        right: token_edit.right + gap + device_width,
        bottom: edit_top + 36,
    };
    let activate_button = UiRect {
        left: device_edit.right + gap,
        top: edit_top,
        right: activation_rect.right - 18,
        bottom: edit_top + 36,
    };
    let debug_toggle = UiRect {
        left: width - margin - 230,
        top: logs_top - 42,
        right: width - margin,
        bottom: logs_top - 8,
    };
    let logs = UiRect {
        left: margin,
        top: logs_top,
        right: width - margin,
        bottom: (height - 54).max(logs_top + 80),
    };

    Layout {
        hero,
        activation,
        token_edit,
        device_edit,
        activate_button,
        tray_button,
        debug_toggle,
        logs,
    }
}

unsafe fn draw_runtime_status(
    hdc: *mut c_void,
    runtime: &diagnostics::RuntimeSnapshot,
    rect: UiRect,
    font: *mut c_void,
    title_font: *mut c_void,
) {
    let (title, detail, color) = if runtime.observing {
        (
            "Наблюдение активно",
            "Master Sword распознан, realtime-service подтвердил OBSERVING.",
            COLOR_ACCENT,
        )
    } else if !runtime.cristalix_running {
        (
            "Ожидание Cristalix",
            "Collector работает в фоне и сам подключится, когда игра появится.",
            COLOR_TEXT,
        )
    } else if runtime.game_mode == "MasterSword" && !runtime.realtime_connected {
        (
            "Master Sword найден, нет связи",
            "Режим распознан, но realtime-соединение ещё не подтверждено.",
            COLOR_DANGER,
        )
    } else {
        (
            "Cristalix найден",
            "Collector анализирует текущий latest.log и восстанавливает контекст режима.",
            COLOR_TEXT,
        )
    };

    unsafe {
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 18,
            "СОСТОЯНИЕ",
            font,
            COLOR_ACCENT,
        );
        draw_text(hdc, rect.left + 20, rect.top + 44, title, title_font, color);
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 82,
            detail,
            font,
            COLOR_MUTED,
        );

        let mode = if runtime.game_mode.is_empty() {
            "Unknown"
        } else {
            runtime.game_mode.as_str()
        };
        draw_text(
            hdc,
            rect.left + 20,
            rect.top + 112,
            &format!(
                "Cristalix: {}   •   Режим: {}   •   WSS: {}   •   OBSERVING: {}",
                yes_no(runtime.cristalix_running),
                mode,
                yes_no(runtime.realtime_connected),
                yes_no(runtime.observing),
            ),
            font,
            COLOR_MUTED,
        );
    }
}

unsafe fn draw_card(hdc: *mut c_void, rect: UiRect, fill: u32, border: u32) {
    let brush = unsafe { CreateSolidBrush(fill) };
    let pen = unsafe { CreatePen(0, 1, border) };
    let old_brush = unsafe { SelectObject(hdc, brush) };
    let old_pen = unsafe { SelectObject(hdc, pen) };

    unsafe {
        RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, 18, 18);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        DeleteObject(pen);
        DeleteObject(brush);
    }
}

unsafe fn draw_button(
    hdc: *mut c_void,
    rect: UiRect,
    label: &str,
    accent: bool,
    font: *mut c_void,
) {
    unsafe {
        draw_card(
            hdc,
            rect,
            if accent { COLOR_ACCENT } else { COLOR_CARD_ALT },
            if accent { COLOR_ACCENT } else { COLOR_BORDER },
        );
        draw_text(
            hdc,
            rect.left + 12,
            rect.top + 8,
            label,
            font,
            if accent { COLOR_BACKGROUND } else { COLOR_TEXT },
        );
    }
}

unsafe fn draw_text(hdc: *mut c_void, x: i32, y: i32, text: &str, font: *mut c_void, color: u32) {
    let text = text.encode_utf16().collect::<Vec<_>>();
    let old_font = unsafe { SelectObject(hdc, font) };

    unsafe {
        SetTextColor(hdc, color);
        SetBkMode(hdc, 1);
        TextOutW(hdc, x, y, text.as_ptr(), text.len() as i32);
        SelectObject(hdc, old_font);
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "да" } else { "нет" }
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
