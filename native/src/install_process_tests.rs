//! Isolated end-to-end checks for update behavior around running applications.

use super::*;
use std::{
    future::Future,
    pin::Pin,
    process::{Child, Command},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
struct TestTree(Arc<TempRoot>);

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl TestTree {
    fn new() -> Self {
        let fixture_root = PathBuf::from(
            std::env::var_os("MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT")
                .expect("MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT가 필요합니다."),
        );
        let root = fixture_root.join("runs").join(format!(
            "process-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self(Arc::new(TempRoot(root)))
    }

    fn copy_process_fixture(&self, relative: &str) -> PathBuf {
        let source = std::env::var_os("MOA_PROCESS_FIXTURE_EXE")
            .expect("MOA_PROCESS_FIXTURE_EXE가 필요합니다.");
        let path = self.0.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(source, &path).unwrap();
        path
    }

    fn paths(&self) -> Paths {
        let paths = Paths {
            data_dir: self.0.0.join("data"),
            local_app_data: self.0.0.join("local"),
        };
        fs::create_dir_all(&paths.data_dir).unwrap();
        fs::create_dir_all(&paths.local_app_data).unwrap();
        paths
    }

    fn start(&self, executable: &Path, mode: &str, label: &str) -> OwnedChild {
        let ready = self.0.0.join(format!("{label}.ready"));
        let child = Command::new(executable)
            .args([mode, ready.to_str().unwrap()])
            .spawn()
            .unwrap();
        let mut child = OwnedChild(Some(child));
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if ready.is_file() {
                return child;
            }
            assert!(
                child.is_alive(),
                "fixture exited before creating its ready marker"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("fixture did not create its ready marker in time");
    }
}

struct OwnedChild(Option<Child>);

impl OwnedChild {
    fn is_alive(&mut self) -> bool {
        self.0.as_mut().unwrap().try_wait().unwrap().is_none()
    }

    fn wait(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.wait();
        }
    }

    fn stop_for_test(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
        }
        self.wait();
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.stop_for_test();
    }
}

#[derive(Clone)]
struct ZipUpdate {
    tree: TestTree,
    paths: Paths,
    app: AppDefinition,
    package: PackageDefinition,
    old_exe: PathBuf,
    old_bytes: Vec<u8>,
    archive: PathBuf,
}

impl ZipUpdate {
    fn new() -> Self {
        let tree = TestTree::new();
        let paths = tree.paths();
        let app = process_definition("zip");
        let package = app.packages[0].clone();
        let old_root = install_dir(&paths, &app, &package, Some("1.0.0")).unwrap();
        fs::create_dir_all(&old_root).unwrap();
        let old_exe = safe_relative(&old_root, &package.executable_path).unwrap();
        tree.copy_process_fixture("old/MoaFixture.exe");
        fs::copy(tree.0.0.join("old/MoaFixture.exe"), &old_exe).unwrap();
        let old_bytes = fs::read(&old_exe).unwrap();
        let installed = InstalledApp {
            app_id: app.id.clone(),
            version: "1.0.0".into(),
            executable_path: old_exe.to_string_lossy().into_owned(),
            install_directory: old_root.to_string_lossy().into_owned(),
            package_kind: package.kind.clone(),
            installed_at: "2026-01-01T00:00:00Z".into(),
        };
        crate::storage::save_installed(&paths, &installed).unwrap();
        let payload = tree.copy_process_fixture("payload/MoaFixture.exe");
        let archive = tree.0.0.join("update.zip");
        write_executable_zip(&archive, &payload);
        Self {
            tree,
            paths,
            app,
            package,
            old_exe,
            old_bytes,
            archive,
        }
    }

    async fn install(
        &self,
        cancel: Arc<AtomicBool>,
        confirm: ConfirmForceClose,
    ) -> Result<InstalledApp, String> {
        install_zip(
            &self.paths,
            &self.app,
            &process_release("1.1.0"),
            &self.package,
            &self.archive,
            process_callback(),
            cancel,
            &confirm,
        )
        .await
    }

    fn assert_old_install_untouched(&self) {
        assert_eq!(fs::read(&self.old_exe).unwrap(), self.old_bytes);
        let state = crate::storage::load_installed(&self.paths, &self.app.id)
            .unwrap()
            .expect("기존 설치 상태가 유지되어야 합니다.");
        assert_eq!(state.version, "1.0.0");
        let new_root = install_dir(&self.paths, &self.app, &self.package, Some("1.1.0")).unwrap();
        assert!(!new_root.exists());
        assert_no_staging_dirs(&self.paths.data_dir.join("apps").join(&self.app.id));
    }
}

fn write_executable_zip(archive_path: &Path, executable: &Path) {
    use zip::{ZipWriter, write::SimpleFileOptions};
    let file = fs::File::create(archive_path).unwrap();
    let mut zip = ZipWriter::new(file);
    zip.start_file(
        "MoaFixture.exe",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    let mut source = fs::File::open(executable).unwrap();
    std::io::copy(&mut source, &mut zip).unwrap();
    zip.finish().unwrap();
}

fn assert_no_staging_dirs(apps_root: &Path) {
    if !apps_root.exists() {
        return;
    }
    for entry in fs::read_dir(apps_root).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".staging-"),
            "staging folder leaked: {name}"
        );
    }
}

