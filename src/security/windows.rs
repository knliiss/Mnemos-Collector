use std::ffi::c_void;
use std::io;
use std::ptr::null_mut;
use std::slice;

use anyhow::{Context, Result, bail};
use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use zeroize::Zeroize;

pub fn load(target: &str) -> Result<Option<String>> {
    let target = encode_wide_terminated(target);
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

        decode_password_blob(bytes)
    };

    unsafe {
        CredFree(credential.cast::<c_void>());
    }

    value.map(Some)
}

pub fn save(target: &str, username: &str, value: &str) -> Result<()> {
    let mut target = encode_wide_terminated(target);
    let mut username = encode_wide_terminated(username);
    let mut password = value.encode_utf16().collect::<Vec<_>>();
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };

    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = target.as_mut_ptr();
    credential.CredentialBlobSize = (password.len() * std::mem::size_of::<u16>()) as u32;
    credential.CredentialBlob = password.as_mut_ptr().cast::<u8>();
    credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
    credential.UserName = username.as_mut_ptr();

    let written = unsafe { CredWriteW(&credential, 0) };
    password.zeroize();

    if written == 0 {
        let error = unsafe { GetLastError() };

        return Err(io::Error::from_raw_os_error(error as i32))
            .context("failed to write Windows Credential Manager entry");
    }

    Ok(())
}

pub fn delete(target: &str) -> Result<()> {
    let target = encode_wide_terminated(target);
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

fn decode_password_blob(bytes: &[u8]) -> Result<String> {
    let (pairs, remainder) = bytes.as_chunks::<2>();

    if !remainder.is_empty() {
        bail!("Windows credential password blob has an invalid UTF-16 length");
    }

    let password = pairs
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();

    String::from_utf16(&password).context("Windows credential password blob is not valid UTF-16")
}

fn encode_wide_terminated(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::decode_password_blob;

    #[test]
    fn decodes_keyring_compatible_utf16_password_blob() {
        let password = "collector-secret";
        let encoded = password
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();

        assert_eq!(decode_password_blob(&encoded).unwrap(), password);
    }

    #[test]
    fn rejects_odd_length_password_blob() {
        assert!(decode_password_blob(&[0x41]).is_err());
    }
}
