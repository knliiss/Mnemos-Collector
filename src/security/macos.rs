use std::ffi::c_void;
use std::ptr::{null, null_mut};

use anyhow::{Result, bail};

const ERR_SEC_SUCCESS: i32 = 0;
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecKeychainFindGenericPassword(
        keychain_or_array: *const c_void,
        service_name_length: u32,
        service_name: *const u8,
        account_name_length: u32,
        account_name: *const u8,
        password_length: *mut u32,
        password_data: *mut *mut c_void,
        item_ref: *mut *mut c_void,
    ) -> i32;

    fn SecKeychainAddGenericPassword(
        keychain: *const c_void,
        service_name_length: u32,
        service_name: *const u8,
        account_name_length: u32,
        account_name: *const u8,
        password_length: u32,
        password_data: *const c_void,
        item_ref: *mut *mut c_void,
    ) -> i32;

    fn SecKeychainItemModifyAttributesAndData(
        item_ref: *mut c_void,
        attribute_list: *const c_void,
        data_length: u32,
        data: *const c_void,
    ) -> i32;

    fn SecKeychainItemDelete(item_ref: *mut c_void) -> i32;

    fn SecKeychainItemFreeContent(attribute_list: *const c_void, data: *mut c_void) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const c_void);
}

pub fn load(service: &str, account: &str) -> Result<Option<String>> {
    let mut password_length = 0_u32;
    let mut password_data = null_mut();

    let status = unsafe {
        SecKeychainFindGenericPassword(
            null(),
            checked_length(service)?,
            service.as_ptr(),
            checked_length(account)?,
            account.as_ptr(),
            &mut password_length,
            &mut password_data,
            null_mut(),
        )
    };

    if status == ERR_SEC_ITEM_NOT_FOUND {
        return Ok(None);
    }

    ensure_success(status, "read generic password")?;

    if password_data.is_null() {
        bail!("macOS Keychain returned a credential without password data");
    }

    let bytes =
        unsafe { std::slice::from_raw_parts(password_data.cast::<u8>(), password_length as usize) };
    let value = String::from_utf8(bytes.to_vec())
        .map_err(|error| anyhow::anyhow!("macOS Keychain credential is not valid UTF-8: {error}"));
    let free_status = unsafe { SecKeychainItemFreeContent(null(), password_data) };

    ensure_success(free_status, "free generic password content")?;

    value.map(Some)
}

pub fn save(service: &str, account: &str, value: &str) -> Result<()> {
    let service_length = checked_length(service)?;
    let account_length = checked_length(account)?;
    let value_length = checked_length(value)?;
    let mut item_ref = null_mut();

    let find_status = unsafe {
        SecKeychainFindGenericPassword(
            null(),
            service_length,
            service.as_ptr(),
            account_length,
            account.as_ptr(),
            null_mut(),
            null_mut(),
            &mut item_ref,
        )
    };

    if find_status == ERR_SEC_SUCCESS {
        let item = KeychainItem::new(item_ref)?;
        let status = unsafe {
            SecKeychainItemModifyAttributesAndData(
                item.as_ptr(),
                null(),
                value_length,
                value.as_ptr().cast(),
            )
        };

        return ensure_success(status, "update generic password");
    }

    if find_status != ERR_SEC_ITEM_NOT_FOUND {
        return ensure_success(find_status, "find generic password for update");
    }

    let add_status = unsafe {
        SecKeychainAddGenericPassword(
            null(),
            service_length,
            service.as_ptr(),
            account_length,
            account.as_ptr(),
            value_length,
            value.as_ptr().cast(),
            null_mut(),
        )
    };

    if add_status == ERR_SEC_DUPLICATE_ITEM {
        return save(service, account, value);
    }

    ensure_success(add_status, "add generic password")
}

pub fn delete(service: &str, account: &str) -> Result<()> {
    let mut item_ref = null_mut();
    let status = unsafe {
        SecKeychainFindGenericPassword(
            null(),
            checked_length(service)?,
            service.as_ptr(),
            checked_length(account)?,
            account.as_ptr(),
            null_mut(),
            null_mut(),
            &mut item_ref,
        )
    };

    if status == ERR_SEC_ITEM_NOT_FOUND {
        return Ok(());
    }

    ensure_success(status, "find generic password for deletion")?;

    let item = KeychainItem::new(item_ref)?;
    let delete_status = unsafe { SecKeychainItemDelete(item.as_ptr()) };

    ensure_success(delete_status, "delete generic password")
}

fn checked_length(value: &str) -> Result<u32> {
    u32::try_from(value.len()).map_err(|_| anyhow::anyhow!("Keychain value is too large"))
}

fn ensure_success(status: i32, operation: &str) -> Result<()> {
    if status == ERR_SEC_SUCCESS {
        return Ok(());
    }

    bail!("macOS Keychain failed to {operation} with OSStatus {status}")
}

struct KeychainItem(*mut c_void);

impl KeychainItem {
    fn new(value: *mut c_void) -> Result<Self> {
        if value.is_null() {
            bail!("macOS Keychain returned a null item reference");
        }

        Ok(Self(value))
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for KeychainItem {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0.cast_const());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_status_constants_match_security_framework() {
        assert_eq!(ERR_SEC_SUCCESS, 0);
        assert_eq!(ERR_SEC_DUPLICATE_ITEM, -25299);
        assert_eq!(ERR_SEC_ITEM_NOT_FOUND, -25300);
    }

    #[test]
    fn checked_length_rejects_nothing_in_normal_collector_values() {
        assert_eq!(checked_length("mnemos-collector").unwrap(), 16);
        assert_eq!(checked_length("collector-access-key").unwrap(), 20);
    }
}