fn process_package(kind: &str) -> PackageDefinition {
    PackageDefinition {
        architecture: "x64".into(),
        kind: kind.into(),
        asset_pattern: "MoaFixture-*-x64.exe".into(),
        executable_path: "MoaFixture.exe".into(),
        install_directory: if kind == "zip" {
            None
        } else {
            Some(format!("Programs/Moa Fixture-{kind}"))
        },
    }
}

fn process_definition(kind: &str) -> AppDefinition {
    AppDefinition {
        id: format!("process-fixture-{kind}"),
        name: "process fixture".into(),
        description: String::new(),
        category: "테스트".into(),
        accent_color: "#000000".into(),
        monogram: "F".into(),
        icon_url: None,
        cover_url: None,
        repository: "owner/repo".into(),
        is_private: false,
        packages: vec![process_package(kind)],
    }
}

fn process_release(tag: &str) -> AppRelease {
    AppRelease {
        tag: tag.into(),
        name: tag.into(),
        notes: String::new(),
        page_url: String::new(),
        published_at: String::new(),
        assets: Vec::new(),
        is_private: false,
    }
}

fn process_callback() -> Arc<dyn Fn(InstallProgress) + Send + Sync> {
    Arc::new(|_| {})
}

fn immediate_confirmation(value: bool) -> ConfirmForceClose {
    Arc::new(move |_, _| Box::pin(async move { Ok(value) }))
}

fn pending_confirmation(polled: Arc<AtomicBool>, dropped: Arc<AtomicBool>) -> ConfirmForceClose {
    Arc::new(move |_, _| {
        Box::pin(PendingConfirmation {
            polled: polled.clone(),
            dropped: dropped.clone(),
        })
    })
}

struct PendingConfirmation {
    polled: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

impl Future for PendingConfirmation {
    type Output = Result<bool, String>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.polled.store(true, Ordering::SeqCst);
        Poll::Pending
    }
}

impl Drop for PendingConfirmation {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

async fn wait_for_flag(flag: &AtomicBool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !flag.load(Ordering::SeqCst) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        flag.load(Ordering::SeqCst),
        "확인 대기 상태에 도달하지 못했습니다."
    );
}

#[test]
#[ignore = "격리된 MOA_PROCESS_FIXTURE_EXE 및 MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT가 필요합니다."]
fn zip_update_closes_a_running_app_gracefully_without_confirmation() {
    let case = ZipUpdate::new();
    let mut child = case.tree.start(&case.old_exe, "close", "graceful");
    let confirmed = Arc::new(AtomicUsize::new(0));
    let confirm: ConfirmForceClose = {
        let confirmed = confirmed.clone();
        Arc::new(move |_, _| {
            confirmed.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(false) })
        })
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let installed = runtime
        .block_on(case.install(Arc::new(AtomicBool::new(false)), confirm))
        .unwrap();
    assert_eq!(installed.version, "1.1.0");
    assert_eq!(confirmed.load(Ordering::SeqCst), 0);
    child.wait();
}

#[test]
#[ignore = "격리된 MOA_PROCESS_FIXTURE_EXE 및 MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT가 필요합니다."]
fn zip_update_decline_keeps_old_files_state_and_running_app() {
    let case = ZipUpdate::new();
    let mut child = case.tree.start(&case.old_exe, "ignore-close", "decline");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(case.install(
        Arc::new(AtomicBool::new(false)),
        immediate_confirmation(false),
    ));
    assert!(result.is_err());
    assert!(child.is_alive());
    case.assert_old_install_untouched();
}

#[test]
#[ignore = "격리된 MOA_PROCESS_FIXTURE_EXE 및 MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT가 필요합니다."]
fn zip_update_approval_terminates_app_then_commits_new_version() {
    let case = ZipUpdate::new();
    let mut child = case.tree.start(&case.old_exe, "ignore-close", "approve");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let installed = runtime
        .block_on(case.install(
            Arc::new(AtomicBool::new(false)),
            immediate_confirmation(true),
        ))
        .unwrap();
    assert!(!child.is_alive());
    child.wait();
    assert_eq!(installed.version, "1.1.0");
    assert_eq!(
        crate::storage::load_installed(&case.paths, &case.app.id)
            .unwrap()
            .unwrap()
            .version,
        "1.1.0"
    );
    let new_exe = safe_relative(
        Path::new(&installed.install_directory),
        &case.package.executable_path,
    )
    .unwrap();
    assert_eq!(
        fs::read(new_exe).unwrap(),
        fs::read(case.tree.0.0.join("payload/MoaFixture.exe")).unwrap()
    );
    assert_no_staging_dirs(&case.paths.data_dir.join("apps").join(&case.app.id));
}

