//! UI-independent application engine for the native launcher.
//!
//! The engine owns application state and schedules network/disk-heavy work on the
//! Tokio runtime supplied by the host. The UI can poll snapshots and drain events.

use crate::{
    artwork, credentials, github, install,
    models::*,
    platform, self_update,
    storage::{self, Paths},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, sync::oneshot};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SELF_ID: &str = "moa-launcher";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    pub definition: AppDefinition,
    pub installed: Option<InstalledApp>,
    pub release: Option<AppRelease>,
    pub is_cached: bool,
    pub is_launcher: bool,
    pub busy: bool,
    pub can_cancel: bool,
    pub error: Option<String>,
    pub progress: Option<InstallProgress>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub apps: Vec<AppView>,
    pub settings: Settings,
    pub github_connected: bool,
    pub startup_enabled: bool,
    pub data_directory: String,
    pub version: String,
    pub checking: bool,
    pub batch_updating: bool,
    pub working: bool,
    pub status: String,
    pub last_checked: Option<String>,
    pub notification_ids: Vec<String>,
    pub auth_generation: u64,
    pub update_result: Option<UpdateResult>,
    pub force_close_prompt: Option<ForceClosePrompt>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForceClosePrompt {
    pub request_id: String,
    pub app_id: String,
    pub app_name: String,
    pub process_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    #[serde(alias = "Success")]
    pub success: bool,
    #[serde(alias = "Message")]
    pub message: String,
    #[serde(default, alias = "LogPath")]
    pub log_path: String,
    #[serde(alias = "Version")]
    pub version: String,
    #[serde(alias = "CompletedAt")]
    pub completed_at: String,
}

#[derive(Clone, Debug)]
pub enum EngineEvent {
    Changed,
    Notify {
        ids: Vec<String>,
        body: String,
    },
    ExitRequested,
    Error(String),
    ImportPreview(Catalog),
    CatalogSaved,
    ForceCloseRequested,
    Artwork {
        id: String,
        icon: Option<String>,
        cover: Option<String>,
        generation: u64,
    },
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
    working: bool,
    force_close_prompt: Option<ForceClosePrompt>,
}

struct PendingForceClose {
    prompt: ForceClosePrompt,
    reply: oneshot::Sender<bool>,
}

struct ForceClosePromptGuard {
    engine: Weak<Engine>,
    request_id: String,
}

impl Drop for ForceClosePromptGuard {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.upgrade() {
            engine.clear_force_close_request(&self.request_id);
        }
    }
}

struct Engine {
    paths: Paths,
    client: reqwest::Client,
    runtime: Handle,
    wake: Arc<dyn Fn() + Send + Sync>,
    event_tx: Sender<EngineEvent>,
    event_rx: Mutex<Receiver<EngineEvent>>,
    data: Mutex<Data>,
    activity: tokio::sync::Mutex<()>,
    cancellations: Mutex<HashMap<String, Arc<AtomicBool>>>,
    force_close: Mutex<Option<PendingForceClose>>,
    batch_cancel: AtomicBool,
    exiting: AtomicBool,
    background: AtomicBool,
}

#[derive(Clone)]
pub struct SharedEngine(Arc<Engine>);

