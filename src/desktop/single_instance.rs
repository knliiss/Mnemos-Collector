use std::io;
use std::ptr::{null, null_mut};

use anyhow::{Context, Result};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow,
};

const INSTANCE_MUTEX_NAME: &str = "Local\\MnemosCollector.SingleInstance";
const WINDOW_CLASS_NAME: &str = "MnemosCollectorShell";

pub(super) struct InstanceGuard {
    handle: HANDLE,
}

impl InstanceGuard {
    pub(super) fn acquire() -> Result<Option<Self>> {
        let name = wide(INSTANCE_MUTEX_NAME);
        let handle = unsafe { CreateMutexW(null_mut(), 0, name.as_ptr()) };

        if handle.is_null() {
            return Err(io::Error::last_os_error())
                .context("failed to create Collector single-instance mutex");
        }

        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

        if already_running {
            unsafe {
                CloseHandle(handle);
            }

            return Ok(None);
        }

        Ok(Some(Self { handle }))
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

pub(super) fn activate_existing_window() {
    let class_name = wide(WINDOW_CLASS_NAME);
    let hwnd = unsafe { FindWindowW(class_name.as_ptr(), null()) };

    if hwnd.is_null() {
        return;
    }

    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::InstanceGuard;

    #[test]
    fn only_one_collector_instance_guard_can_be_held() {
        let first = InstanceGuard::acquire()
            .expect("first single-instance guard acquisition should succeed")
            .expect("first Collector instance should own the guard");

        let second = InstanceGuard::acquire()
            .expect("second single-instance guard acquisition should be checked");

        assert!(second.is_none());

        drop(first);

        let after_release = InstanceGuard::acquire()
            .expect("single-instance guard should be reusable after release");

        assert!(after_release.is_some());
    }
}