#[test]
#[ignore = "격리된 MOA_PROCESS_FIXTURE_EXE 및 MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT가 필요합니다."]
fn zip_update_cancel_while_confirmation_is_pending_preserves_the_running_install() {
    let case = ZipUpdate::new();
    let mut child = case.tree.start(&case.old_exe, "ignore-close", "cancel");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let polled = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let task = runtime.spawn({
        let case = case.clone();
        let cancel = cancel.clone();
        let confirm = pending_confirmation(polled.clone(), dropped.clone());
        async move { case.install(cancel, confirm).await }
    });
    runtime.block_on(wait_for_flag(&polled));
    cancel.store(true, Ordering::SeqCst);
    assert!(runtime.block_on(task).unwrap().is_err());
    assert!(dropped.load(Ordering::SeqCst));
    assert!(child.is_alive());
    case.assert_old_install_untouched();
}

#[test]
#[ignore = "격리된 MOA_PROCESS_FIXTURE_EXE 및 MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT가 필요합니다."]
fn zip_update_continues_when_app_exits_while_confirmation_is_pending() {
    let case = ZipUpdate::new();
    let mut child = case
        .tree
        .start(&case.old_exe, "ignore-close", "manual-exit");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let polled = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let task = runtime.spawn({
        let case = case.clone();
        let confirm = pending_confirmation(polled.clone(), dropped.clone());
        async move {
            case.install(Arc::new(AtomicBool::new(false)), confirm)
                .await
        }
    });
    runtime.block_on(wait_for_flag(&polled));
    child.stop_for_test();
    let installed = runtime.block_on(task).unwrap().unwrap();
    assert!(
        dropped.load(Ordering::SeqCst),
        "manual exit should drop the pending confirmation future"
    );
    assert_eq!(installed.version, "1.1.0");
    assert_eq!(
        crate::storage::load_installed(&case.paths, &case.app.id)
            .unwrap()
            .unwrap()
            .version,
        "1.1.0"
    );
}

fn prepare_installer_update(
    tree: &TestTree,
    paths: &Paths,
    kind: &str,
) -> (AppDefinition, PackageDefinition, PathBuf) {
    let app = process_definition(kind);
    let package = app.packages[0].clone();
    let root = install_dir(paths, &app, &package, None).unwrap();
    fs::create_dir_all(&root).unwrap();
    let executable = safe_relative(&root, &package.executable_path).unwrap();
    let fixture = tree.copy_process_fixture(&format!("old-{kind}/MoaFixture.exe"));
    fs::copy(&fixture, &executable).unwrap();
    crate::storage::save_installed(
        paths,
        &InstalledApp {
            app_id: app.id.clone(),
            version: "1.0.0".into(),
            executable_path: executable.to_string_lossy().into_owned(),
            install_directory: root.to_string_lossy().into_owned(),
            package_kind: kind.into(),
            installed_at: "2026-01-01T00:00:00Z".into(),
        },
    )
    .unwrap();
    (app, package, executable)
}

fn installer_fixture(kind: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("MOALAUNCHER_RUST_INSTALL_FIXTURE_ROOT").unwrap())
        .join("installers")
        .join(format!("setup-{kind}-1.1.0.exe"))
}

#[test]
#[ignore = "실제 NSIS/Inno installer 및 process fixture 환경 변수가 필요합니다."]
fn nsis_and_inno_updates_require_approval_before_terminating_resident_apps() {
    for kind in ["nsis", "inno"] {
        let tree = TestTree::new();
        let paths = tree.paths();
        let (app, package, executable) = prepare_installer_update(&tree, &paths, kind);
        let old_bytes = fs::read(&executable).unwrap();
        let mut child = tree.start(&executable, "ignore-close", kind);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(install_executable(
            &paths,
            &app,
            &process_release("1.1.0"),
            &package,
            &installer_fixture(kind),
            process_callback().as_ref(),
            &AtomicBool::new(false),
            &immediate_confirmation(false),
        ));
        assert!(result.is_err());
        assert!(child.is_alive());
        assert_eq!(fs::read(&executable).unwrap(), old_bytes);
        assert_eq!(
            crate::storage::load_installed(&paths, &app.id)
                .unwrap()
                .unwrap()
                .version,
            "1.0.0"
        );
    }
}

#[test]
#[ignore = "실제 NSIS/Inno installer 및 process fixture 환경 변수가 필요합니다."]
fn nsis_and_inno_updates_terminate_resident_apps_after_approval() {
    for kind in ["nsis", "inno"] {
        let tree = TestTree::new();
        let paths = tree.paths();
        let (app, package, executable) = prepare_installer_update(&tree, &paths, kind);
        let mut child = tree.start(&executable, "ignore-close", kind);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let installed = runtime
            .block_on(install_executable(
                &paths,
                &app,
                &process_release("1.1.0"),
                &package,
                &installer_fixture(kind),
                process_callback().as_ref(),
                &AtomicBool::new(false),
                &immediate_confirmation(true),
            ))
            .unwrap();
        assert!(!child.is_alive());
        child.wait();
        assert_eq!(installed.version, "1.1.0");
        assert_eq!(
            crate::storage::load_installed(&paths, &app.id)
                .unwrap()
                .unwrap()
                .version,
            "1.1.0"
        );
        assert_eq!(read_product_version(&executable).as_deref(), Some("1.1.0"));
    }
}
