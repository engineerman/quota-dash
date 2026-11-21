//! DPAPI-encrypted local secrets.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static GATE: Mutex<()> = Mutex::new(());

const ENTROPY: &[u8] = b"quota-dash.v1";

fn secrets_dir() -> PathBuf {
    if let Ok(p) = std::env::var("QUOTA_DASH_SECRETS_DIR") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("quota-dash").join("secrets")
}

fn path_for(name: &str) -> PathBuf {
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    let digest = format!("{:016x}{:016x}", h.finish(), name.len() as u64);
    let safe: String = digest.chars().take(32).collect();
    secrets_dir().join(format!("{safe}.bin"))
}

/// Override secrets directory (tests).
pub fn set_secrets_dir_for_test(dir: &Path) {
    std::env::set_var("QUOTA_DASH_SECRETS_DIR", dir);
}

pub fn set(name: &str, value: Option<&str>) -> Result<(), String> {
    let p = path_for(name);
    let _g = GATE.lock().map_err(|_| "secret store lock".to_string())?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match value {
        None => {
            if p.exists() {
                std::fs::remove_file(&p).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Some(v) => {
            let blob = dpapi_protect(v.as_bytes())?;
            let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&tmp, &blob).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
            Ok(())
        }
    }
}

pub fn get(name: &str) -> Option<String> {
    let p = path_for(name);
    if !p.exists() {
        return None;
    }
    let _g = GATE.lock().ok()?;
    let raw = std::fs::read(&p).ok()?;
    let plain = dpapi_unprotect(&raw).ok()?;
    String::from_utf8(plain).ok()
}

#[cfg(windows)]
fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    let mut data_buf = data.to_vec();
    let mut entropy = ENTROPY.to_vec();
    let mut in_blob = CRYPT_INTEGER_BLOB {
        cbData: data_buf.len() as u32,
        pbData: data_buf.as_mut_ptr(),
    };
    let mut ent_blob = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_mut_ptr(),
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &mut in_blob,
            None,
            Some(&mut ent_blob),
            None,
            None,
            0,
            &mut out_blob,
        )
    };
    if ok.is_err() {
        return Err("DPAPI CryptProtectData failed".into());
    }
    let slice = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
    let vec = slice.to_vec();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out_blob.pbData as *mut _)));
    }
    Ok(vec)
}

#[cfg(windows)]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let mut data_buf = data.to_vec();
    let mut entropy = ENTROPY.to_vec();
    let mut in_blob = CRYPT_INTEGER_BLOB {
        cbData: data_buf.len() as u32,
        pbData: data_buf.as_mut_ptr(),
    };
    let mut ent_blob = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_mut_ptr(),
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &mut in_blob,
            None,
            Some(&mut ent_blob),
            None,
            None,
            0,
            &mut out_blob,
        )
    };
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
fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut v = b"PLAIN:".to_vec();
    v.extend_from_slice(data);
    Ok(v)
}

#[cfg(not(windows))]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    data.strip_prefix(b"PLAIN:")
        .map(|s| s.to_vec())
        .ok_or_else(|| "bad plaintext secret".into())
}

#[cfg(test)]
pub static TEST_SECRETS_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip() {
        let _guard = TEST_SECRETS_LOCK.lock().unwrap();
        let dir = tempdir().unwrap();
        set_secrets_dir_for_test(dir.path());
        set("demo", Some("hello")).unwrap();
        assert_eq!(get("demo").as_deref(), Some("hello"));
        set("demo", None).unwrap();
        assert!(get("demo").is_none());
    }
}
