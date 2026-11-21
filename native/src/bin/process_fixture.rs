//! Minimal opt-in fixture used only by isolated process-control tests.

#[cfg(windows)]
fn main() {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, path::PathBuf};
    use windows_sys::Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
            DispatchMessageW, GetMessageW, MSG, PostQuitMessage, RegisterClassW, TranslateMessage,
            WM_CLOSE, WM_DESTROY, WNDCLASSW, WS_OVERLAPPEDWINDOW,
        },
    };

    const MODE_CLOSE: u8 = 0;
    const MODE_IGNORE: u8 = 1;
    const MODE_NO_WINDOW: u8 = 2;
    static mut MODE: u8 = MODE_CLOSE;

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CLOSE if unsafe { MODE == MODE_IGNORE } => 0,
            WM_CLOSE => {
                unsafe { DestroyWindow(hwnd) };
                0
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    let mut args = std::env::args_os().skip(1);
    let mode_arg = args.next().unwrap_or_default();
    let ready_path = args.next().map(PathBuf::from);
    let mode = match mode_arg.to_string_lossy().as_ref() {
        "close" => MODE_CLOSE,
        "ignore-close" => MODE_IGNORE,
        "no-window" => MODE_NO_WINDOW,
        _ => std::process::exit(2),
    };
    unsafe { MODE = mode };
    if mode == MODE_NO_WINDOW {
        if let Some(path) = ready_path
            && std::fs::write(path, b"ready").is_err()
        {
            std::process::exit(3);
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }

    let class: Vec<u16> = std::ffi::OsStr::new("MoaProcessFixtureHiddenWindow")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) } as HINSTANCE;
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        std::process::exit(4);
    }
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null::<c_void>(),
        )
    };
    if hwnd.is_null() {
        std::process::exit(5);
    }
    // Intentionally never show the top-level window; EnumWindows still finds it.
    if let Some(path) = ready_path
        && std::fs::write(path, b"ready").is_err()
    {
        std::process::exit(3);
    }
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This fixture is intended for Windows-only isolated tests.");
    std::process::exit(2);
}
