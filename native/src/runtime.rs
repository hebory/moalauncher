use crate::{gui::NativeApp, platform, self_update, storage::Paths};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

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
    let paths = Paths::new(data);
    let guard = match platform::InstanceGuard::acquire(&paths, !background) {
        Ok(Some(guard)) => guard,
        Ok(None) => return,
        Err(error) => {
            platform::show_error(&error);
            return;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => Arc::new(runtime),
        Err(_) => {
            platform::show_error("백그라운드 작업을 준비하지 못했습니다.");
            return;
        }
    };
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
            .with_inner_size([1000.0, 760.0])
            .with_min_inner_size([760.0, 560.0])
            .with_icon(Arc::new(icon))
            .with_visible(!background),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "Moa Launcher",
        options,
        Box::new(move |cc| {
            NativeApp::new(cc, paths, runtime, guard, background)
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
