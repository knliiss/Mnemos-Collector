use std::ffi::c_void;
use std::io;
use std::ptr::null_mut;
use std::slice;

use anyhow::{Context, Result};
use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
use windows_sys::Win32::Security::Credentials::{
    CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use zeroize::Zeroize;

pub fn load(target: &str) -> Result<Option<String>> {
    let target = encode_wide(target);
    let mut credential: *mut CREDENTIALW = null_mut();

    let read = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };

    if read == 0 {
        let error = unsafe { GetLastError() };

        if error == ERROR_NOT_FOUND {
            return Ok(None);
        }

        return Err(io::Error::from_raw_os_error(error as i32))
            .context("failed to read Windows Credential Manager entry");
    }

    let value = unsafe {
        let credential_ref = &*credential;
        let bytes = slice::from_raw_parts(
            credential_ref.CredentialBlob,
            credential_ref.CredentialBlobSize as usize,
        );

        String::from_utf8(bytes.to_vec()).context("Windows credential contains invalid UTF-8")
    };

    unsafe {
        CredFree(credential.cast::<c_void>());
    }

    value.map(Some)
}

pub fn save(target: &str, value: &str) -> Result<()> {
    let mut target = encode_wide(target);
    let mut username = encode_wide("Mnemos Collector");
    let mut blob = value.as_bytes().to_vec();
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };

    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = target.as_mut_ptr();
    credential.CredentialBlobSize = blob.len() as u32;
    credential.CredentialBlob = blob.as_mut_ptr();
    credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
    credential.UserName = username.as_mut_ptr();

    let written = unsafe { CredWriteW(&credential, 0) };
    blob.zeroize();

    if written == 0 {
        let error = unsafe { GetLastError() };

        return Err(io::Error::from_raw_os_error(error as i32))
            .context("failed to write Windows Credential Manager entry");
    }

    Ok(())
}

pub fn delete(target: &str) -> Result<()> {
    let target = encode_wide(target);
    let deleted = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };

    if deleted != 0 {
        return Ok(());
    }

    let error = unsafe { GetLastError() };

    if error == ERROR_NOT_FOUND {
        return Ok(());
    }

    Err(io::Error::from_raw_os_error(error as i32))
        .context("failed to delete Windows Credential Manager entry")
}

fn encode_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
