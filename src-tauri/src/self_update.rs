//! Isolated helper handoff for installing the launcher's official Inno package.
use crate::{
    download::{download_asset, MAX_PACKAGE_BYTES},
    install::{normalize_version, read_product_version, wildcard_match},
    models::{AppRelease, InstallProgress},
    storage::Paths,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

const REPOSITORY: &str = "hebory/moalauncher";
const PATTERN: &str = "MoaLauncher-*-Setup-x64.exe";
const EXE: &str = "MoaLauncher.exe";
const HELPER_ARG: &str = "--apply-self-update";
const MAX_FILES: usize = 4096;
const MAX_TOTAL: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct PreparedUpdate {
    pub installer_path: PathBuf,
    pub version: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    target: String,
    data: String,
    installer: String,
    sha256: String,
    version: String,
    parent_pid: u32,
    parent_started: u64,
    parent_exe: String,
    background: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultFile {
    success: bool,
    message: String,
    log_path: String,
    version: String,
    completed_at: String,
}

fn cb(stage: &str, bytes: u64, total: Option<u64>, progress: &dyn Fn(InstallProgress)) {
    progress(InstallProgress {
        app_id: "moa-launcher".into(),
        stage: stage.into(),
        bytes_received: bytes,
        total_bytes: total,
    });
}
fn cancelled(c: &AtomicBool) -> Result<(), String> {
    if c.load(Ordering::Relaxed) {
        Err("작업이 취소되었습니다.".into())
    } else {
        Ok(())
    }
}
fn matches(pattern: &str, name: &str) -> bool {
    wildcard_match(pattern, name)
}
fn pick_asset(release: &AppRelease) -> Result<&crate::models::ReleaseAsset, String> {
    let a: Vec<_> = release
        .assets
        .iter()
        .filter(|a| matches(PATTERN, &a.name))
        .collect();
    match a.as_slice() {
        [x] => Ok(*x),
        [] => Err("런처 릴리스에 설치 파일이 없습니다.".into()),
        _ => Err("런처 릴리스에 일치하는 설치 파일이 여러 개 있습니다.".into()),
    }
}

pub async fn prepare(
    client: &reqwest::Client,
    paths: &Paths,
    release: &AppRelease,
    cancel: Arc<AtomicBool>,
    progress: Arc<dyn Fn(InstallProgress) + Send + Sync>,
) -> Result<PreparedUpdate, String> {
    if release.is_private || !official_page(&release.page_url, &release.tag) {
        return Err("런처 릴리스 페이지가 공식 공개 저장소와 일치하지 않습니다.".into());
    }
    let target = current_target()?;
    let current = read_product_version(&target.join(EXE))
        .ok_or("게시된 MoaLauncher.exe로 실행한 경우에만 자체 업데이트할 수 있습니다.")?;
    if !is_newer(&release.tag, &current)? {
        return Err("현재 버전보다 새로운 런처 릴리스가 아닙니다.".into());
    }
    let asset = pick_asset(release)?;
    if !crate::github::validate_asset_url(REPOSITORY, &asset.download_url) {
        return Err("런처 설치 파일이 공식 GitHub 저장소에서 오지 않았습니다.".into());
    }
    let job = paths
        .data_dir
        .join("self-update")
        .join(uuid::Uuid::new_v4().simple().to_string());
    fs::create_dir_all(&job).map_err(|_| "자체 업데이트 임시 폴더를 만들지 못했습니다.")?;
    let dest = job.join(&asset.name);
    let result = async {
        cb(
            "런처 업데이트 다운로드 준비",
            0,
            Some(asset.size),
            progress.as_ref(),
        );
        let got = download_asset(
            client,
            asset,
            REPOSITORY,
            false,
            None,
            &dest,
            MAX_PACKAGE_BYTES,
            "moa-launcher",
            &cancel,
            progress.as_ref(),
        )
        .await?;
        if asset.size > 0 && got.bytes != asset.size {
            return Err("런처 설치 파일 크기가 GitHub 릴리스 정보와 다릅니다.".into());
        }
        let expected = if let Some(hash) = &asset.sha256 {
            hash.to_ascii_lowercase()
        } else {
            let checksum = release
                .assets
                .iter()
                .find(|a| {
                    a.name
                        .eq_ignore_ascii_case(&(asset.name.clone() + ".sha256"))
                })
                .or_else(|| {
                    release
                        .assets
                        .iter()
                        .find(|a| a.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
                })
                .ok_or("런처 릴리스에 SHA-256 체크섬이 없습니다.")?;
            let file = job.join("checksum.txt");
            let r = download_asset(
                client,
                checksum,
                REPOSITORY,
                false,
                None,
                &file,
                1024 * 1024,
                "moa-launcher",
                &cancel,
                progress.as_ref(),
            )
            .await?;
            let text = fs::read_to_string(&file).map_err(|_| "체크섬 파일을 읽지 못했습니다.")?;
            let sum = crate::download::checksum_matches(&text, &asset.name)
                .ok_or("런처 체크섬 파일의 항목이 올바르지 않습니다.")?;
            let _ = fs::remove_file(file);
            let _ = r;
            sum
        };
        if expected.len() != 64
            || !expected.bytes().all(|x| x.is_ascii_hexdigit())
            || !constant_eq(&got.sha256, &expected)
        {
            return Err("런처 설치 파일의 SHA-256 검증에 실패했습니다.".into());
        }
        cancelled(&cancel)?;
        cb(
            "런처 업데이트 다운로드 완료",
            got.bytes,
            Some(got.bytes),
            progress.as_ref(),
        );
        Ok(PreparedUpdate {
            installer_path: dest.clone(),
            version: normalize_tag(&release.tag)?,
            sha256: got.sha256,
        })
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_dir_all(&job);
    }
    result
}

pub fn handoff(paths: &Paths, update: &PreparedUpdate, background: bool) -> Result<(), String> {
    let target = current_target()?;
    validate_target(&target)?;
    reject_reparse(&paths.data_dir)?;
    reject_reparse(
        update
            .installer_path
            .parent()
            .ok_or("업데이트 파일 경로가 올바르지 않습니다.")?,
    )?;
    let data = canonical(&paths.data_dir)?;
    let job = canonical(
        update
            .installer_path
            .parent()
            .ok_or("업데이트 파일 경로가 올바르지 않습니다.")?,
    )?;
    reject_reparse(&data.join("self-update"))?;
    let expected_root = canonical(&data.join("self-update"))?;
    if within(&target, &data) || within(&data, &target) {
        return Err("런처 설치 폴더와 데이터 폴더가 겹칩니다.".into());
    }
    if job.parent() != Some(expected_root.as_path())
        || !uuid::Uuid::parse_str(job.file_name().and_then(|x| x.to_str()).unwrap_or("")).is_ok()
    {
        return Err("자체 업데이트 임시 폴더가 예상 위치와 다릅니다.".into());
    }
    reject_reparse(&target)?;
    reject_reparse(&data)?;
    reject_reparse(&job)?;
    reject_reparse(&update.installer_path)?;
    if !update.installer_path.is_file()
        || !installer_name(
            update
                .installer_path
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or(""),
        )
    {
        return Err("런처 설치 파일 경로가 올바르지 않습니다.".into());
    }
    if !constant_eq(&hash(&update.installer_path)?, &update.sha256) {
        return Err("handoff 전 런처 설치 파일 SHA-256 검증에 실패했습니다.".into());
    }
    let current =
        std::env::current_exe().map_err(|_| "현재 런처 실행 파일을 확인하지 못했습니다.")?;
    let pid = std::process::id();
    let started = process_start(pid).ok_or("현재 런처 프로세스 정보를 확인하지 못했습니다.")?;
    let plan = Plan {
        target: target.to_string_lossy().into_owned(),
        data: data.to_string_lossy().into_owned(),
        installer: update.installer_path.to_string_lossy().into_owned(),
        sha256: update.sha256.to_ascii_lowercase(),
        version: update.version.clone(),
        parent_pid: pid,
        parent_started: started,
        parent_exe: current.to_string_lossy().into_owned(),
        background,
    };
    let plan_path = job.join("plan.json");
    let ready = job.join("helper.ready");
    let runner = job.join("runner");
    if runner.exists() || ready.exists() || plan_path.exists() {
        return Err("자체 업데이트 helper 파일이 이미 있습니다.".into());
    }
    write_json_atomic(&plan_path, &plan)?;
    fs::create_dir(&runner).map_err(|_| {
        let _ = fs::remove_file(&plan_path);
        "helper 폴더를 만들지 못했습니다."
    })?;
    if fs::copy(&current, runner.join(EXE)).is_err() {
        let _ = fs::remove_dir_all(&runner);
        let _ = fs::remove_file(&plan_path);
        return Err("자체 업데이트 helper를 준비하지 못했습니다.".into());
    }
    let mut child = match Command::new(runner.join(EXE))
        .arg(HELPER_ARG)
        .arg(&plan_path)
        .current_dir(&runner)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags_if_windows()
        .spawn()
    {
        Ok(p) => p,
        Err(_) => {
            let _ = fs::remove_dir_all(&runner);
            let _ = fs::remove_file(plan_path);
            return Err("자체 업데이트 helper를 시작하지 못했습니다.".into());
        }
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if ready.is_file() {
            return Ok(());
        }
        if child
            .try_wait()
            .map_err(|_| "자체 업데이트 helper 상태를 확인하지 못했습니다.")?
            .is_some()
        {
            let _ = fs::remove_dir_all(&runner);
            let _ = fs::remove_file(plan_path);
            return Err("자체 업데이트 helper가 준비되지 않았습니다.".into());
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_dir_all(&runner);
            let _ = fs::remove_file(plan_path);
            return Err("자체 업데이트 helper 준비 응답 시간이 초과되었습니다.".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub fn run_helper(plan_path: &Path) -> Result<(), String> {
    reject_reparse(plan_path)?;
    let plan_path = canonical(plan_path)?;
    reject_reparse(&plan_path)?;
    let plan: Plan = serde_json::from_slice(
        &fs::read(&plan_path).map_err(|_| "자체 업데이트 계획을 읽지 못했습니다.")?,
    )
    .map_err(|_| "자체 업데이트 계획 형식이 올바르지 않습니다.")?;
    reject_reparse(Path::new(&plan.data))?;
    reject_reparse(Path::new(&plan.target))?;
    reject_reparse(Path::new(&plan.installer))?;
    let data = canonical(Path::new(&plan.data))?;
    let target = canonical(Path::new(&plan.target))?;
    let installer = canonical(Path::new(&plan.installer))?;
    let job = plan_path.parent().ok_or("작업 폴더가 없습니다.")?;
    let runner = canonical(
        &std::env::current_exe()
            .map_err(|_| "helper 경로를 확인하지 못했습니다.")?
            .parent()
            .ok_or("helper 폴더가 없습니다.")?,
    )?;
    if job.parent() != Some(canonical(&data.join("self-update"))?.as_path())
        || runner != job.join("runner")
        || installer.parent() != Some(job)
        || !installer_name(installer.file_name().and_then(|x| x.to_str()).unwrap_or(""))
    {
        return Err("helper 경로 검증에 실패했습니다.".into());
    }
    validate_target(&target)?;
    reject_reparse(&data)?;
    reject_reparse(&target)?;
    reject_reparse(&installer)?;
    if !constant_eq(&hash(&installer)?, &plan.sha256) {
        return Err("helper 실행 후 설치 파일의 SHA-256 검증에 실패했습니다.".into());
    }
    validate_parent(&plan)?;
    let ready = job.join("helper.ready");
    write_new(&ready, std::process::id().to_string().as_bytes())?;
    if !wait_parent(&plan) {
        write_result(
            &data,
            false,
            "런처가 종료되지 않아 업데이트를 시작하지 않았습니다.",
            &plan.version,
            "",
        );
        return Err("런처 종료 대기 시간이 초과되었습니다.".into());
    }
    // Verify the staged package a second time after the original launcher has exited.
    if !constant_eq(&hash(&installer)?, &plan.sha256) {
        write_result(
            &data,
            false,
            "런처 설치 파일 재검증에 실패했습니다. 기존 런처를 다시 시작합니다.",
            &plan.version,
            "",
        );
        restart(&plan, &target)?;
        return Err("설치 파일 재검증에 실패했습니다.".into());
    }
    let backup = runner.join("backup");
    if fs::create_dir(&backup).is_err() {
        write_result(
            &data,
            false,
            "이전 런처 백업을 만들지 못했습니다. 기존 런처를 다시 시작합니다.",
            &plan.version,
            "",
        );
        restart(&plan, &target)?;
        return Err("이전 런처를 백업하지 못했습니다.".into());
    }
    let manifest = match copy_tree(&target, &backup) {
        Ok(v) => v,
        Err(e) => {
            write_result(
                &data,
                false,
                "이전 런처를 백업하지 못했습니다. 기존 런처를 다시 시작합니다.",
                &plan.version,
                "",
            );
            restart(&plan, &target)?;
            return Err(e);
        }
    };
    let log = job.join("install.log");
    let status = Command::new(&installer)
        .args([
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/SP-",
            "/NOCLOSEAPPLICATIONS",
            "/NORESTARTAPPLICATIONS",
        ])
        .arg(format!("/DIR={}", target.display()))
        .arg(format!("/LOG={}", log.display()))
        .current_dir(job)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags_if_windows()
        .status();
    let installed = matches!(status,Ok(s) if s.success())
        && read_product_version(&target.join(EXE))
            .is_some_and(|v| normalize_version(&v) == normalize_version(&plan.version));
    if !installed {
        let restore = restore_tree(&backup, &target, &manifest);
        if restore.is_err() {
            write_result(
                &data,
                false,
                "업데이트 실패 후 이전 런처를 복구하지 못했습니다.",
                &plan.version,
                &log.to_string_lossy(),
            );
            return Err("업데이트 및 이전 런처 복구에 실패했습니다.".into());
        }
        write_result(
            &data,
            false,
            "업데이트 실패. 이전 런처 파일을 복구했습니다.",
            &plan.version,
            &log.to_string_lossy(),
        );
        restart(&plan, &target)?;
        return Err("런처 업데이트에 실패해 이전 버전으로 복구했습니다.".into());
    }
    write_result(
        &data,
        true,
        "Moa Launcher 업데이트를 완료했습니다.",
        &plan.version,
        &log.to_string_lossy(),
    );
    restart(&plan, &target)
}

fn normalize_tag(tag: &str) -> Result<String, String> {
    let s = tag.trim().trim_start_matches(['v', 'V']);
    let core = s.split('+').next().unwrap_or("");
    semver::Version::parse(core).map_err(|_| "런처 릴리스 버전 형식이 올바르지 않습니다.")?;
    Ok(s.to_owned())
}
fn is_newer(a: &str, b: &str) -> Result<bool, String> {
    let a = semver::Version::parse(
        a.trim()
            .trim_start_matches(['v', 'V'])
            .split('+')
            .next()
            .unwrap_or(""),
    )
    .map_err(|_| "런처 릴리스 버전 형식이 올바르지 않습니다.")?;
    let b = semver::Version::parse(
        b.trim()
            .trim_start_matches(['v', 'V'])
            .split('+')
            .next()
            .unwrap_or(""),
    )
    .map_err(|_| "현재 런처 버전 형식을 확인하지 못했습니다.")?;
    Ok(a > b)
}
fn official_page(url: &str, tag: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https"
        && u.host_str() == Some("github.com")
        && u.port().is_none()
        && u.username().is_empty()
        && u.password().is_none()
        && u.query().is_none()
        && u.fragment().is_none()
        && u.path() == format!("/{REPOSITORY}/releases/tag/{tag}")
}
fn installer_name(n: &str) -> bool {
    n.starts_with("MoaLauncher-")
        && n.ends_with("-Setup-x64.exe")
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'+' || b == b'-')
}
fn current_target() -> Result<PathBuf, String> {
    let p = std::env::current_exe().map_err(|_| "현재 런처 경로를 확인하지 못했습니다.")?;
    if p.file_name().and_then(|x| x.to_str()) != Some(EXE) {
        return Err("게시된 MoaLauncher.exe로 실행한 경우에만 자체 업데이트할 수 있습니다.".into());
    }
    reject_reparse(&p)?;
    canonical(p.parent().ok_or("런처 폴더가 없습니다.")?)
}
fn validate_target(p: &Path) -> Result<(), String> {
    if !p.join(EXE).is_file() {
        return Err("자체 업데이트 대상에 MoaLauncher.exe가 없습니다.".into());
    }
    Ok(())
}
fn canonical(p: &Path) -> Result<PathBuf, String> {
    if !p.is_absolute() {
        return Err("절대 경로가 필요합니다.".into());
    }
    let x = fs::canonicalize(p).map_err(|_| "경로를 확인하지 못했습니다.")?;
    if x.parent().is_none() {
        return Err("파일 시스템 루트는 사용할 수 없습니다.".into());
    }
    Ok(x)
}
fn within(root: &Path, path: &Path) -> bool {
    let r = root
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_uppercase();
    let p = path.to_string_lossy().to_ascii_uppercase();
    p == r || p.starts_with(&(r.clone() + "\\")) || p.starts_with(&(r + "/"))
}
fn reject_reparse(p: &Path) -> Result<(), String> {
    if !p.is_absolute() {
        return Err("절대 경로가 필요합니다.".into());
    }
    let mut cur = PathBuf::new();
    for c in p.components() {
        if matches!(c, std::path::Component::ParentDir) {
            return Err("경로에 상위 폴더 이동이 포함되어 있습니다.".into());
        }
        cur.push(c);
        if let Ok(m) = fs::symlink_metadata(&cur) {
            if m.file_type().is_symlink() {
                return Err("자체 업데이트 경로에 링크가 포함되어 있습니다.".into());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                GetFileAttributesW, FILE_ATTRIBUTE_REPARSE_POINT, INVALID_FILE_ATTRIBUTES,
            };
            let wide: Vec<u16> = cur.as_os_str().encode_wide().chain(Some(0)).collect();
            let attrs = unsafe { GetFileAttributesW(wide.as_ptr()) };
            if attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err("자체 업데이트 경로에 재분석 지점이 포함되어 있습니다.".into());
            }
        }
    }
    Ok(())
}
fn hash(p: &Path) -> Result<String, String> {
    let mut f = fs::File::open(p).map_err(|_| "SHA-256 검증 파일을 열지 못했습니다.")?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut Sha256Writer(&mut h))
        .map_err(|_| "SHA-256 검증 파일을 읽지 못했습니다.")?;
    Ok(hex::encode(h.finalize()))
}
struct Sha256Writer<'a>(&'a mut Sha256);
impl std::io::Write for Sha256Writer<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.update(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn constant_eq(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (hex::decode(a), hex::decode(b)) else {
        return false;
    };
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |x, (l, r)| x | (l ^ r)) == 0
}
fn write_json_atomic<T: Serialize>(p: &Path, v: &T) -> Result<(), String> {
    let t = p.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec(v).map_err(|_| "업데이트 계획을 만들지 못했습니다.")?;
    write_new(&t, &bytes)?;
    #[cfg(windows)]
    if p.exists() {
        fs::remove_file(p).map_err(|_| "기존 업데이트 결과 파일을 교체하지 못했습니다.")?;
    }
    fs::rename(t, p)
        .map_err(|_| "업데이트 계획을 저장하지 못했습니다.")
        .map_err(str::to_owned)
}
fn write_new(p: &Path, b: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)
        .map_err(|_| "업데이트 파일을 안전하게 만들지 못했습니다.")?;
    f.write_all(b)
        .and_then(|_| f.sync_all())
        .map_err(|_| "업데이트 파일을 저장하지 못했습니다.")
        .map_err(str::to_owned)
}
fn process_start(pid: u32) -> Option<u64> {
    let mut s = sysinfo::System::new();
    s.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    s.process(sysinfo::Pid::from_u32(pid))
        .map(|p| p.start_time())
}
fn validate_parent(p: &Plan) -> Result<(), String> {
    let mut s = sysinfo::System::new();
    s.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let parent = s
        .process(sysinfo::Pid::from_u32(p.parent_pid))
        .ok_or("handoff 런처 프로세스를 확인할 수 없습니다.")?;
    if parent.start_time() != p.parent_started
        || parent
            .exe()
            .is_none_or(|e| canonical(e).ok().as_deref() != Some(Path::new(&p.parent_exe)))
    {
        return Err("handoff 런처 프로세스가 일치하지 않습니다.".into());
    }
    Ok(())
}
fn wait_parent(p: &Plan) -> bool {
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < until {
        let mut s = sysinfo::System::new();
        s.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        if s.process(sysinfo::Pid::from_u32(p.parent_pid))
            .is_none_or(|x| x.start_time() != p.parent_started)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(150));
    }
    false
}
fn copy_tree(src: &Path, dst: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let mut total = 0u64;
    copy_walk(src, dst, src, &mut files, &mut total)?;
    if files.len() > MAX_FILES {
        return Err("런처 백업 파일 수가 제한을 넘었습니다.".into());
    }
    Ok(files)
}
fn copy_walk(
    root: &Path,
    src: &Path,
    dst: &Path,
    files: &mut Vec<String>,
    total: &mut u64,
) -> Result<(), String> {
    for ent in fs::read_dir(src).map_err(|_| "런처 파일을 백업하지 못했습니다.")? {
        let e = ent.map_err(|_| "런처 파일을 읽지 못했습니다.")?;
        let p = e.path();
        reject_reparse(&p)?;
        let rel = p
            .strip_prefix(root)
            .map_err(|_| "백업 경로가 잘못되었습니다.")?;
        let out = dst.join(rel);
        let m = e
            .metadata()
            .map_err(|_| "런처 파일 정보를 읽지 못했습니다.")?;
        if m.is_dir() {
            fs::create_dir_all(&out).map_err(|_| "백업 폴더를 만들지 못했습니다.")?;
            copy_walk(root, &p, dst, files, total)?;
        } else if m.is_file() {
            *total = total
                .checked_add(m.len())
                .ok_or("런처 백업 크기가 잘못되었습니다.")?;
            if *total > MAX_TOTAL || files.len() >= MAX_FILES {
                return Err("런처 백업 크기 또는 파일 수가 제한을 넘었습니다.".into());
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|_| "백업 폴더를 만들지 못했습니다.")?;
            }
            fs::copy(&p, &out).map_err(|_| "런처 파일을 백업하지 못했습니다.")?;
            files.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}
fn restore_tree(backup: &Path, target: &Path, manifest: &[String]) -> Result<(), String> {
    let allowed: std::collections::HashSet<_> = manifest.iter().cloned().collect();
    let mut current = Vec::new();
    collect_files(target, target, &mut current)?;
    for rel in current {
        if !allowed.contains(&rel) {
            let p = target.join(&rel);
            reject_reparse(&p)?;
            let _ = fs::remove_file(p);
        }
    }
    for rel in manifest {
        let src = backup.join(rel);
        let dest = target.join(rel);
        reject_reparse(&src)?;
        reject_reparse(&dest)?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|_| "이전 런처 폴더를 복구하지 못했습니다.")?;
        }
        fs::copy(src, dest).map_err(|_| "이전 런처 파일을 복구하지 못했습니다.")?;
    }
    Ok(())
}
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    for e in fs::read_dir(dir).map_err(|_| "런처 폴더를 확인하지 못했습니다.")? {
        let p = e.map_err(|_| "런처 파일 정보를 읽지 못했습니다.")?.path();
        reject_reparse(&p)?;
        if p.is_dir() {
            collect_files(root, &p, out)?;
        } else if p.is_file() {
            out.push(
                p.strip_prefix(root)
                    .map_err(|_| "런처 경로가 잘못되었습니다.")?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}
fn write_result(data: &Path, success: bool, message: &str, version: &str, log: &str) {
    let r = ResultFile {
        success,
        message: message.into(),
        log_path: log.into(),
        version: version.into(),
        completed_at: chrono::Utc::now().to_rfc3339(),
    };
    let _ = write_json_atomic(&data.join("self-update-result.json"), &r);
}
fn restart(p: &Plan, target: &Path) -> Result<(), String> {
    let mut c = Command::new(target.join(EXE));
    c.arg("--data-dir").arg(&p.data);
    if p.background {
        c.arg("--background");
    }
    c.current_dir(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags_if_windows()
        .spawn()
        .map_err(|_| "업데이트 후 런처를 다시 시작하지 못했습니다.")?;
    Ok(())
}
trait HiddenProcess {
    fn creation_flags_if_windows(&mut self) -> &mut Self;
}
impl HiddenProcess for Command {
    fn creation_flags_if_windows(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(0x08000000);
        }
        self
    }
}
