use crate::{
    engine::{EngineEvent, SharedEngine},
    gui::NativeApp,
    library, platform, self_update,
    storage::Paths,
    windows::{NativeEvent, Tray},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
};

pub(crate) struct Session {
    pub tray: Tray,
    pub guard: platform::InstanceGuard,
    pub engine: SharedEngine,
    pub runtime: Arc<tokio::runtime::Runtime>,
    pub native_events: Receiver<NativeEvent>,
    pub wake_context: Arc<Mutex<Option<eframe::egui::Context>>>,
    pub wake_rx: Receiver<()>,
    pub notified: Vec<String>,
    pub error: String,
}

impl Session {
    fn new(paths: Paths, background: bool) -> Result<Option<Self>, String> {
        let Some(guard) = platform::InstanceGuard::acquire(&paths, !background)? else {
            return Ok(None);
        };
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|_| "백그라운드 작업을 준비하지 못했습니다.")?,
        );
        let wake_context: Arc<Mutex<Option<eframe::egui::Context>>> = Arc::new(Mutex::new(None));
        let context = wake_context.clone();
        let (wake_tx, wake_rx) = mpsc::channel();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(context) = context.lock().unwrap().as_ref() {
                context.request_repaint();
            } else {
                let _ = wake_tx.send(());
            }
        });
        let engine = SharedEngine::new(paths, runtime.handle().clone(), wake.clone())?;
        engine.set_background(background);
        let (tray, native_events) = Tray::new(guard.event_handle(), wake)?;
        tray.set_update_enabled(false);
        engine.start();
        Ok(Some(Self {
            tray,
            guard,
            engine,
            runtime,
            native_events,
            wake_context,
            wake_rx,
            notified: vec![],
            error: String::new(),
        }))
    }

    /// Keep startup genuinely tray-only: eframe creates no window or graphics context
    /// until an explicit Open or notification click asks for the library.
    fn wait_for_window(&mut self) -> bool {
        loop {
            for event in self.engine.drain_events() {
                match event {
                    EngineEvent::Notify { ids, body } => self.tray.notify(ids, &body),
                    EngineEvent::ForceCloseRequested => return true,
                    EngineEvent::ExitRequested => return false,
                    EngineEvent::Error(error) => self.error = error,
                    _ => {}
                }
            }
            for event in self.native_events.try_iter() {
                match event {
                    NativeEvent::Open => return true,
                    NativeEvent::ShowUpdates(ids) => {
                        self.notified = ids;
                        return true;
                    }
                    NativeEvent::UpdateAll => {
                        if let Err(error) = self.engine.update_all() {
                            self.error = error;
                        }
                    }
                    NativeEvent::Quit => match self.engine.request_exit() {
                        Ok(()) => return false,
                        Err(error) => {
                            self.error = error;
                            return true;
                        }
                    },
                }
            }
            let snapshot = self.engine.snapshot();
            if snapshot.force_close_prompt.is_some() {
                return true;
            }
            let busy = snapshot.checking
                || snapshot.batch_updating
                || snapshot.working
                || snapshot.apps.iter().any(|app| app.busy);
            self.tray
                .set_update_enabled(!busy && library::update_count(&snapshot.apps) > 0);
            if self.wake_rx.recv().is_err() {
                return false;
            }
        }
    }
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--apply-self-update") {
        match args
            .get(index + 1)
            .ok_or_else(|| "업데이트 계획이 없습니다.".to_owned())
            .and_then(|plan| self_update::run_helper(Path::new(plan)))
        {
            Ok(()) => {}
            Err(error) => {
                eprintln!("자체 업데이트 helper: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let data = match args.iter().position(|arg| arg == "--data-dir") {
        Some(index) => match args.get(index + 1).filter(|arg| !arg.starts_with("--")) {
            Some(path) => Some(PathBuf::from(path)),
            None => {
                platform::show_error("--data-dir 다음에 데이터 폴더를 지정하세요.");
                return;
            }
        },
        None => None,
    };
    let background = args.iter().any(|arg| arg == "--background");
    let mut session = match Session::new(Paths::new(data), background) {
        Ok(Some(session)) => session,
        Ok(None) => return,
        Err(error) => {
            platform::show_error(&error);
            return;
        }
    };
    if background && !session.wait_for_window() {
        return;
    }
    session.engine.set_background(false);
    let image = image::load_from_memory(include_bytes!("../icons/icon.png"))
        .expect("bundled icon")
        .into_rgba8();
    let icon = eframe::egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Moa Launcher")
            .with_inner_size([1200.0, 840.0])
            .with_min_inner_size([820.0, 600.0])
            .with_icon(Arc::new(icon)),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "Moa Launcher",
        options,
        Box::new(move |cc| {
            NativeApp::new(cc, session)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(|message| {
                    Box::new(std::io::Error::other(message))
                        as Box<dyn std::error::Error + Send + Sync>
                })
        }),
    ) {
        platform::show_error(&format!("런처를 시작하지 못했습니다: {error}"));
    }
}
