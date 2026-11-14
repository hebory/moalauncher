use crate::{
    artwork, credentials, github, install,
    models::*,
    platform, self_update,
    storage::{self, Paths},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SELF_ID: &str = "moa-launcher";
type Shared = Arc<Engine>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppView {
    definition: AppDefinition,
    installed: Option<InstalledApp>,
    release: Option<AppRelease>,
    is_cached: bool,
    is_launcher: bool,
    busy: bool,
    can_cancel: bool,
    error: Option<String>,
    progress: Option<InstallProgress>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    apps: Vec<AppView>,
    settings: Settings,
    github_connected: bool,
    startup_enabled: bool,
    data_directory: String,
    version: String,
    checking: bool,
    batch_updating: bool,
    status: String,
    last_checked: Option<String>,
    notification_ids: Vec<String>,
    auth_generation: u64,
    update_result: Option<UpdateResult>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateResult {
    success: bool,
    message: String,
    #[serde(default)]
    log_path: String,
    version: String,
    completed_at: String,
}
struct Data {
    apps: Vec<AppView>,
    settings: Settings,
    checking: bool,
    batch_updating: bool,
    status: String,
    last_checked: Option<String>,
    notification_ids: Vec<String>,
    announced: HashSet<String>,
    announced_ids: HashSet<String>,
    auth_generation: u64,
    next_check: Instant,
    update_result: Option<UpdateResult>,
}
struct Engine {
    paths: Paths,
    client: reqwest::Client,
    data: Mutex<Data>,
    activity: tokio::sync::Mutex<()>,
    cancellations: Mutex<HashMap<String, Arc<AtomicBool>>>,
    batch_cancel: AtomicBool,
    exiting: AtomicBool,
    tray_update: Mutex<Option<MenuItem<tauri::Wry>>>,
}

fn newer(latest: &str, current: &str) -> bool {
    match (
        semver::Version::parse(latest.trim_start_matches(['v', 'V'])),
        semver::Version::parse(current.trim_start_matches(['v', 'V'])),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}
fn has_update(app: &AppView) -> bool {
    app.release
        .as_ref()
        .zip(app.installed.as_ref())
        .is_some_and(|(r, i)| newer(&r.tag, &i.version))
}
fn launcher_definition() -> AppDefinition {
    AppDefinition {
        id: SELF_ID.into(),
        name: "Moa Launcher".into(),
        description: "모아 프로그램 업데이트 런처".into(),
        category: "런처".into(),
        accent_color: "#6964E8".into(),
        monogram: "M".into(),
        repository: "hebory/moalauncher".into(),
        is_private: false,
        icon_url: None,
        cover_url: None,
        packages: vec![PackageDefinition {
            architecture: "x64".into(),
            kind: "inno".into(),
            asset_pattern: "MoaLauncher-*-Setup-x64.exe".into(),
            executable_path: "MoaLauncher.exe".into(),
            install_directory: Some("Programs/MoaLauncher".into()),
        }],
    }
}
impl Engine {
    fn new(paths: Paths) -> Result<Shared, String> {
        let settings = storage::load_settings(&paths)?;
        let interval = Duration::from_secs(u64::from(settings.check_interval_minutes) * 60);
        let result_path = paths.data_dir.join("self-update-result.json");
        let update_result = std::fs::metadata(&result_path)
            .ok()
            .filter(|m| m.len() <= 65536)
            .and_then(|_| std::fs::read(&result_path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let engine = Arc::new(Self {
            paths,
            client: github::client()?,
            data: Mutex::new(Data {
                apps: vec![],
                settings,
                checking: false,
                batch_updating: false,
                status: "도구를 불러왔습니다.".into(),
                last_checked: None,
                notification_ids: vec![],
                announced: HashSet::new(),
                announced_ids: HashSet::new(),
                auth_generation: 0,
                next_check: Instant::now() + interval,
                update_result,
            }),
            activity: tokio::sync::Mutex::new(()),
            cancellations: Mutex::new(HashMap::new()),
            batch_cancel: AtomicBool::new(false),
            exiting: AtomicBool::new(false),
            tray_update: Mutex::new(None),
        });
        engine.rebuild()?;
        Ok(engine)
    }
    fn rebuild(&self) -> Result<(), String> {
        let mut definitions = storage::load_catalog(&self.paths)?.apps;
        definitions.retain(|a| a.id != SELF_ID);
        definitions.insert(0, launcher_definition());
        let mut data = self.data.lock().unwrap();
        let previous: HashMap<_, _> = data
            .apps
            .drain(..)
            .map(|a| (a.definition.id.clone(), a))
            .collect();
        for definition in definitions {
            let is_launcher = definition.id == SELF_ID;
            let (installed, error) = if is_launcher {
                let exe =
                    std::env::current_exe().map_err(|_| "실행 경로를 확인하지 못했습니다.")?;
                (
                    Some(InstalledApp {
                        app_id: SELF_ID.into(),
                        version: VERSION.into(),
                        executable_path: exe.to_string_lossy().into(),
                        install_directory: exe.parent().unwrap().to_string_lossy().into(),
                        package_kind: "launcher".into(),
                        installed_at: chrono::Utc::now().to_rfc3339(),
                    }),
                    None,
                )
            } else {
                match install::find_installed(&self.paths, &definition) {
                    Ok(i) => (i, None),
                    Err(e) => (None, Some(e)),
                }
            };
            let old = previous.get(&definition.id).filter(|old| {
                old.definition.repository == definition.repository
                    && old.definition.is_private == definition.is_private
            });
            let release = old.and_then(|a| a.release.clone());
            let is_cached = old.is_some_and(|a| a.is_cached);
            data.apps.push(AppView {
                definition,
                installed,
                release,
                is_cached,
                is_launcher,
                busy: false,
                can_cancel: false,
                error,
                progress: None,
            });
        }
        data.auth_generation += 1;
        Ok(())
    }
    fn snapshot(&self) -> Snapshot {
        let data = self.data.lock().unwrap();
        Snapshot {
            apps: data.apps.clone(),
            settings: data.settings.clone(),
            github_connected: credentials::read_token(&self.paths)
                .ok()
                .flatten()
                .is_some(),
            startup_enabled: platform::startup_enabled(&self.paths),
            data_directory: self.paths.data_dir.to_string_lossy().into(),
            version: VERSION.into(),
            checking: data.checking,
            batch_updating: data.batch_updating,
            status: data.status.clone(),
            last_checked: data.last_checked.clone(),
            notification_ids: data.notification_ids.clone(),
            auth_generation: data.auth_generation,
            update_result: data.update_result.clone(),
        }
    }
    fn emit(&self, app: &tauri::AppHandle) {
        let snapshot = self.snapshot();
        let updates = snapshot
            .apps
            .iter()
            .any(|a| has_update(a) && a.error.is_none());
        if let Some(item) = self.tray_update.lock().unwrap().as_ref() {
            let _ = item.set_enabled(
                updates
                    && !snapshot.checking
                    && !snapshot.batch_updating
                    && !snapshot.apps.iter().any(|a| a.busy),
            );
        }
        let _ = app.emit("launcher-state", snapshot);
    }
    fn view(&self, id: &str) -> Result<AppView, String> {
        self.data
            .lock()
            .unwrap()
            .apps
            .iter()
            .find(|a| a.definition.id == id)
            .cloned()
            .ok_or("프로그램을 찾을 수 없습니다.".into())
    }
    fn update(&self, id: &str, f: impl FnOnce(&mut AppView)) {
        if let Some(a) = self
            .data
            .lock()
            .unwrap()
            .apps
            .iter_mut()
            .find(|a| a.definition.id == id)
        {
            f(a);
        }
    }
    fn status(&self, message: impl Into<String>) {
        self.data.lock().unwrap().status = message.into();
    }
    fn notify(&self, app: &tauri::AppHandle) {
        let mut data = self.data.lock().unwrap();
        if !data.settings.update_notifications {
            return;
        }
        let candidates: Vec<_> = data
            .apps
            .iter()
            .filter(|a| has_update(a) && !a.is_cached && a.error.is_none() && !a.busy)
            .filter(|a| {
                !data.announced.contains(&format!(
                    "{}|{}|{}",
                    a.definition.id,
                    a.definition.repository,
                    a.release.as_ref().unwrap().tag
                ))
            })
            .map(|a| {
                (
                    a.definition.id.clone(),
                    a.definition.name.clone(),
                    a.definition.repository.clone(),
                    a.release.as_ref().unwrap().tag.clone(),
                )
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let message = if candidates.len() == 1 {
            format!(
                "{} {} 업데이트를 사용할 수 있습니다. 클릭하여 확인하세요.",
                candidates[0].1, candidates[0].3
            )
        } else {
            format!(
                "{}개 도구의 새 버전이 있습니다. 클릭하여 확인하세요.",
                candidates.len()
            )
        };
        if app
            .notification()
            .builder()
            .title("프로그램 업데이트 · Moa Launcher")
            .body(message)
            .show()
            .is_ok()
        {
            for (id, _, repo, tag) in candidates {
                data.announced.insert(format!("{id}|{repo}|{tag}"));
                data.announced_ids.insert(id);
            }
        }
    }
}

fn restore(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
fn notification_click(app: &tauri::AppHandle) {
    let engine = app.state::<Shared>();
    let ids = {
        let mut d = engine.data.lock().unwrap();
        d.notification_ids = d.announced_ids.iter().cloned().collect();
        d.notification_ids.clone()
    };
    restore(app);
    engine.emit(app);
    let _ = app.emit("launcher-show-updates", ids);
}
fn request_exit(app: &tauri::AppHandle) -> Result<(), String> {
    let engine = app.state::<Shared>();
    if engine.activity.try_lock().is_err() {
        restore(app);
        return Err("진행 중인 작업을 마치거나 취소한 뒤 종료하세요.".into());
    }
    engine.exiting.store(true, Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

async fn check_all(app: tauri::AppHandle, engine: Shared) -> Result<(), String> {
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 확인하세요.")?;
    {
        let mut d = engine.data.lock().unwrap();
        d.checking = true;
        d.status = "새 버전을 확인하고 있습니다…".into();
    }
    engine.emit(&app);
    let definitions: Vec<_> = engine
        .data
        .lock()
        .unwrap()
        .apps
        .iter()
        .map(|a| a.definition.clone())
        .collect();
    for definition in definitions {
        if engine.exiting.load(Ordering::SeqCst) {
            break;
        }
        engine.update(&definition.id, |a| {
            a.busy = true;
            a.error = None;
            if a.definition.is_private {
                a.release = None;
                a.is_cached = false;
            }
        });
        if !definition.repository.is_empty() {
            let result = github::latest(&engine.client, &engine.paths, &definition).await;
            let installed = if definition.id == SELF_ID {
                engine.view(SELF_ID).ok().and_then(|a| a.installed)
            } else {
                install::find_installed(&engine.paths, &definition)
                    .ok()
                    .flatten()
            };
            engine.update(&definition.id, |a| {
                a.installed = installed;
                match result {
                    Ok(r) => {
                        a.release = Some(r.release);
                        a.is_cached = r.is_cached;
                    }
                    Err(e) => a.error = Some(e),
                }
            });
        }
        engine.update(&definition.id, |a| a.busy = false);
    }
    {
        let mut d = engine.data.lock().unwrap();
        d.checking = false;
        d.last_checked = Some(chrono::Local::now().format("%H:%M").to_string());
        let errors = d.apps.iter().filter(|a| a.error.is_some()).count();
        let count = d.apps.iter().filter(|a| has_update(a)).count();
        d.status = if errors > 0 {
            format!("{errors}개 도구의 확인 결과를 살펴보세요.")
        } else {
            format!("확인 완료 · 업데이트 가능한 도구 {count}개")
        };
    }
    engine.data.lock().unwrap().auth_generation += 1;
    engine.notify(&app);
    engine.emit(&app);
    Ok(())
}

async fn perform_install(
    app: &tauri::AppHandle,
    engine: &Shared,
    id: &str,
    update_only: bool,
) -> Result<bool, String> {
    let view = engine.view(id)?;
    if view.definition.repository.is_empty() {
        return Err("프로그램 설정에서 저장소를 연결하세요.".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    engine
        .cancellations
        .lock()
        .unwrap()
        .insert(id.into(), cancel.clone());
    engine.update(id, |a| {
        a.busy = true;
        a.can_cancel = true;
        a.error = None;
        a.progress = Some(InstallProgress {
            app_id: id.into(),
            stage: "배포 정보 확인 중…".into(),
            bytes_received: 0,
            total_bytes: None,
        });
        if a.definition.is_private {
            a.release = None;
        }
    });
    engine.emit(app);
    let callback_app = app.clone();
    let callback_engine = engine.clone();
    let callback_id = id.to_string();
    let last_emit = Mutex::new(Instant::now() - Duration::from_secs(1));
    let progress: Arc<dyn Fn(InstallProgress) + Send + Sync> = Arc::new(move |mut value| {
        value.app_id = callback_id.clone();
        let installer_started = matches!(
            value.stage.as_str(),
            "설치 프로그램 실행" | "설치 중" | "설치 완료"
        );
        callback_engine.update(&callback_id, |a| {
            a.progress = Some(value.clone());
            if installer_started {
                a.can_cancel = false;
            }
        });
        if installer_started {
            callback_engine.emit(&callback_app);
        }
        let mut last = last_emit.lock().unwrap();
        if last.elapsed() > Duration::from_millis(100) || value.bytes_received == 0 {
            *last = Instant::now();
            let _ = callback_app.emit("install-progress", value);
        }
    });
    let result: Result<bool, String> = async {
        let latest = github::latest(&engine.client, &engine.paths, &view.definition).await?;
        if latest.is_cached {
            return Err("최신 배포 정보를 확인한 뒤 다시 시도하세요.".into());
        }
        engine.update(id, |a| {
            a.release = Some(latest.release.clone());
            a.is_cached = false;
        });
        if !view.is_launcher {
            let installed = install::find_installed(&engine.paths, &view.definition)?;
            if update_only && installed.is_none() {
                return Err("설치된 프로그램을 찾을 수 없습니다. 목록을 다시 확인하세요.".into());
            }
            engine.update(id, |app| app.installed = installed);
        }
        let current = engine.view(id)?;
        if current.installed.is_some() && !has_update(&current) {
            return Ok(false);
        }
        if cancel.load(Ordering::SeqCst) {
            return Err("작업을 취소했습니다.".into());
        }
        if view.is_launcher {
            let prepared = self_update::prepare(
                &engine.client,
                &engine.paths,
                &latest.release,
                cancel.clone(),
                progress.clone(),
            )
            .await?;
            let background = app
                .get_webview_window("main")
                .and_then(|w| w.is_visible().ok())
                .map(|v| !v)
                .unwrap_or(false);
            self_update::handoff(&engine.paths, &prepared, background)?;
            engine.exiting.store(true, Ordering::SeqCst);
            app.exit(0);
            return Ok(true);
        }
        let installed = install::install(
            &engine.client,
            &engine.paths,
            &view.definition,
            &latest.release,
            cancel.clone(),
            progress,
        )
        .await?;
        engine.update(id, |a| a.installed = Some(installed));
        Ok(false)
    }
    .await;
    engine.cancellations.lock().unwrap().remove(id);
    engine.update(id, |a| {
        a.busy = false;
        a.can_cancel = false;
        a.progress = None;
        if let Err(ref e) = result {
            a.error = Some(e.clone());
        }
    });
    match &result {
        Ok(_) => engine.status(format!(
            "{} 설치 상태를 확인했습니다.",
            view.definition.name
        )),
        Err(e) => engine.status(e.clone()),
    };
    engine.emit(app);
    result
}
async fn update_all(app: tauri::AppHandle, engine: Shared) -> Result<(), String> {
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 업데이트하세요.")?;
    let mut ids: Vec<_> = engine
        .data
        .lock()
        .unwrap()
        .apps
        .iter()
        .filter(|a| has_update(a) && a.error.is_none())
        .map(|a| a.definition.id.clone())
        .collect();
    ids.sort_by_key(|id| id == SELF_ID);
    if ids.is_empty() {
        return Ok(());
    }
    engine.batch_cancel.store(false, Ordering::SeqCst);
    engine.data.lock().unwrap().batch_updating = true;
    engine.emit(&app);
    let _ = app.emit("launcher-show-updates", ids.clone());
    let mut result = Ok(());
    for id in ids {
        if engine.batch_cancel.load(Ordering::SeqCst) {
            result = Err("모두 업데이트를 취소했습니다.".into());
            break;
        }
        match perform_install(&app, &engine, &id, true).await {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    engine.data.lock().unwrap().batch_updating = false;
    if let Err(ref e) = result {
        engine.status(e.clone());
    }
    engine.emit(&app);
    result
}

#[tauri::command]
fn snapshot(engine: State<Shared>) -> Snapshot {
    engine.snapshot()
}
#[tauri::command]
async fn check_updates(app: tauri::AppHandle, engine: State<'_, Shared>) -> Result<(), String> {
    check_all(app, engine.inner().clone()).await
}
#[tauri::command]
async fn install_app(
    app: tauri::AppHandle,
    engine: State<'_, Shared>,
    id: String,
) -> Result<(), String> {
    let engine = engine.inner().clone();
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 설치하세요.")?;
    perform_install(&app, &engine, &id, false).await.map(|_| ())
}
#[tauri::command]
async fn update_all_apps(app: tauri::AppHandle, engine: State<'_, Shared>) -> Result<(), String> {
    update_all(app, engine.inner().clone()).await
}
#[tauri::command]
fn cancel_operation(engine: State<Shared>, id: Option<String>) {
    if let Some(id) = id {
        if let Some(cancel) = engine.cancellations.lock().unwrap().get(&id) {
            cancel.store(true, Ordering::SeqCst);
        }
    } else {
        engine.batch_cancel.store(true, Ordering::SeqCst);
        for cancel in engine.cancellations.lock().unwrap().values() {
            cancel.store(true, Ordering::SeqCst);
        }
    }
}
#[tauri::command]
fn update_settings(
    app: tauri::AppHandle,
    engine: State<Shared>,
    patch: serde_json::Value,
) -> Result<(), String> {
    let mut data = engine.data.lock().unwrap();
    let mut json = serde_json::to_value(&data.settings).map_err(|_| "설정을 읽지 못했습니다.")?;
    for (key, value) in patch.as_object().ok_or("설정 형식이 올바르지 않습니다.")? {
        if ![
            "checkOnStartup",
            "periodicChecks",
            "updateNotifications",
            "useGridView",
            "favoriteAppIds",
            "checkIntervalMinutes",
        ]
        .contains(&key.as_str())
        {
            return Err("변경할 수 없는 설정입니다.".into());
        }
        json[key] = value.clone();
    }
    let settings: Settings =
        serde_json::from_value(json).map_err(|_| "설정 형식이 올바르지 않습니다.")?;
    storage::save_settings(&engine.paths, &settings)?;
    data.settings = storage::load_settings(&engine.paths)?;
    data.next_check =
        Instant::now() + Duration::from_secs(u64::from(data.settings.check_interval_minutes) * 60);
    drop(data);
    engine.emit(&app);
    Ok(())
}
#[tauri::command]
fn set_startup(app: tauri::AppHandle, engine: State<Shared>, enabled: bool) -> Result<(), String> {
    platform::set_startup(&engine.paths, enabled)?;
    engine.emit(&app);
    Ok(())
}
#[tauri::command]
async fn connect_github(
    app: tauri::AppHandle,
    engine: State<'_, Shared>,
    token: String,
) -> Result<(), String> {
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 연결하세요.")?;
    let login = github::verify_token(&engine.client, &token).await?;
    credentials::save_token(&engine.paths, &token)?;
    invalidate_private(&engine);
    engine.status(format!("GitHub @{login} 연결 완료"));
    engine.emit(&app);
    Ok(())
}
fn invalidate_private(engine: &Engine) {
    let mut data = engine.data.lock().unwrap();
    data.auth_generation += 1;
    data.notification_ids.clear();
    data.announced.clear();
    data.announced_ids.clear();
    for view in &mut data.apps {
        if view.definition.is_private {
            view.release = None;
            view.error = None;
            view.is_cached = false;
        }
    }
}
#[tauri::command]
fn disconnect_github(app: tauri::AppHandle, engine: State<Shared>) -> Result<(), String> {
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 연결을 해제하세요.")?;
    credentials::delete_token(&engine.paths)?;
    invalidate_private(&engine);
    engine.status("GitHub 연결을 해제했습니다.");
    engine.emit(&app);
    Ok(())
}
#[derive(Serialize)]
struct Artwork {
    icon: Option<String>,
    cover: Option<String>,
}
#[tauri::command]
async fn load_artwork(engine: State<'_, Shared>, id: String) -> Result<Artwork, String> {
    let view = engine.view(&id)?;
    let generation = engine.data.lock().unwrap().auth_generation;
    let icon = if let Some(url) = &view.definition.icon_url {
        artwork::load(&engine.client, &engine.paths, &view.definition, url).await?
    } else {
        None
    };
    let cover = if let Some(url) = &view.definition.cover_url {
        artwork::load(&engine.client, &engine.paths, &view.definition, url).await?
    } else {
        None
    };
    if generation != engine.data.lock().unwrap().auth_generation {
        return Ok(Artwork {
            icon: None,
            cover: None,
        });
    }
    Ok(Artwork { icon, cover })
}
#[tauri::command]
async fn save_program(
    app: tauri::AppHandle,
    engine: State<'_, Shared>,
    definition: AppDefinition,
) -> Result<(), String> {
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 저장하세요.")?;
    if definition.id == SELF_ID {
        return Err("런처 항목은 자동으로 관리합니다.".into());
    }
    storage::validate_catalog(&Catalog {
        schema_version: 1,
        apps: vec![definition.clone()],
    })?;
    let resolved = github::resolve_visibility(&engine.client, &engine.paths, &definition).await?;
    let mut personal = storage::load_personal_catalog(&engine.paths)?;
    personal.apps.retain(|a| a.id != resolved.id);
    personal.apps.push(resolved);
    storage::save_personal_catalog(&engine.paths, &personal)?;
    engine.rebuild()?;
    engine.status("프로그램 설정을 저장했습니다.");
    engine.emit(&app);
    Ok(())
}
#[tauri::command]
async fn import_catalog(app: tauri::AppHandle) -> Result<Option<Catalog>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("앱 카탈로그", &["json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| "파일 선택을 완료하지 못했습니다.")?;
    let Some(path) = picked else { return Ok(None) };
    let path = path.into_path().map_err(|_| "로컬 파일을 선택하세요.")?;
    if std::fs::metadata(&path)
        .map_err(|_| "파일을 읽지 못했습니다.")?
        .len()
        > 4 * 1024 * 1024
    {
        return Err("카탈로그 파일이 너무 큽니다.".into());
    }
    let catalog: Catalog =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| "파일을 읽지 못했습니다.")?)
            .map_err(|_| "카탈로그 형식이 올바르지 않습니다.")?;
    storage::validate_catalog(&catalog)?;
    if catalog.apps.iter().any(|a| a.id == SELF_ID) {
        return Err("런처 항목은 가져올 수 없습니다.".into());
    }
    Ok(Some(catalog))
}
#[tauri::command]
async fn confirm_import(
    app: tauri::AppHandle,
    engine: State<'_, Shared>,
    catalog: Catalog,
) -> Result<(), String> {
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 가져오세요.")?;
    storage::validate_catalog(&catalog)?;
    if catalog.apps.iter().any(|a| a.id == SELF_ID) {
        return Err("런처 항목은 가져올 수 없습니다.".into());
    }
    let mut personal = storage::load_personal_catalog(&engine.paths)?;
    for definition in catalog.apps {
        let resolved =
            github::resolve_visibility(&engine.client, &engine.paths, &definition).await?;
        personal.apps.retain(|a| a.id != resolved.id);
        personal.apps.push(resolved);
    }
    storage::save_personal_catalog(&engine.paths, &personal)?;
    engine.rebuild()?;
    engine.status("카탈로그를 가져왔습니다.");
    engine.emit(&app);
    Ok(())
}
#[tauri::command]
async fn export_catalog(app: tauri::AppHandle, engine: State<'_, Shared>) -> Result<(), String> {
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 내보내세요.")?;
    let definitions = storage::load_personal_catalog(&engine.paths)?.apps;
    let exported = github::public_export(&engine.client, &definitions).await?;
    let destination = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("앱 카탈로그", &["json"])
            .set_file_name("moa-catalog.json")
            .blocking_save_file()
    })
    .await
    .map_err(|_| "파일 선택에 실패했습니다.")?;
    if let Some(path) = destination {
        let path = path.into_path().map_err(|_| "로컬 파일을 선택하세요.")?;
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&exported).map_err(|_| "카탈로그를 만들지 못했습니다.")?,
        )
        .map_err(|_| "내보내기 파일을 저장하지 못했습니다.")?;
    }
    Ok(())
}
#[tauri::command]
fn launch_app(engine: State<Shared>, id: String) -> Result<(), String> {
    let view = engine.view(&id)?;
    if view.is_launcher || view.busy {
        return Err("지금 실행할 수 없는 프로그램입니다.".into());
    }
    install::launch(&view.installed.ok_or("설치된 프로그램이 없습니다.")?)
}
#[tauri::command]
fn open_location(engine: State<Shared>, id: Option<String>) -> Result<(), String> {
    let path = match id {
        Some(id) => PathBuf::from(
            engine
                .view(&id)?
                .installed
                .ok_or("설치된 프로그램이 없습니다.")?
                .install_directory,
        ),
        None => engine.paths.data_dir.clone(),
    };
    platform::open_folder(&path)
}
#[tauri::command]
fn open_github(engine: State<Shared>, id: String) -> Result<(), String> {
    platform::open_repository(&engine.view(&id)?.definition.repository)
}
#[tauri::command]
fn exit_launcher(app: tauri::AppHandle) -> Result<(), String> {
    request_exit(&app)
}
#[tauri::command]
fn dismiss_update_result(app: tauri::AppHandle, engine: State<Shared>) -> Result<(), String> {
    let source = engine.paths.data_dir.join("self-update-result.json");
    if source.exists() {
        std::fs::rename(
            &source,
            engine
                .paths
                .data_dir
                .join(format!("self-update-result.{}.json", uuid::Uuid::new_v4())),
        )
        .map_err(|_| "업데이트 결과를 보관하지 못했습니다.")?;
    }
    engine.data.lock().unwrap().update_result = None;
    engine.emit(&app);
    Ok(())
}
#[tauri::command]
fn open_token_page() -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg("https://github.com/settings/personal-access-tokens/new")
        .spawn()
        .map(|_| ())
        .map_err(|_| "토큰 발급 페이지를 열지 못했습니다.".into())
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|s| s == "--apply-self-update") {
        if let Some(plan) = args.get(index + 1) {
            if let Err(error) = self_update::run_helper(Path::new(plan)) {
                eprintln!("자체 업데이트 helper: {error}");
                std::process::exit(1);
            }
        } else {
            std::process::exit(1);
        }
        return;
    }
    let background = args.iter().any(|s| s == "--background");
    let custom = args
        .iter()
        .position(|s| s == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let paths = Paths::new(custom);
    let guard = match platform::InstanceGuard::acquire(&paths, !background) {
        Ok(Some(g)) => g,
        Ok(None) => return,
        Err(e) => {
            platform::show_error(&e);
            return;
        }
    };
    let engine = match Engine::new(paths) {
        Ok(e) => e,
        Err(e) => {
            platform::show_error(&e);
            return;
        }
    };
    let setup_engine = engine.clone();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(engine.clone())
        .manage(guard)
        .invoke_handler(tauri::generate_handler![
            snapshot,
            check_updates,
            install_app,
            update_all_apps,
            cancel_operation,
            update_settings,
            set_startup,
            connect_github,
            disconnect_github,
            load_artwork,
            save_program,
            import_catalog,
            confirm_import,
            export_catalog,
            launch_app,
            open_location,
            open_github,
            exit_launcher,
            dismiss_update_result,
            open_token_page
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let open = MenuItem::with_id(app, "open", "모아 런처 열기", true, None::<&str>)?;
            let update =
                MenuItem::with_id(app, "update-all", "모두 업데이트", false, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &update, &quit])?;
            *setup_engine.tray_update.lock().unwrap() = Some(update.clone());
            TrayIconBuilder::with_id("moa-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Moa Launcher")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => restore(app),
                    "quit" => {
                        let _ = request_exit(app);
                    }
                    "update-all" => {
                        let app = app.clone();
                        let engine = app.state::<Shared>().inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = update_all(app, engine).await;
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        restore(tray.app_handle());
                    }
                })
                .build(app)?;
            let notify_handle = handle.clone();
            app.notification()
                .on_action(move |_| notification_click(&notify_handle))?;
            if !background {
                restore(&handle);
            }
            let event = app.state::<platform::InstanceGuard>().event_handle();
            let activation = handle.clone();
            let activation_engine = setup_engine.clone();
            std::thread::spawn(move || {
                while !activation_engine.exiting.load(Ordering::SeqCst) {
                    let result = unsafe {
                        windows_sys::Win32::System::Threading::WaitForSingleObject(event as _, 500)
                    };
                    if result == 0 {
                        restore(&activation);
                    } else if result == u32::MAX {
                        break;
                    }
                }
            });
            let schedule = setup_engine.clone();
            tauri::async_runtime::spawn(async move {
                let initial = schedule.data.lock().unwrap().settings.check_on_startup;
                if initial {
                    let _ = check_all(handle.clone(), schedule.clone()).await;
                }
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if schedule.exiting.load(Ordering::SeqCst) {
                        break;
                    }
                    let due = {
                        let mut d = schedule.data.lock().unwrap();
                        let due = d.settings.periodic_checks && Instant::now() >= d.next_check;
                        if due {
                            d.next_check = Instant::now()
                                + Duration::from_secs(
                                    u64::from(d.settings.check_interval_minutes) * 60,
                                );
                        }
                        due
                    };
                    if due {
                        let _ = check_all(handle.clone(), schedule.clone()).await;
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let engine = window.state::<Shared>();
                if !engine.exiting.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        });
    match builder.build(tauri::generate_context!()) {
        Ok(app) => app.run(move |_, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                engine.exiting.store(true, Ordering::SeqCst);
            }
        }),
        Err(e) => platform::show_error(&format!("런처를 시작하지 못했습니다: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions_and_launcher_contract() {
        assert!(newer("v1.2.0", "1.1.0"));
        assert!(!newer("v1.0.0", "2.0.0"));
        assert_eq!(
            launcher_definition().packages[0].asset_pattern,
            "MoaLauncher-*-Setup-x64.exe"
        );
    }
}
