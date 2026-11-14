//! Windows notification-area integration, isolated from the UI message loop.

use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicIsize, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, POINT, WAIT_FAILED, WAIT_OBJECT_0, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Shell::{
            Shell_NotifyIconW, NOTIFYICONDATAW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP,
            NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
            DispatchMessageW, GetCursorPos, GetWindowLongPtrW, LoadImageW,
            PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
            MsgWaitForMultipleObjects, SetForegroundWindow, SetWindowLongPtrW, TrackPopupMenu,
            TranslateMessage, WNDCLASSW,
            IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, MF_SEPARATOR, MF_STRING, MF_GRAYED,
            PM_REMOVE, TPM_RETURNCMD, WM_APP, WM_DESTROY, WM_NCCREATE, WM_NULL, WM_RBUTTONUP,
            GWLP_USERDATA, HWND_MESSAGE, MSG, QS_ALLINPUT,
        },
    },
};

const TRAY_CALLBACK: u32 = WM_APP + 41;
const COMMAND_MESSAGE: u32 = WM_APP + 42;
const TRAY_ID: u32 = 1;
const MENU_OPEN: usize = 1001;
const MENU_UPDATE_ALL: usize = 1002;
const MENU_QUIT: usize = 1003;

const WINDOW_CLASS: [u16; 19] = [
    'M' as u16, 'o' as u16, 'a' as u16, 'L' as u16, 'a' as u16, 'u' as u16, 'n' as u16,
    'c' as u16, 'h' as u16, 'e' as u16, 'r' as u16, 'T' as u16, 'r' as u16, 'a' as u16,
    'y' as u16, 'W' as u16, 'i' as u16, 'n' as u16, 0,
];

#[derive(Debug)]
pub enum NativeEvent {
    Open,
    ShowUpdates(Vec<String>),
    UpdateAll,
    Quit,
}

enum TrayCommand {
    SetUpdateEnabled(bool),
    Notify(Vec<String>, String),
    Stop,
}

/// Owns the tray worker. Dropping it removes the notification icon and joins the worker.
pub struct Tray {
    commands: Sender<TrayCommand>,
    hwnd: Arc<AtomicIsize>,
    worker: Option<JoinHandle<()>>,
}

