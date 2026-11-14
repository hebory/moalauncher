use crate::storage::{Paths, normalize_path};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Command};

pub fn show_error(message: &str) {
    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "Moa Launcher".encode_utf16().chain(Some(0)).collect();
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
        );
    }
}

pub fn startup_command(exe: &Path, data: &Path) -> Result<String, String> {
    fn quote(path: &Path) -> Result<String, String> {
        let value = path.to_string_lossy();
        if !path.is_absolute() || value.chars().any(|c| c == '"' || c.is_control()) {
            return Err("시작 프로그램 경로가 올바르지 않습니다.".into());
        }
        let slashes = value.len() - value.trim_end_matches('\\').len();
        Ok(format!("\"{}{}\"", value, "\\".repeat(slashes)))
    }
    Ok(format!(
        "{} --background --data-dir {}",
        quote(exe)?,
        quote(data)?
    ))
}

pub fn startup_enabled(paths: &Paths) -> bool {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Ok(expected) = startup_command(&exe, &paths.data_dir) else {
        return false;
    };
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .and_then(|key| key.get_value::<String, _>("MoaLauncher"))
        .is_ok_and(|actual| actual.eq_ignore_ascii_case(&expected))
}

pub fn set_startup(paths: &Paths, enabled: bool) -> Result<(), String> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let command = startup_command(
        &std::env::current_exe().map_err(|_| "실행 경로를 확인하지 못했습니다.")?,
        &paths.data_dir,
    )?;
    if enabled && command.encode_utf16().count() > 260 {
        return Err("시작 프로그램 경로가 너무 깁니다.".into());
    }
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .map_err(|_| "시작 프로그램 설정을 열지 못했습니다.")?;
    if enabled {
        key.set_value("MoaLauncher", &command)
            .map_err(|_| "자동 실행을 등록하지 못했습니다.".into())
    } else {
        match key.delete_value("MoaLauncher") {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("자동 실행 등록을 해제하지 못했습니다.".into()),
        }
    }
}

pub fn open_folder(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("폴더를 찾을 수 없습니다.".into());
    }
    Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|_| "폴더를 열지 못했습니다.".into())
}

pub fn open_repository(repo: &str) -> Result<(), String> {
    if !crate::storage::valid_repo(repo) {
        return Err("저장소 주소가 올바르지 않습니다.".into());
    }
    // explorer's URL handler receives one validated argument; no cmd/PowerShell is involved.
    Command::new("explorer.exe")
        .arg(format!("https://github.com/{repo}/releases"))
        .spawn()
        .map(|_| ())
        .map_err(|_| "GitHub를 열지 못했습니다.".into())
}

pub struct InstanceGuard {
    mutex: isize,
    event: isize,
}
impl InstanceGuard {
    pub fn acquire(paths: &Paths, activate: bool) -> Result<Option<Self>, String> {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError},
            System::Threading::{CreateEventW, CreateMutexW, SetEvent},
        };
        let identity =
            hex::encode_upper(Sha256::digest(normalize_path(&paths.data_dir).as_bytes()))[..16]
                .to_string();
        let event_name: Vec<u16> = format!("Local\\MoaLauncher-Activate-{identity}\0")
            .encode_utf16()
            .collect();
        let mutex_name: Vec<u16> = format!("Local\\MoaLauncher-{identity}\0")
            .encode_utf16()
            .collect();
        unsafe {
            let event = CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr());
            if event.is_null() {
                return Err("런처 활성화 이벤트를 만들지 못했습니다.".into());
            }
            let mutex = CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr());
            let exists = GetLastError() == ERROR_ALREADY_EXISTS;
            if mutex.is_null() {
                CloseHandle(event);
                return Err("런처 실행 상태를 확인하지 못했습니다.".into());
            }
            if exists {
                if activate {
                    SetEvent(event);
                }
                CloseHandle(event);
                CloseHandle(mutex);
                return Ok(None);
            }
            Ok(Some(Self {
                mutex: mutex as isize,
                event: event as isize,
            }))
        }
    }
    pub fn event_handle(&self) -> isize {
        self.event
    }
}
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.mutex as _);
            windows_sys::Win32::Foundation::CloseHandle(self.mutex as _);
            windows_sys::Win32::Foundation::CloseHandle(self.event as _);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_paths_are_quoted() {
        let command = startup_command(
            Path::new("C:\\Program Files\\Moa\\MoaLauncher.exe"),
            Path::new("C:\\Users\\사용자\\Data\\"),
        )
        .unwrap();
        assert!(
            command.starts_with(
                "\"C:\\Program Files\\Moa\\MoaLauncher.exe\" --background --data-dir "
            )
        );
        assert!(command.ends_with("Data\\\\\""));
        assert!(startup_command(Path::new("relative.exe"), Path::new("C:\\Data")).is_err());
    }
}
