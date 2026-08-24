use std::env;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::copy_nonoverlapping;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use windows_sys::Win32::Foundation::{GlobalFree, HWND, POINT};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::UI::Shell::DROPFILES;

const CF_UNICODETEXT_FORMAT: u32 = 13;
const CF_HDROP_FORMAT: u32 = 15;
const CLIPBOARD_OPEN_ATTEMPTS: usize = 8;
const CLIPBOARD_OPEN_RETRY_DELAY: Duration = Duration::from_millis(10);
const TEMP_DIRECTORY_NAME: &str = "mnemos-collector-clipboard";
const TEMP_LOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LogCopyFormat {
    File,
    Text,
}

pub(super) fn copy_text(owner: HWND, text: &str) -> Result<()> {
    let _clipboard = open_clipboard(owner)?;

    unsafe {
        if EmptyClipboard() == 0 {
            return Err(io::Error::last_os_error()).context("failed to clear Windows clipboard");
        }
    }

    set_unicode_text(text).context("failed to place text on Windows clipboard")
}

pub(super) fn copy_log(owner: HWND, text: &str) -> Result<LogCopyFormat> {
    let temporary_file = create_temporary_log(text).ok();
    let _clipboard = open_clipboard(owner)?;

    unsafe {
        if EmptyClipboard() == 0 {
            return Err(io::Error::last_os_error()).context("failed to clear Windows clipboard");
        }
    }

    let file_copied = temporary_file
        .as_deref()
        .is_some_and(|path| set_file_path(path).is_ok());
    let text_copied = set_unicode_text(text).is_ok();

    if file_copied {
        return Ok(LogCopyFormat::File);
    }

    if let Some(path) = temporary_file {
        let _ = fs::remove_file(path);
    }

    if text_copied {
        return Ok(LogCopyFormat::Text);
    }

    Err(io::Error::last_os_error()).context("failed to place log on Windows clipboard")
}

fn open_clipboard(owner: HWND) -> Result<ClipboardGuard> {
    for attempt in 0..CLIPBOARD_OPEN_ATTEMPTS {
        if unsafe { OpenClipboard(owner) } != 0 {
            return Ok(ClipboardGuard);
        }

        if attempt + 1 < CLIPBOARD_OPEN_ATTEMPTS {
            thread::sleep(CLIPBOARD_OPEN_RETRY_DELAY);
        }
    }

    Err(io::Error::last_os_error()).context("failed to open Windows clipboard")
}

fn set_unicode_text(text: &str) -> Result<()> {
    let encoded = text
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let byte_length = encoded.len() * size_of::<u16>();
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, byte_length) };

    if memory.is_null() {
        return Err(io::Error::last_os_error()).context("failed to allocate clipboard text memory");
    }

    let destination = unsafe { GlobalLock(memory) };

    if destination.is_null() {
        unsafe {
            GlobalFree(memory);
        }

        return Err(io::Error::last_os_error()).context("failed to lock clipboard text memory");
    }

    unsafe {
        copy_nonoverlapping(encoded.as_ptr(), destination.cast::<u16>(), encoded.len());
        GlobalUnlock(memory);
    }

    let result = unsafe { SetClipboardData(CF_UNICODETEXT_FORMAT, memory) };

    if result.is_null() {
        unsafe {
            GlobalFree(memory);
        }

        return Err(io::Error::last_os_error()).context("failed to set clipboard text data");
    }

    Ok(())
}

fn set_file_path(path: &Path) -> Result<()> {
    let encoded_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let drop_files_size = size_of::<DROPFILES>();
    let byte_length = drop_files_size + encoded_path.len() * size_of::<u16>();
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, byte_length) };

    if memory.is_null() {
        return Err(io::Error::last_os_error()).context("failed to allocate clipboard file memory");
    }

    let destination = unsafe { GlobalLock(memory) };

    if destination.is_null() {
        unsafe {
            GlobalFree(memory);
        }

        return Err(io::Error::last_os_error()).context("failed to lock clipboard file memory");
    }

    let descriptor = DROPFILES {
        pFiles: drop_files_size as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: 0,
        fWide: 1,
    };

    unsafe {
        destination.cast::<DROPFILES>().write(descriptor);

        let path_destination = destination.cast::<u8>().add(drop_files_size).cast::<u16>();
        copy_nonoverlapping(encoded_path.as_ptr(), path_destination, encoded_path.len());

        GlobalUnlock(memory);
    }

    let result = unsafe { SetClipboardData(CF_HDROP_FORMAT, memory) };

    if result.is_null() {
        unsafe {
            GlobalFree(memory);
        }

        return Err(io::Error::last_os_error()).context("failed to set clipboard file data");
    }

    Ok(())
}

fn create_temporary_log(text: &str) -> Result<PathBuf> {
    let directory = env::temp_dir().join(TEMP_DIRECTORY_NAME);

    fs::create_dir_all(&directory).with_context(|| {
        format!(
            "failed to create clipboard log directory {}",
            directory.display()
        )
    })?;

    cleanup_stale_logs(&directory);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = format!(
        "mnemos-collector-log-{}-{timestamp}.txt",
        std::process::id()
    );
    let path = directory.join(file_name);

    fs::write(&path, text.as_bytes())
        .with_context(|| format!("failed to write clipboard log file {}", path.display()))?;

    Ok(path)
}

fn cleanup_stale_logs(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(age) = modified.elapsed() else {
            continue;
        };

        if metadata.is_file() && age >= TEMP_LOG_MAX_AGE {
            let _ = fs::remove_file(path);
        }
    }
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{TEMP_DIRECTORY_NAME, create_temporary_log};

    #[test]
    fn temporary_log_uses_txt_extension_and_preserves_content() {
        let text = "[INFO] first line\n[DEBUG] second line";

        let path = create_temporary_log(text).expect("temporary clipboard log should be created");
        let stored = fs::read_to_string(&path).expect("temporary clipboard log should be readable");

        assert_eq!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("txt")
        );
        assert!(path.to_string_lossy().contains(TEMP_DIRECTORY_NAME));
        assert_eq!(stored, text);

        fs::remove_file(path).expect("temporary clipboard log should be removable");
    }
}
