//! Chromium/Electron safeStorage envelopes (Cursor `cursor-safe-storage:v1:`).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::Value;

pub const CURSOR_PREFIX: &str = "cursor-safe-storage:v1:";

/// Returns `value` unchanged when not sealed; otherwise AES-256-GCM decrypts it.
pub fn unseal(value: &str, key: Option<&[u8]>) -> Result<String, String> {
    if !value.starts_with(CURSOR_PREFIX) {
        return Ok(value.to_string());
    }
    let key = key
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "Sealed Cursor token needs the Chromium Safe Storage key.".to_string())?;
    let blob = B64
        .decode(&value[CURSOR_PREFIX.len()..])
        .map_err(|e| format!("safe-storage base64: {e}"))?;
    if blob.len() < 3 + 12 + 16 || &blob[0..3] != b"v10" {
        return Err("Unsupported Cursor safe-storage ciphertext.".into());
    }
    let nonce: Nonce<_> = blob[3..15]
        .try_into()
        .map_err(|_| "bad nonce length".to_string())?;
    let tag = &blob[blob.len() - 16..];
    let ct = &blob[15..blob.len() - 16];
    let mut combined = Vec::with_capacity(ct.len() + tag.len());
    combined.extend_from_slice(ct);
    combined.extend_from_slice(tag);

    let key: Key<Aes256Gcm> = key
        .try_into()
        .map_err(|_| "Chromium Safe Storage key must be 32 bytes".to_string())?;
    let cipher = Aes256Gcm::new(&key);
    let plain = cipher
        .decrypt(&nonce, combined.as_ref())
        .map_err(|_| "AES-GCM decrypt failed for Cursor token.".to_string())?;
    String::from_utf8(plain).map_err(|e| format!("token utf8: {e}"))
}

/// Reads `%APPDATA%\{app}\Local State` → `os_crypt.encrypted_key` and DPAPI-unprotects it.
pub fn read_local_state_key(app_name: &str) -> Option<Vec<u8>> {
    let path = dirs_appdata()?.join(app_name).join("Local State");
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let ek = v.pointer("/os_crypt/encrypted_key")?.as_str()?;
    let raw = B64.decode(ek).ok()?;
    if raw.len() < 5 || &raw[0..5] != b"DPAPI" {
        return None;
    }
    dpapi_unprotect(&raw[5..]).ok()
}

fn dirs_appdata() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(std::path::PathBuf::from)
}

#[cfg(windows)]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let mut data_buf = data.to_vec();
    let mut in_blob = CRYPT_INTEGER_BLOB {
        cbData: data_buf.len() as u32,
        pbData: data_buf.as_mut_ptr(),
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe { CryptUnprotectData(&mut in_blob, None, None, None, None, 0, &mut out_blob) };
    if ok.is_err() {
        return Err("DPAPI CryptUnprotectData failed".into());
    }
    let slice = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
    let vec = slice.to_vec();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out_blob.pbData as *mut _)));
    }
    Ok(vec)
}

#[cfg(not(windows))]
fn dpapi_unprotect(_data: &[u8]) -> Result<Vec<u8>, String> {
    Err("DPAPI only available on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_passthrough() {
        assert_eq!(unseal("raw-token", None).unwrap(), "raw-token");
    }
}