impl Tray {
    pub fn new(
        event_handle: isize,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(Self, Receiver<NativeEvent>), String> {
        let (events_tx, events_rx) = mpsc::channel();
        let (commands_tx, commands_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let hwnd = Arc::new(AtomicIsize::new(0));
        let worker_hwnd = hwnd.clone();
        let worker = thread::Builder::new()
            .name("moa-tray".into())
            .spawn(move || {
                worker_main(
                    event_handle,
                    events_tx,
                    commands_rx,
                    ready_tx,
                    worker_hwnd,
                    wake,
                )
            })
            .map_err(|_| "시스템 트레이 스레드를 시작하지 못했습니다.".to_string())?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok((
                Self {
                    commands: commands_tx,
                    hwnd,
                    worker: Some(worker),
                },
                events_rx,
            )),
            Ok(Err(message)) => {
                let _ = worker.join();
                Err(message)
            }
            Err(_) => {
                let _ = worker.join();
                Err("시스템 트레이를 초기화하지 못했습니다.".into())
            }
        }
    }

    pub fn set_update_enabled(&self, enabled: bool) {
        self.send(TrayCommand::SetUpdateEnabled(enabled));
    }

    pub fn notify(&self, ids: Vec<String>, body: &str) {
        self.send(TrayCommand::Notify(ids, body.to_owned()));
    }

    fn send(&self, command: TrayCommand) {
        if self.commands.send(command).is_ok() {
            let hwnd = self.hwnd.load(Ordering::Acquire) as HWND;
            if !hwnd.is_null() {
                unsafe { PostMessageW(hwnd, COMMAND_MESSAGE, 0, 0) };
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let _ = self.commands.send(TrayCommand::Stop);
        let hwnd = self.hwnd.load(Ordering::Acquire) as HWND;
        if !hwnd.is_null() {
            unsafe { PostMessageW(hwnd, COMMAND_MESSAGE, 0, 0) };
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct WindowState {
    events: Sender<NativeEvent>,
    commands: Receiver<TrayCommand>,
    wake: Arc<dyn Fn() + Send + Sync>,
    hwnd: HWND,
    icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON,
    update_enabled: bool,
    announced_ids: Vec<String>,
    taskbar_created: u32,
    stopping: bool,
}

fn worker_main(
    event_handle: isize,
    events: Sender<NativeEvent>,
    commands: Receiver<TrayCommand>,
    ready: SyncSender<Result<(), String>>,
    shared_hwnd: Arc<AtomicIsize>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let class_name = WINDOW_CLASS.as_ptr();
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        let _ = ready.send(Err("런처 모듈을 찾지 못했습니다.".into()));
        return;
    }

    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name,
    };
    let registered = unsafe { RegisterClassW(&class) };
    if registered == 0 && unsafe { GetLastError() } != 1410 /* class already exists */ {
        let _ = ready.send(Err("트레이 창 클래스를 등록하지 못했습니다.".into()));
        return;
    }

    let mut state = Box::new(WindowState {
        events,
        commands,
        wake,
        hwnd: null_mut(),
        icon: null_mut(),
        update_enabled: true,
        announced_ids: Vec::new(),
        taskbar_created: unsafe { RegisterWindowMessageW(windows_wide("TaskbarCreated").as_ptr()) },
        stopping: false,
    });
    let state_ptr = (&mut *state) as *mut WindowState;
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name,
            windows_wide("모아 런처 트레이").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            state_ptr.cast(),
        )
    };
    if hwnd.is_null() {
        let _ = ready.send(Err("트레이 메시지 창을 만들지 못했습니다.".into()));
        return;
    }
    state.hwnd = hwnd;
    shared_hwnd.store(hwnd as isize, Ordering::Release);

    let icon = unsafe {
        LoadImageW(
            instance,
            1usize as *const u16,
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        )
    } as _;
    if icon.is_null() {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd) };
        let _ = ready.send(Err("런처 트레이 아이콘 리소스를 불러오지 못했습니다.".into()));
        return;
    }
    state.icon = icon;
    if !add_tray_icon(&state) {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd) };
        let _ = ready.send(Err("시스템 트레이 아이콘을 등록하지 못했습니다.".into()));
        return;
    }
    let _ = ready.send(Ok(()));

    let mut message: MSG = unsafe { zeroed() };
    let handle = event_handle as windows_sys::Win32::Foundation::HANDLE;
    loop {
        let result = unsafe {
            MsgWaitForMultipleObjects(
                1,
                &handle,
                0,
                u32::MAX,
                QS_ALLINPUT,
            )
        };
        if result == WAIT_OBJECT_0 {
            emit(&state, NativeEvent::Open);
        } else if result == WAIT_OBJECT_0 + 1 {
            while unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
                if message.message == windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    state.stopping = true;
                    break;
                }
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        } else if result == WAIT_FAILED {
            break;
        } else {
            // Unexpected wait result: don't spin forever on a broken native handle.
            break;
        }
        if state.stopping {
            break;
        }
    }
    remove_tray_icon(&state);
    shared_hwnd.store(0, Ordering::Release);
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd) };
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam as *const windows_sys::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let state = &mut *state_ptr;

    if message == TRAY_CALLBACK {
        match lparam as u32 {
            windows_sys::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP
            | windows_sys::Win32::UI::Shell::NIN_SELECT => emit(state, NativeEvent::Open),
            WM_RBUTTONUP | windows_sys::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU => {
                show_menu(state)
            }
            windows_sys::Win32::UI::Shell::NIN_BALLOONUSERCLICK => {
                if !state.announced_ids.is_empty() {
                    emit(state, NativeEvent::ShowUpdates(state.announced_ids.clone()));
                }
            }
            _ => {}
        }
        return 0;
    }
    if message == COMMAND_MESSAGE {
        process_commands(state);
        return 0;
    }
    if message == state.taskbar_created && state.taskbar_created != 0 {
        add_tray_icon(state);
        return 0;
    }
    if message == WM_DESTROY {
        remove_tray_icon(state);
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn process_commands(state: &mut WindowState) {
    loop {
        match state.commands.try_recv() {
            Ok(TrayCommand::SetUpdateEnabled(enabled)) => {
                state.update_enabled = enabled;
                if !enabled {
                    state.announced_ids.clear();
                }
            }
            Ok(TrayCommand::Notify(ids, body)) => {
                if state.update_enabled && !ids.is_empty() {
                    state.announced_ids = ids;
                    let mut data = notify_data(state);
                    data.uFlags = NIF_INFO;
                    data.szInfoTitle = fixed_wide("모아 런처 업데이트");
                    data.szInfo = fixed_wide(&body);
                    data.dwInfoFlags = NIIF_INFO;
                    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
                }
            }
            Ok(TrayCommand::Stop) => {
                state.stopping = true;
                unsafe { PostQuitMessage(0) };
                break;
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
}

fn emit(state: &WindowState, event: NativeEvent) {
    if state.events.send(event).is_ok() {
        (state.wake)();
    }
}

fn add_tray_icon(state: &WindowState) -> bool {
    let mut data = notify_data(state);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = TRAY_CALLBACK;
    data.hIcon = state.icon;
    data.szTip = fixed_wide("모아 런처");
    unsafe { Shell_NotifyIconW(NIM_ADD, &data) != 0 }
}

fn remove_tray_icon(state: &WindowState) {
    let data = notify_data(state);
    unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
}

fn notify_data(state: &WindowState) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = state.hwnd;
    data.uID = TRAY_ID;
    data
}

fn show_menu(state: &WindowState) {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return;
    }
    let open = windows_wide("모아 런처 열기");
    let update = windows_wide("모두 업데이트");
    let quit = windows_wide("종료");
    unsafe {
        AppendMenuW(menu, MF_STRING, MENU_OPEN, open.as_ptr());
        AppendMenuW(
            menu,
            MF_STRING | if state.update_enabled { 0 } else { MF_GRAYED },
            MENU_UPDATE_ALL,
            update.as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, MENU_QUIT, quit.as_ptr());
    }
    let mut point: POINT = unsafe { zeroed() };
    unsafe {
        GetCursorPos(&mut point);
        SetForegroundWindow(state.hwnd);
    }
    let selected = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD,
            point.x,
            point.y,
            0,
            state.hwnd,
            null(),
        )
    };
    unsafe {
        PostMessageW(state.hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
    match selected {
        MENU_OPEN => emit(state, NativeEvent::Open),
        MENU_UPDATE_ALL if state.update_enabled => emit(state, NativeEvent::UpdateAll),
        MENU_QUIT => emit(state, NativeEvent::Quit),
        _ => {}
    }
}

fn fixed_wide<const N: usize>(value: &str) -> [u16; N] {
    let mut output = [0; N];
    for (target, source) in output.iter_mut().take(N.saturating_sub(1)).zip(value.encode_utf16()) {
        if source == 0 {
            break;
        }
        *target = source;
    }
    output
}

fn windows_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
