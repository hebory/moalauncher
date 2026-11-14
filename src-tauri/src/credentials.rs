use crate::storage::{normalize_path, Paths};
use sha2::{Digest, Sha256};

const TARGET_PREFIX: &str = "MoaLauncher/GitHub/";
const MAX_BLOB: usize = 2560;

fn credential_target(paths: &Paths) -> String {
    let mut hasher = Sha256::new();
    hasher.update(normalize_path(&paths.data_dir).as_bytes());
    format!("{TARGET_PREFIX}{}", hex::encode(hasher.finalize()))
}

pub fn read_token(paths: &Paths) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        windows_impl::read(&credential_target(paths))
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
        Err("Windows 자격 증명 저장소를 사용할 수 없습니다.".into())
    }
}

pub fn save_token(paths: &Paths, token: &str) -> Result<(), String> {
    validate_token(token)?;
    #[cfg(windows)]
    {
        windows_impl::write(&credential_target(paths), token)
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
        Err("Windows 자격 증명 저장소를 사용할 수 없습니다.".into())
    }
}

pub fn delete_token(paths: &Paths) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_impl::delete(&credential_target(paths))
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
        Err("Windows 자격 증명 저장소를 사용할 수 없습니다.".into())
    }
}

fn validate_token(token: &str) -> Result<(), String> {
    if token.is_empty()
        || token.trim() != token
        || token.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(
            "GitHub 인증 정보를 확인하세요. 앞뒤 공백과 줄바꿈은 허용되지 않습니다.".into(),
        );
    }
    if token.len() > MAX_BLOB {
        return Err("GitHub 인증 정보가 Windows 자격 증명 저장 한도를 초과합니다.".into());
    }
    Ok(())
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::GetLastError,
        Security::Credentials::{
            CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
            CRED_TYPE_GENERIC,
        },
    };

    const ERROR_NOT_FOUND: u32 = 1168;
    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub(super) fn read(target: &str) -> Result<Option<String>, String> {
        let target = wide(target);
        let mut credential: *mut CREDENTIALW = ptr::null_mut();
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
        if ok == 0 {
            if unsafe { GetLastError() } == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err("Windows 자격 증명에서 GitHub 인증 정보를 읽지 못했습니다.".into());
        }
        if credential.is_null() {
            return Err("저장된 GitHub 인증 정보 형식이 올바르지 않습니다.".into());
        }
        struct Free(*const core::ffi::c_void);
        impl Drop for Free {
            fn drop(&mut self) {
                unsafe {
                    CredFree(self.0);
                }
            }
        }
        let _free = Free(credential.cast());
        let cred = unsafe { &*credential };
        let len = cred.CredentialBlobSize as usize;
        if cred.CredentialBlob.is_null() || len == 0 || len > MAX_BLOB {
            return Err("저장된 GitHub 인증 정보 형식이 올바르지 않습니다.".into());
        }
        let blob = unsafe { std::slice::from_raw_parts(cred.CredentialBlob, len) };
        let token = std::str::from_utf8(blob)
            .map_err(|_| "저장된 GitHub 인증 정보 형식이 올바르지 않습니다.")?
            .to_owned();
        validate_token(&token)
            .map_err(|_| "저장된 GitHub 인증 정보 형식이 올바르지 않습니다.".to_string())?;
        Ok(Some(token))
    }

    pub(super) fn write(target: &str, token: &str) -> Result<(), String> {
        let target = wide(target);
        let mut blob = token.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_ptr() as *mut u16,
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        let ok = unsafe { CredWriteW(&credential, 0) };
        blob.fill(0);
        if ok == 0 {
            return Err("Windows 자격 증명에 GitHub 인증 정보를 저장하지 못했습니다.".into());
        }
        Ok(())
    }

    pub(super) fn delete(target: &str) -> Result<(), String> {
        let target = wide(target);
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0 {
            return Ok(());
        }
        if unsafe { GetLastError() } == ERROR_NOT_FOUND {
            return Ok(());
        }
        Err("Windows 자격 증명에서 GitHub 인증 정보를 삭제하지 못했습니다.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    #[test]
    fn token_validation_rejects_surrounding_whitespace() {
        assert!(validate_token("ghp_example").is_ok());
        assert!(validate_token(" ghp_example").is_err());
        assert!(validate_token("ghp_example\n").is_err());
    }
    #[test]
    fn target_is_stable_and_scoped_to_data_path() {
        let a = Paths {
            data_dir: PathBuf::from(r"C:\Users\user\AppData\Local\MoaLauncher"),
            local_app_data: PathBuf::new(),
        };
        let b = Paths {
            data_dir: PathBuf::from(r"C:\Users\user\AppData\Local\MoaLauncher\"),
            local_app_data: PathBuf::new(),
        };
        assert_eq!(credential_target(&a), credential_target(&b));
        assert!(credential_target(&a).starts_with(TARGET_PREFIX));
    }
}