fn newer(latest: &str, current: &str) -> bool {
    match (
        semver::Version::parse(latest.trim_start_matches(['v', 'V'])),
        semver::Version::parse(current.trim_start_matches(['v', 'V'])),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}

pub fn has_update(app: &AppView) -> bool {
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

impl SharedEngine {
    pub fn new(
        paths: Paths,
        runtime: Handle,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let settings = storage::load_settings(&paths)?;
        let interval = Duration::from_secs(u64::from(settings.check_interval_minutes) * 60);
        let result_path = paths.data_dir.join("self-update-result.json");
        let update_result = std::fs::metadata(&result_path)
            .ok()
            .filter(|m| m.len() <= 65536)
            .and_then(|_| std::fs::read(&result_path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let (event_tx, event_rx) = mpsc::channel();
        let engine = Arc::new(Engine {
            paths,
            client: github::client()?,
            runtime,
            wake,
            event_tx,
            event_rx: Mutex::new(event_rx),
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
                working: false,
                force_close_prompt: None,
            }),
            activity: tokio::sync::Mutex::new(()),
            cancellations: Mutex::new(HashMap::new()),
            force_close: Mutex::new(None),
            batch_cancel: AtomicBool::new(false),
            exiting: AtomicBool::new(false),
            background: AtomicBool::new(false),
        });
        engine.rebuild()?;
        Ok(Self(engine))
    }

    pub fn snapshot(&self) -> Snapshot {
        self.0.snapshot()
    }

    pub fn drain_events(&self) -> Vec<EngineEvent> {
        self.0.event_rx.lock().unwrap().try_iter().collect()
    }

    pub fn start(&self) {
        let engine = self.0.clone();
        let runtime = engine.runtime.clone();
        runtime.spawn(async move {
            let initial = engine.data.lock().unwrap().settings.check_on_startup;
            if initial {
                let _ = check_all(engine.clone()).await;
            }
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if engine.exiting.load(Ordering::SeqCst) {
                    break;
                }
                let due = {
                    let mut d = engine.data.lock().unwrap();
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
                    let _ = check_all(engine.clone()).await;
                }
            }
        });
    }

    pub fn request_exit(&self) -> Result<(), String> {
        if self.0.activity.try_lock().is_err() {
            return Err("진행 중인 작업을 마치거나 취소한 뒤 종료하세요.".into());
        }
        self.0.exiting.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Keep the next launcher process hidden after a self-update handoff.
    pub fn set_background(&self, background: bool) {
        self.0.background.store(background, Ordering::SeqCst);
    }

    pub fn check_updates(&self) -> Result<(), String> {
        let engine = self.0.clone();
        self.0.runtime.spawn(async move {
            if let Err(e) = check_all(engine.clone()).await {
                engine.error(e);
            }
        });
        Ok(())
    }
    pub fn install(&self, id: String) -> Result<(), String> {
        let engine = self.0.clone();
        self.0.runtime.spawn(async move {
            if let Err(e) = install_one(engine.clone(), &id, false).await {
                engine.error(e);
            }
        });
        Ok(())
    }
    pub fn update_all(&self) -> Result<(), String> {
        let engine = self.0.clone();
        self.0.runtime.spawn(async move {
            if let Err(e) = update_all(engine.clone()).await {
                engine.error(e);
            }
        });
        Ok(())
    }
    pub fn save_settings(&self, patch: serde_json::Value) -> Result<(), String> {
        self.0.save_settings(patch)
    }
    pub fn set_startup(&self, enabled: bool) -> Result<(), String> {
        platform::set_startup(&self.0.paths, enabled)?;
        self.0.changed();
        Ok(())
    }
    pub fn connect_github(&self, token: String) -> Result<(), String> {
        let engine = self.0.clone();
        self.0.runtime.spawn(async move {
            let mut marked_working = false;
            let result = async {
                let _guard = engine
                    .activity
                    .try_lock()
                    .map_err(|_| "진행 중인 작업이 끝난 뒤 연결하세요.")?;
                engine.set_working(true);
                marked_working = true;
                let login = github::verify_token(&engine.client, &token).await?;
                credentials::save_token(&engine.paths, &token)?;
                engine.invalidate_private();
                engine.status(format!("GitHub @{login} 연결 완료"));
                Ok::<_, String>(())
            }
            .await;
            if marked_working {
                engine.set_working(false);
            }
            if let Err(e) = result {
                engine.error(e);
            } else {
                engine.changed();
            }
        });
        Ok(())
    }
    pub fn disconnect_github(&self) -> Result<(), String> {
        let _guard = self
            .0
            .activity
            .try_lock()
            .map_err(|_| "진행 중인 작업이 끝난 뒤 연결을 해제하세요.")?;
        self.0.set_working(true);
        let result = credentials::delete_token(&self.0.paths);
        self.0.set_working(false);
        result?;
        self.0.invalidate_private();
        self.0.status("GitHub 연결을 해제했습니다.");
        self.0.changed();
        Ok(())
    }
    pub fn save_program(&self, definition: AppDefinition) -> Result<(), String> {
        let engine = self.0.clone();
        self.0.runtime.spawn(async move {
            let mut marked_working = false;
            let result = async {
                let _guard = engine
                    .activity
                    .try_lock()
                    .map_err(|_| "진행 중인 작업이 끝난 뒤 저장하세요.")?;
                engine.set_working(true);
                marked_working = true;
                if definition.id == SELF_ID {
                    return Err("런처 항목은 자동으로 관리합니다.".into());
                }
                storage::validate_catalog(&Catalog {
                    schema_version: 1,
                    apps: vec![definition.clone()],
                })?;
                let resolved =
                    github::resolve_visibility(&engine.client, &engine.paths, &definition).await?;
                let mut personal = storage::load_personal_catalog(&engine.paths)?;
                personal.apps.retain(|a| a.id != resolved.id);
                personal.apps.push(resolved);
                storage::save_personal_catalog(&engine.paths, &personal)?;
                engine.rebuild()?;
                engine.status("프로그램 설정을 저장했습니다.");
                Ok::<_, String>(())
            }
            .await;
            if marked_working {
                engine.set_working(false);
            }
            if let Err(e) = result {
                engine.error(e);
            } else {
                engine.event(EngineEvent::CatalogSaved);
                engine.changed();
            }
        });
        Ok(())
    }
    pub fn preview_import(&self, path: PathBuf) -> Result<(), String> {
        let engine = self.0.clone();
        self.0.runtime.spawn_blocking(move || {
            let result = read_catalog(&path);
            match result {
                Ok(catalog) => engine.event(EngineEvent::ImportPreview(catalog)),
                Err(e) => engine.error(e),
            }
        });
        Ok(())
    }
    pub fn confirm_import(&self, catalog: Catalog) -> Result<(), String> {
        let engine = self.0.clone();
        self.0.runtime.spawn(async move {
            let mut marked_working = false;
            let result = async {
                let _guard = engine
                    .activity
                    .try_lock()
                    .map_err(|_| "진행 중인 작업이 끝난 뒤 가져오세요.")?;
                engine.set_working(true);
                marked_working = true;
                storage::validate_catalog(&catalog)?;
                if catalog.apps.iter().any(|a| a.id == SELF_ID) {
                    return Err("런처 항목은 가져올 수 없습니다.".into());
                }
                let mut personal = storage::load_personal_catalog(&engine.paths)?;
                for definition in catalog.apps {
                    let resolved =
                        github::resolve_visibility(&engine.client, &engine.paths, &definition)
                            .await?;
                    personal.apps.retain(|a| a.id != resolved.id);
                    personal.apps.push(resolved);
                }
                storage::save_personal_catalog(&engine.paths, &personal)?;
                engine.rebuild()?;
                engine.status("카탈로그를 가져왔습니다.");
                Ok::<_, String>(())
            }
            .await;
            if marked_working {
                engine.set_working(false);
            }
            if let Err(e) = result {
                engine.error(e);
            } else {
                engine.event(EngineEvent::CatalogSaved);
                engine.changed();
            }
        });
        Ok(())
    }
    pub fn export_catalog(&self, path: PathBuf) -> Result<(), String> {
        let engine = self.0.clone();
        self.0.runtime.spawn(async move {
            let mut marked_working = false;
            let result = async {
                let _guard = engine
                    .activity
                    .try_lock()
                    .map_err(|_| "진행 중인 작업이 끝난 뒤 내보내세요.")?;
                engine.set_working(true);
                marked_working = true;
                let definitions = storage::load_personal_catalog(&engine.paths)?.apps;
                let exported = github::public_export(&engine.client, &definitions).await?;
                let bytes = serde_json::to_vec_pretty(&exported)
                    .map_err(|_| "카탈로그를 만들지 못했습니다.")?;
                tokio::fs::write(path, bytes)
                    .await
                    .map_err(|_| "내보내기 파일을 저장하지 못했습니다.".to_string())
            }
            .await;
            if marked_working {
                engine.set_working(false);
            }
            if let Err(e) = result {
                engine.error(e);
            }
        });
        Ok(())
    }
    pub fn load_artwork(&self, id: String) -> Result<(), String> {
        let engine = self.0.clone();
        self.0.runtime.spawn(async move {
            let (view, generation) = {
                let data = engine.data.lock().unwrap();
                let Some(view) = data
                    .apps
                    .iter()
                    .find(|app| app.definition.id == id)
                    .cloned()
                else {
                    return;
                };
                (view, data.auth_generation)
            };
            let icon = if let Some(url) = &view.definition.icon_url {
                artwork::load(&engine.client, &engine.paths, &view.definition, url)
                    .await
                    .unwrap_or(None)
            } else {
                None
            };
            let cover = if let Some(url) = &view.definition.cover_url {
                artwork::load(&engine.client, &engine.paths, &view.definition, url)
                    .await
                    .unwrap_or(None)
            } else {
                None
            };
            let queued = {
                let data = engine.data.lock().unwrap();
                if artwork_generation_is_current(data.auth_generation, generation)
                    && data.apps.iter().any(|app| app.definition.id == id)
                {
                    let _ = engine.event_tx.send(EngineEvent::Artwork {
                        id,
                        icon,
                        cover,
                        generation,
                    });
                    true
                } else {
                    false
                }
            };
            if queued {
                (engine.wake)();
            }
        });
        Ok(())
    }
    pub fn cancel(&self, id: Option<String>) {
        if let Some(id) = id {
            if let Some(c) = self.0.cancellations.lock().unwrap().get(&id) {
                c.store(true, Ordering::SeqCst);
            }
            self.0.cancel_force_close(&id);
        } else {
            self.0.batch_cancel.store(true, Ordering::SeqCst);
            for c in self.0.cancellations.lock().unwrap().values() {
                c.store(true, Ordering::SeqCst);
            }
            self.0.cancel_force_close_all();
        }
    }

    /// Resolves the currently displayed force-close confirmation only when its
    /// request ID still matches. A denial also cancels the matching install.
    pub fn respond_force_close(&self, request_id: &str, approved: bool) -> bool {
        let pending = {
            let mut slot = self.0.force_close.lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(|pending| pending.prompt.request_id == request_id)
            {
                slot.take()
            } else {
                None
            }
        };
        let Some(pending) = pending else {
            return false;
        };
        if !approved
            && let Some(cancel) = self
                .0
                .cancellations
                .lock()
                .unwrap()
                .get(&pending.prompt.app_id)
        {
            cancel.store(true, Ordering::SeqCst);
        }
        self.0.clear_force_close_snapshot(request_id);
        let _ = pending.reply.send(approved);
        true
    }
    pub fn launch(&self, id: &str) -> Result<(), String> {
        let view = self.0.view(id)?;
        if view.is_launcher || view.busy {
            return Err("지금 실행할 수 없는 프로그램입니다.".into());
        }
        install::launch(&view.installed.ok_or("설치된 프로그램이 없습니다.")?)
    }
    pub fn open_location(&self, id: Option<String>) -> Result<(), String> {
        let path = match id {
            Some(id) => PathBuf::from(
                self.0
                    .view(&id)?
                    .installed
                    .ok_or("설치된 프로그램이 없습니다.")?
                    .install_directory,
            ),
            None => self.0.paths.data_dir.clone(),
        };
        platform::open_folder(&path)
    }
    pub fn open_github(&self, id: &str) -> Result<(), String> {
        platform::open_repository(&self.0.view(id)?.definition.repository)
    }
    pub fn dismiss_update_result(&self) -> Result<(), String> {
        let source = self.0.paths.data_dir.join("self-update-result.json");
        if source.exists() {
            std::fs::rename(
                &source,
                self.0
                    .paths
                    .data_dir
                    .join(format!("self-update-result.{}.json", uuid::Uuid::new_v4())),
            )
            .map_err(|_| "업데이트 결과를 보관하지 못했습니다.")?;
        }
        self.0.data.lock().unwrap().update_result = None;
        self.0.changed();
        Ok(())
    }
    pub fn open_token_page(&self) -> Result<(), String> {
        std::process::Command::new("explorer.exe")
            .arg("https://github.com/settings/personal-access-tokens/new")
            .spawn()
            .map(|_| ())
            .map_err(|_| "토큰 발급 페이지를 열지 못했습니다.".into())
    }
}

impl Engine {
    fn event(&self, event: EngineEvent) {
        let _ = self.event_tx.send(event);
        (self.wake)();
    }
    fn changed(&self) {
        self.event(EngineEvent::Changed);
    }
    fn error(&self, message: String) {
        self.status(message.clone());
        self.event(EngineEvent::Error(message));
        self.changed();
    }
    fn status(&self, message: impl Into<String>) {
        self.data.lock().unwrap().status = message.into();
    }
    fn set_working(&self, working: bool) {
        self.data.lock().unwrap().working = working;
        self.changed();
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
            data.apps.push(AppView {
                definition,
                installed,
                release: old.and_then(|a| a.release.clone()),
                is_cached: old.is_some_and(|a| a.is_cached),
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
            working: data.working,
            status: data.status.clone(),
            last_checked: data.last_checked.clone(),
            notification_ids: data.notification_ids.clone(),
            auth_generation: data.auth_generation,
            update_result: data.update_result.clone(),
            force_close_prompt: data.force_close_prompt.clone(),
        }
    }

    fn request_force_close(
        &self,
        prompt: ForceClosePrompt,
        reply: oneshot::Sender<bool>,
    ) -> Result<(), oneshot::Sender<bool>> {
        let mut slot = self.force_close.lock().unwrap();
        if slot.is_some() {
            return Err(reply);
        }
        self.data.lock().unwrap().force_close_prompt = Some(prompt.clone());
        *slot = Some(PendingForceClose { prompt, reply });
        Ok(())
    }

    fn clear_force_close_snapshot(&self, request_id: &str) {
        let mut data = self.data.lock().unwrap();
        if data
            .force_close_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.request_id == request_id)
        {
            data.force_close_prompt = None;
        }
        drop(data);
        self.changed();
    }

    fn clear_force_close_request(&self, request_id: &str) {
        let pending = {
            let mut slot = self.force_close.lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(|pending| pending.prompt.request_id == request_id)
            {
                slot.take()
            } else {
                None
            }
        };
        if pending.is_some() {
            self.clear_force_close_snapshot(request_id);
        }
    }

    fn cancel_force_close(&self, app_id: &str) {
        let request_id = {
            let mut slot = self.force_close.lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(|pending| pending.prompt.app_id == app_id)
            {
                slot.take().map(|pending| {
                    let request_id = pending.prompt.request_id.clone();
                    let _ = pending.reply.send(false);
                    request_id
                })
            } else {
                None
            }
        };
        if let Some(request_id) = request_id {
            self.clear_force_close_snapshot(&request_id);
        }
    }

    fn cancel_force_close_all(&self) {
        let request_id = {
            let mut slot = self.force_close.lock().unwrap();
            slot.take().map(|pending| {
                let request_id = pending.prompt.request_id.clone();
                let _ = pending.reply.send(false);
                request_id
            })
        };
        if let Some(request_id) = request_id {
            self.clear_force_close_snapshot(&request_id);
        }
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
    fn invalidate_private(&self) {
        let mut data = self.data.lock().unwrap();
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
    fn save_settings(&self, patch: serde_json::Value) -> Result<(), String> {
        let mut data = self.data.lock().unwrap();
        let mut json =
            serde_json::to_value(&data.settings).map_err(|_| "설정을 읽지 못했습니다.")?;
        for (key, value) in patch.as_object().ok_or("설정 형식이 올바르지 않습니다.")?
        {
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
        storage::save_settings(&self.paths, &settings)?;
        data.settings = storage::load_settings(&self.paths)?;
        data.next_check = Instant::now()
            + Duration::from_secs(u64::from(data.settings.check_interval_minutes) * 60);
        drop(data);
        self.changed();
        Ok(())
    }
    fn notify(&self) {
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
        let body = if candidates.len() == 1 {
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
        let ids: Vec<_> = candidates.iter().map(|c| c.0.clone()).collect();
        for (id, _, repo, tag) in candidates {
            data.announced.insert(format!("{id}|{repo}|{tag}"));
            data.announced_ids.insert(id);
        }
        data.notification_ids = ids.clone();
        drop(data);
        self.event(EngineEvent::Notify { ids, body });
    }
}

fn artwork_generation_is_current(current: u64, requested: u64) -> bool {
    current == requested
}

fn read_catalog(path: &Path) -> Result<Catalog, String> {
    if std::fs::metadata(path)
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
    Ok(catalog)
}

async fn check_all(engine: Arc<Engine>) -> Result<(), String> {
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 확인하세요.")?;
    {
        let mut d = engine.data.lock().unwrap();
        d.checking = true;
        d.status = "새 버전을 확인하고 있습니다…".into();
    }
    engine.changed();
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
        engine.changed();
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
        d.auth_generation += 1;
    }
    engine.notify();
    engine.changed();
    Ok(())
}

async fn install_one(engine: Arc<Engine>, id: &str, update_only: bool) -> Result<bool, String> {
    let _guard = engine
        .activity
        .try_lock()
        .map_err(|_| "진행 중인 작업이 끝난 뒤 설치하세요.")?;
    perform_install(&engine, id, update_only).await
}

async fn perform_install(
    engine: &Arc<Engine>,
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
    engine.changed();
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
        let mut last = last_emit.lock().unwrap();
        if last.elapsed() > Duration::from_millis(100) || value.bytes_received == 0 {
            *last = Instant::now();
            callback_engine.changed();
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
            self_update::handoff(
                &engine.paths,
                &prepared,
                engine.background.load(Ordering::SeqCst),
            )?;
            engine.exiting.store(true, Ordering::SeqCst);
            engine.event(EngineEvent::ExitRequested);
            return Ok(true);
        }
        let installed = install::install(
            &engine.client,
            &engine.paths,
            &view.definition,
            &latest.release,
            cancel.clone(),
            progress,
            force_close_confirmation(engine.clone(), cancel.clone()),
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
    }
    engine.changed();
    result
}

fn force_close_confirmation(
    engine: Arc<Engine>,
    cancel: Arc<AtomicBool>,
) -> install::ConfirmForceClose {
    Arc::new(move |installed, process_count| {
        let engine = engine.clone();
        let cancel = cancel.clone();
        Box::pin(async move {
            if cancel.load(Ordering::SeqCst) {
                return Ok(false);
            }
            let app_name = engine
                .view(&installed.app_id)
                .map(|app| app.definition.name)
                .unwrap_or_else(|_| installed.app_id.clone());
            let prompt = ForceClosePrompt {
                request_id: uuid::Uuid::new_v4().to_string(),
                app_id: installed.app_id.clone(),
                app_name,
                process_count,
            };
            let (reply, response) = oneshot::channel();
            let request_id = prompt.request_id.clone();
            engine
                .request_force_close(prompt, reply)
                .map_err(|_| "다른 프로그램의 종료 확인이 진행 중입니다.".to_owned())?;
            let _guard = ForceClosePromptGuard {
                engine: Arc::downgrade(&engine),
                request_id,
            };
            if cancel.load(Ordering::SeqCst) {
                engine.cancel_force_close(&installed.app_id);
                return Ok(false);
            }
            engine.event(EngineEvent::ForceCloseRequested);
            engine.changed();
            let approved = response.await.unwrap_or(false);
            Ok(approved && !cancel.load(Ordering::SeqCst))
        }) as Pin<Box<dyn Future<Output = Result<bool, String>> + Send>>
    })
}

async fn update_all(engine: Arc<Engine>) -> Result<(), String> {
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
    engine.changed();
    let mut result = Ok(());
    for id in ids {
        if engine.batch_cancel.load(Ordering::SeqCst) {
            result = Err("모두 업데이트를 취소했습니다.".into());
            break;
        }
        match perform_install(&engine, &id, true).await {
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
    engine.changed();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> (SharedEngine, tokio::runtime::Runtime, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "moa-engine-test-{}-{}",
            std::process::id(),
            FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let settings = Settings {
            favorite_app_ids: vec!["moaview".into(), "moavcs".into()],
            use_grid_view: false,
            check_interval_minutes: 1440,
            ..Default::default()
        };
        storage::save_settings(&Paths::new(Some(path.clone())), &settings).unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let engine = SharedEngine::new(
            Paths::new(Some(path.clone())),
            runtime.handle().clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        (engine, runtime, path)
    }

    fn remove_fixture(path: &Path) {
        let _ = std::fs::remove_dir_all(path);
    }

    fn installed_fixture(app_id: &str) -> InstalledApp {
        InstalledApp {
            app_id: app_id.into(),
            version: "1.0.0".into(),
            executable_path: "C:/fixture/app.exe".into(),
            install_directory: "C:/fixture".into(),
            package_kind: "zip".into(),
            installed_at: String::new(),
        }
    }

    fn register_cancellation(engine: &SharedEngine, app_id: &str) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        engine
            .0
            .cancellations
            .lock()
            .unwrap()
            .insert(app_id.into(), cancel.clone());
        cancel
    }

    async fn wait_for_force_close_prompt(engine: &SharedEngine) -> ForceClosePrompt {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(prompt) = engine.snapshot().force_close_prompt {
                    break prompt;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("force-close prompt should be published")
    }

    #[test]
    fn versions_and_launcher_contract() {
        assert!(newer("v1.2.0", "1.1.0"));
        assert!(!newer("v1.0.0", "2.0.0"));
        assert_eq!(
            launcher_definition().packages[0].asset_pattern,
            "MoaLauncher-*-Setup-x64.exe"
        );
    }
    #[test]
    fn legacy_helper_result_remains_visible_after_migration() {
        let result: UpdateResult = serde_json::from_str(r#"{"Success":true,"Message":"업데이트 완료","LogPath":"install.log","Version":"0.3.0","CompletedAt":"2026-11-15T00:00:00Z"}"#).unwrap();
        assert!(result.success);
        assert_eq!(result.message, "업데이트 완료");
        assert_eq!(result.version, "0.3.0");
    }

    #[test]
    fn engine_preserves_existing_view_preferences_and_interval() {
        let (engine, _runtime, path) = fixture();
        let snapshot = engine.snapshot();
        assert_eq!(
            snapshot.settings.favorite_app_ids,
            vec!["moaview".to_string(), "moavcs".to_string()]
        );
        assert!(!snapshot.settings.use_grid_view);
        assert_eq!(snapshot.settings.check_interval_minutes, 1440);

        engine
            .save_settings(serde_json::json!({ "periodicChecks": false }))
            .unwrap();
        let updated = engine.snapshot();
        assert!(!updated.settings.periodic_checks);
        assert_eq!(
            updated.settings.favorite_app_ids,
            vec!["moaview".to_string(), "moavcs".to_string()]
        );
        assert!(!updated.settings.use_grid_view);
        assert_eq!(updated.settings.check_interval_minutes, 1440);
        remove_fixture(&path);
    }

    #[test]
    fn private_auth_changes_invalidate_cached_artwork_generation() {
        let (engine, _runtime, path) = fixture();
        let definition = AppDefinition {
            id: "private-fixture".into(),
            name: "비공개 테스트".into(),
            description: String::new(),
            category: "테스트".into(),
            accent_color: "#000000".into(),
            monogram: "T".into(),
            icon_url: Some("icon.png".into()),
            cover_url: Some("cover.png".into()),
            repository: "owner/private-fixture".into(),
            is_private: true,
            packages: vec![],
        };
        engine.0.data.lock().unwrap().apps.push(AppView {
            definition,
            installed: None,
            release: Some(AppRelease {
                tag: "v1.0.0".into(),
                name: "v1.0.0".into(),
                notes: String::new(),
                page_url: String::new(),
                published_at: String::new(),
                assets: vec![],
                is_private: true,
            }),
            is_cached: true,
            is_launcher: false,
            busy: false,
            can_cancel: false,
            error: None,
            progress: None,
        });

        let before = engine.snapshot().auth_generation;
        engine.0.invalidate_private();
        let after = engine.snapshot();
        let private = after
            .apps
            .iter()
            .find(|app| app.definition.id == "private-fixture")
            .unwrap();
        assert!(after.auth_generation > before);
        assert!(private.release.is_none());
        assert!(!private.is_cached);
        assert!(!artwork_generation_is_current(
            after.auth_generation,
            before
        ));
        assert!(artwork_generation_is_current(
            after.auth_generation,
            after.auth_generation
        ));
        remove_fixture(&path);
    }

    #[test]
    fn catalog_saved_event_reaches_ui_event_queue() {
        let (engine, _runtime, path) = fixture();
        engine.0.event(EngineEvent::CatalogSaved);
        assert!(
            engine
                .drain_events()
                .iter()
                .any(|event| matches!(event, EngineEvent::CatalogSaved))
        );
        remove_fixture(&path);
    }

    #[test]
    fn force_close_confirmation_accepts_only_matching_approval() {
        let (engine, runtime, path) = fixture();
        let cancel = register_cancellation(&engine, "moaview");
        runtime.block_on(async {
            let confirm = force_close_confirmation(engine.0.clone(), cancel.clone());
            let task = tokio::spawn(async move { confirm(installed_fixture("moaview"), 2).await });
            let prompt = wait_for_force_close_prompt(&engine).await;
            assert_eq!(prompt.app_id, "moaview");
            assert_eq!(prompt.process_count, 2);
            assert!(!engine.respond_force_close("stale-request", true));
            assert_eq!(
                engine
                    .snapshot()
                    .force_close_prompt
                    .as_ref()
                    .map(|current| current.request_id.as_str()),
                Some(prompt.request_id.as_str())
            );
            assert!(engine.respond_force_close(&prompt.request_id, true));
            assert!(task.await.unwrap().unwrap());
            assert!(!cancel.load(Ordering::SeqCst));
            assert!(engine.snapshot().force_close_prompt.is_none());
        });
        remove_fixture(&path);
    }

    #[test]
    fn force_close_rejection_cancels_matching_install() {
        let (engine, runtime, path) = fixture();
        let cancel = register_cancellation(&engine, "moaview");
        runtime.block_on(async {
            let confirm = force_close_confirmation(engine.0.clone(), cancel.clone());
            let task = tokio::spawn(async move { confirm(installed_fixture("moaview"), 1).await });
            let prompt = wait_for_force_close_prompt(&engine).await;
            assert!(engine.respond_force_close(&prompt.request_id, false));
            assert!(!task.await.unwrap().unwrap());
            assert!(cancel.load(Ordering::SeqCst));
            assert!(engine.snapshot().force_close_prompt.is_none());
        });
        remove_fixture(&path);
    }

    #[test]
    fn dropping_polled_force_close_future_clears_prompt() {
        let (engine, runtime, path) = fixture();
        let cancel = register_cancellation(&engine, "moaview");
        runtime.block_on(async {
            let confirm = force_close_confirmation(engine.0.clone(), cancel);
            let task = tokio::spawn(async move { confirm(installed_fixture("moaview"), 1).await });
            let prompt = wait_for_force_close_prompt(&engine).await;
            task.abort();
            let _ = task.await;
            assert!(engine.snapshot().force_close_prompt.is_none());
            assert!(!engine.respond_force_close(&prompt.request_id, true));
        });
        remove_fixture(&path);
    }

    #[test]
    fn cancellation_clears_force_close_prompt_for_one_or_all_apps() {
        let (engine, runtime, path) = fixture();
        let cancel = register_cancellation(&engine, "moaview");
        runtime.block_on(async {
            let confirm = force_close_confirmation(engine.0.clone(), cancel.clone());
            let task = tokio::spawn(async move { confirm(installed_fixture("moaview"), 1).await });
            let _ = wait_for_force_close_prompt(&engine).await;
            engine.cancel(Some("moaview".into()));
            assert!(!task.await.unwrap().unwrap());
            assert!(cancel.load(Ordering::SeqCst));
            assert!(engine.snapshot().force_close_prompt.is_none());

            cancel.store(false, Ordering::SeqCst);
            let confirm = force_close_confirmation(engine.0.clone(), cancel.clone());
            let task = tokio::spawn(async move { confirm(installed_fixture("moaview"), 1).await });
            let _ = wait_for_force_close_prompt(&engine).await;
            engine.cancel(None);
            assert!(!task.await.unwrap().unwrap());
            assert!(cancel.load(Ordering::SeqCst));
            assert!(engine.snapshot().force_close_prompt.is_none());
        });
        remove_fixture(&path);
    }

    #[test]
    fn old_prompt_guard_does_not_clear_a_new_request() {
        let (engine, _runtime, path) = fixture();
        let (old_reply, _old_response) = oneshot::channel();
        let old_prompt = ForceClosePrompt {
            request_id: "old-request".into(),
            app_id: "moaview".into(),
            app_name: "MoaView".into(),
            process_count: 1,
        };
        assert!(
            engine
                .0
                .request_force_close(old_prompt.clone(), old_reply)
                .is_ok()
        );
        let old_guard = ForceClosePromptGuard {
            engine: Arc::downgrade(&engine.0),
            request_id: old_prompt.request_id.clone(),
        };
        assert!(engine.respond_force_close(&old_prompt.request_id, true));

        let (new_reply, _new_response) = oneshot::channel();
        let new_prompt = ForceClosePrompt {
            request_id: "new-request".into(),
            app_id: "moaview".into(),
            app_name: "MoaView".into(),
            process_count: 1,
        };
        assert!(
            engine
                .0
                .request_force_close(new_prompt.clone(), new_reply)
                .is_ok()
        );
        drop(old_guard);
        assert_eq!(
            engine
                .snapshot()
                .force_close_prompt
                .as_ref()
                .map(|prompt| prompt.request_id.as_str()),
            Some("new-request")
        );
        assert!(engine.respond_force_close(&new_prompt.request_id, true));
        remove_fixture(&path);
    }
}
