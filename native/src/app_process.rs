//! Carefully scoped process handling for applications being updated.

use std::path::{Path, PathBuf};

#[cfg(windows)]
mod imp {
    use super::*;
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use sysinfo::{ProcessesToUpdate, System};
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, GetLastError, HANDLE,
            WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Storage::FileSystem::SYNCHRONIZE,
        System::Threading::{
            GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
            QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
        },
    };

    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

    struct ProcessHandle {
        pid: u32,
        raw: isize,
        can_terminate: bool,
    }

    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            if self.raw != 0 {
                unsafe { CloseHandle(self.raw as HANDLE) };
                self.raw = 0;
            }
        }
    }

    pub(crate) struct AppProcesses {
        processes: Vec<ProcessHandle>,
    }

    fn normalized(path: &Path) -> Result<PathBuf, String> {
        let path = dunce::canonicalize(path)
            .map_err(|e| format!("실행 파일 경로를 정규화하지 못했습니다: {e}"))?;
        let text = path.to_string_lossy().replace('/', "\\");
        Ok(PathBuf::from(text.to_lowercase()))
    }

    fn query_image_path(handle: HANDLE) -> Result<PathBuf, String> {
        let mut capacity = 512usize;
        loop {
            let mut buffer = vec![0u16; capacity];
            let mut length = capacity as u32;
            let ok =
                unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) };
            if ok != 0 {
                buffer.truncate(length as usize);
                return Ok(PathBuf::from(OsString::from_wide(&buffer)));
            }
            let error = unsafe { GetLastError() };
            if error == ERROR_INSUFFICIENT_BUFFER && capacity < 32768 {
                capacity *= 2;
                continue;
            }
            return Err(format!(
                "프로세스 실행 경로를 확인하지 못했습니다 (오류 {error})."
            ));
        }
    }

    fn open_matching(pid: u32, target: &Path) -> Result<Option<ProcessHandle>, String> {
        let desired = PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE | PROCESS_TERMINATE;
        let mut raw = unsafe { OpenProcess(desired, 0, pid) };
        let mut can_terminate = true;
        if raw.is_null() {
            let error = unsafe { GetLastError() };
            if error == ERROR_INVALID_PARAMETER {
                return Ok(None); // The process exited between enumeration and OpenProcess.
            }
            if error != ERROR_ACCESS_DENIED {
                return Err(format!(
                    "PID {pid} 프로세스를 열지 못했습니다 (오류 {error})."
                ));
            }
            raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
            if raw.is_null() {
                let fallback_error = unsafe { GetLastError() };
                if fallback_error == ERROR_INVALID_PARAMETER {
                    return Ok(None);
                }
                return Err(format!(
                    "PID {pid} 프로세스를 확인할 권한이 없습니다 (오류 {fallback_error})."
                ));
            }
            can_terminate = false;
        }

        let actual = match query_image_path(raw) {
            Ok(path) => path,
            Err(error) => {
                unsafe { CloseHandle(raw) };
                return Err(format!("PID {pid}: {error}"));
            }
        };
        let actual = match normalized(&actual) {
            Ok(path) => path,
            Err(error) => {
                unsafe { CloseHandle(raw) };
                return Err(format!("PID {pid}: {error}"));
            }
        };
        if actual != target {
            unsafe { CloseHandle(raw) };
            return Ok(None);
        }
        Ok(Some(ProcessHandle {
            pid,
            raw: raw as isize,
            can_terminate,
        }))
    }

    pub(crate) fn capture(executable: &Path) -> Result<AppProcesses, String> {
        let target = normalized(executable)?;
        let current_id = unsafe { GetCurrentProcessId() };
        let current_exe = std::env::current_exe()
            .ok()
            .and_then(|p| normalized(&p).ok());
        if current_id != 0 && current_exe.as_ref() == Some(&target) {
            return Err(
                "현재 실행 중인 모아 런처는 업데이트 대상 프로세스로 지정할 수 없습니다.".into(),
            );
        }

        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let candidates: Vec<u32> = system
            .processes()
            .iter()
            .filter_map(|(pid, process)| {
                process
                    .exe()
                    .and_then(|path| normalized(path).ok())
                    .filter(|path| path == &target)
                    .map(|_| pid.as_u32())
            })
            .collect();

        let mut processes = Vec::new();
        for pid in candidates {
            if pid == current_id {
                return Err("현재 런처 프로세스를 업데이트 대상으로 지정할 수 없습니다.".into());
            }
            if let Some(process) = open_matching(pid, &target)? {
                processes.push(process);
            }
        }
        processes.sort_by_key(|process| process.pid);
        Ok(AppProcesses { processes })
    }

    impl AppProcesses {
        pub(crate) fn is_empty(&self) -> bool {
            self.processes.is_empty()
        }

        pub(crate) fn pids(&self) -> Vec<u32> {
            self.processes.iter().map(|process| process.pid).collect()
        }

        pub(crate) fn all_exited(&self) -> Result<bool, String> {
            for process in &self.processes {
                let result = unsafe { WaitForSingleObject(process.raw as HANDLE, 0) };
                match result {
                    WAIT_OBJECT_0 => {}
                    WAIT_TIMEOUT => return Ok(false),
                    WAIT_FAILED => {
                        let error = unsafe { GetLastError() };
                        return Err(format!(
                            "PID {} 종료 상태를 확인하지 못했습니다 (오류 {error}).",
                            process.pid
                        ));
                    }
                    other => {
                        return Err(format!(
                            "PID {} 대기 결과가 예상과 다릅니다 ({other}).",
                            process.pid
                        ));
                    }
                }
            }
            Ok(true)
        }

        pub(crate) fn request_close(&self) -> Result<(), String> {
            struct EnumContext {
                processes: std::collections::HashMap<u32, isize>,
                posted: usize,
                failed: bool,
            }
            unsafe extern "system" fn visit(
                hwnd: windows_sys::Win32::Foundation::HWND,
                lparam: isize,
            ) -> i32 {
                let context = unsafe { &mut *(lparam as *mut EnumContext) };
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
                let still_same_process = if let Some(handle) = context.processes.get(&pid) {
                    // A signaled process handle proves this PID has exited and may have been reused.
                    match unsafe { WaitForSingleObject(*handle as HANDLE, 0) } {
                        WAIT_TIMEOUT => true,
                        WAIT_OBJECT_0 => false,
                        _ => {
                            context.failed = true;
                            false
                        }
                    }
                } else {
                    false
                };
                if still_same_process && unsafe { GetWindow(hwnd, GW_OWNER) }.is_null() {
                    if unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } != 0 {
                        context.posted += 1;
                    } else {
                        context.failed = true;
                    }
                }
                1
            }
            let mut context = EnumContext {
                processes: self.processes.iter().map(|p| (p.pid, p.raw)).collect(),
                posted: 0,
                failed: false,
            };
            if context.processes.is_empty() {
                return Ok(());
            }
            let ok = unsafe { EnumWindows(Some(visit), &mut context as *mut _ as isize) };
            if ok == 0 {
                let error = unsafe { GetLastError() };
                return Err(format!("앱 창을 검색하지 못했습니다 (오류 {error})."));
            }
            if context.failed {
                return Err("앱 창에 종료 요청을 전달하지 못했습니다.".into());
            }
            Ok(())
        }

        pub(crate) fn force_close(&self) -> Result<(), String> {
            let mut errors = Vec::new();
            for process in &self.processes {
                if !process.can_terminate {
                    errors.push(format!("PID {} 강제 종료 권한이 없습니다.", process.pid));
                    continue;
                }
                let wait = unsafe { WaitForSingleObject(process.raw as HANDLE, 0) };
                if wait == WAIT_OBJECT_0 {
                    continue;
                }
                if wait != WAIT_TIMEOUT {
                    let error = unsafe { GetLastError() };
                    errors.push(format!(
                        "PID {} 상태 확인 실패 (오류 {error}).",
                        process.pid
                    ));
                    continue;
                }
                if unsafe { TerminateProcess(process.raw as HANDLE, 1) } == 0 {
                    let error = unsafe { GetLastError() };
                    errors.push(format!(
                        "PID {} 강제 종료에 실패했습니다 (오류 {error}).",
                        process.pid
                    ));
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join(" "))
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{
            process::{Child, Command},
            time::{Duration, Instant, SystemTime, UNIX_EPOCH},
        };

        struct TestWorkspace(PathBuf);

        impl TestWorkspace {
            fn new() -> Self {
                let root = std::env::temp_dir().join(format!(
                    "moa-process-test-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&root).unwrap();
                let source = std::env::var_os("MOA_PROCESS_FIXTURE_EXE")
                    .expect("격리된 fixture 경로 MOA_PROCESS_FIXTURE_EXE가 필요합니다");
                assert!(Path::new(&source).is_file());
                Self(root)
            }

            fn copy(&self, folder: &str) -> PathBuf {
                let directory = self.0.join(folder);
                std::fs::create_dir_all(&directory).unwrap();
                let destination = directory.join("process_fixture.exe");
                std::fs::copy(
                    std::env::var_os("MOA_PROCESS_FIXTURE_EXE").unwrap(),
                    &destination,
                )
                .unwrap();
                destination
            }

            fn start(&self, exe: &Path, mode: &str, label: &str) -> OwnedChild {
                let ready = self.0.join(format!("{label}.ready"));
                let child = Command::new(exe)
                    .args([mode, ready.to_str().unwrap()])
                    .spawn()
                    .unwrap();
                let mut owned = OwnedChild(Some(child));
                let deadline = Instant::now() + Duration::from_secs(10);
                while Instant::now() < deadline {
                    if ready.is_file() {
                        return owned;
                    }
                    assert!(
                        owned.0.as_mut().unwrap().try_wait().unwrap().is_none(),
                        "fixture exited before ready"
                    );
                    std::thread::sleep(Duration::from_millis(25));
                }
                panic!("fixture did not create its ready marker in time");
            }
        }

        impl Drop for TestWorkspace {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        struct OwnedChild(Option<Child>);

        impl OwnedChild {
            fn id(&self) -> u32 {
                self.0.as_ref().unwrap().id()
            }
            fn is_alive(&mut self) -> bool {
                self.0.as_mut().unwrap().try_wait().unwrap().is_none()
            }
            fn wait(&mut self) {
                if let Some(mut child) = self.0.take() {
                    let _ = child.wait();
                }
            }
        }

        impl Drop for OwnedChild {
            fn drop(&mut self) {
                if let Some(mut child) = self.0.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }

        #[test]
        fn normalized_paths_are_case_insensitive_and_separator_insensitive() {
            let base = std::env::temp_dir().join(format!("moa-path-{}", std::process::id()));
            std::fs::create_dir_all(&base).unwrap();
            let file = base.join("Target.exe");
            std::fs::write(&file, b"fixture").unwrap();
            assert_eq!(
                normalized(&file).unwrap(),
                normalized(&base.join("target.EXE")).unwrap()
            );
            let _ = std::fs::remove_file(file);
            let _ = std::fs::remove_dir(base);
        }

        // End-to-end process tests are opt-in and use only an explicitly supplied fixture binary.
        #[test]
        #[ignore = "격리된 fixture 실행 파일을 준비한 뒤 MOA_PROCESS_FIXTURE_EXE로 지정하세요"]
        fn captures_multiple_processes_of_the_same_exact_executable() {
            let workspace = TestWorkspace::new();
            let exe = workspace.copy("same");
            let first = workspace.start(&exe, "no-window", "first");
            let second = workspace.start(&exe, "no-window", "second");
            let app = capture(&exe).unwrap();
            assert!(app.pids().contains(&first.id()));
            assert!(app.pids().contains(&second.id()));
            drop(app);
        }

        #[test]
        #[ignore = "격리된 fixture 실행 파일을 준비한 뒤 MOA_PROCESS_FIXTURE_EXE로 지정하세요"]
        fn same_basename_in_another_folder_is_not_captured_or_closed() {
            let workspace = TestWorkspace::new();
            let first_exe = workspace.copy("first");
            let second_exe = workspace.copy("second");
            let mut first = workspace.start(&first_exe, "no-window", "first");
            let mut second = workspace.start(&second_exe, "no-window", "second");
            let app = capture(&first_exe).unwrap();
            assert!(app.pids().contains(&first.id()));
            assert!(!app.pids().contains(&second.id()));
            app.force_close().unwrap();
            for _ in 0..100 {
                if app.all_exited().unwrap() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(app.all_exited().unwrap());
            assert!(
                second.is_alive(),
                "같은 이름의 다른 경로 프로세스가 영향을 받았습니다"
            );
            first.wait();
        }

        #[test]
        #[ignore = "격리된 fixture 실행 파일을 준비한 뒤 MOA_PROCESS_FIXTURE_EXE로 지정하세요"]
        fn wm_close_exits_a_cooperative_hidden_window() {
            let workspace = TestWorkspace::new();
            let exe = workspace.copy("close");
            let mut child = workspace.start(&exe, "close", "close");
            let app = capture(&exe).unwrap();
            assert!(app.pids().contains(&child.id()));
            app.request_close().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && !app.all_exited().unwrap() {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(app.all_exited().unwrap());
            child.wait();
        }

        #[test]
        #[ignore = "격리된 fixture 실행 파일을 준비한 뒤 MOA_PROCESS_FIXTURE_EXE로 지정하세요"]
        fn ignored_wm_close_stays_alive_until_force_terminated() {
            let workspace = TestWorkspace::new();
            let exe = workspace.copy("ignore");
            let mut child = workspace.start(&exe, "ignore-close", "ignore");
            let app = capture(&exe).unwrap();
            app.request_close().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            assert!(!app.all_exited().unwrap());
            assert!(child.is_alive());
            app.force_close().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && !app.all_exited().unwrap() {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(app.all_exited().unwrap());
            child.wait();
        }

        #[test]
        #[ignore = "격리된 fixture 실행 파일을 준비한 뒤 MOA_PROCESS_FIXTURE_EXE로 지정하세요"]
        fn process_without_window_can_be_force_terminated() {
            let workspace = TestWorkspace::new();
            let exe = workspace.copy("no-window");
            let mut child = workspace.start(&exe, "no-window", "no-window");
            let app = capture(&exe).unwrap();
            app.request_close().unwrap();
            assert!(!app.all_exited().unwrap());
            assert!(child.is_alive());
            app.force_close().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && !app.all_exited().unwrap() {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(app.all_exited().unwrap());
            child.wait();
        }

        #[test]
        #[ignore = "격리된 fixture 실행 파일을 준비한 뒤 MOA_PROCESS_FIXTURE_EXE로 지정하세요"]
        fn current_process_executable_is_rejected() {
            let current_exe = std::env::current_exe().unwrap();
            let error = match capture(&current_exe) {
                Ok(_) => panic!("현재 실행 파일은 거부되어야 합니다"),
                Err(error) => error,
            };
            assert!(error.contains("현재"));
        }
    }
}

#[cfg(windows)]
pub(crate) use imp::capture;

#[cfg(not(windows))]
pub(crate) struct AppProcesses;

#[cfg(not(windows))]
pub(crate) fn capture(_executable: &Path) -> Result<AppProcesses, String> {
    Err("앱 프로세스 제어는 Windows에서만 지원합니다.".into())
}

#[cfg(not(windows))]
impl AppProcesses {
    pub(crate) fn is_empty(&self) -> bool {
        true
    }
    pub(crate) fn pids(&self) -> Vec<u32> {
        Vec::new()
    }
    pub(crate) fn all_exited(&self) -> Result<bool, String> {
        Ok(true)
    }
    pub(crate) fn request_close(&self) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn force_close(&self) -> Result<(), String> {
        Ok(())
    }
}
