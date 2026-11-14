//! Release selection, safe staging and application installation.

use crate::download::{checksum_matches, download_asset, MAX_PACKAGE_BYTES};
use crate::{
    models::{
        AppDefinition, AppRelease, InstallProgress, InstalledApp, PackageDefinition, ReleaseAsset,
    },
    storage::Paths,
};
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use sysinfo::{ProcessesToUpdate, System};

const MAX_EXPANDED: u64 = 5 * 1024 * 1024 * 1024;
const MAX_ENTRY: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 100_000;
fn report(
    app: &AppDefinition,
    stage: &str,
    bytes_received: u64,
    total_bytes: Option<u64>,
    cb: &dyn Fn(InstallProgress),
) {
    cb(InstallProgress {
        app_id: app.id.clone(),
        stage: stage.to_owned(),
        bytes_received,
        total_bytes,
    });
}
fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("작업이 취소되었습니다.".into())
    } else {
        Ok(())
    }
}
fn segment(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 180
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', ':'])
        || value.chars().any(char::is_control)
    {
        Err("앱 ID 또는 버전 경로가 올바르지 않습니다.".into())
    } else {
        Ok(value)
    }
}
fn safe_relative(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.len() > 1024
        || relative.contains('\0')
        || relative.starts_with(['/', '\\'])
        || relative.contains(':')
    {
        return Err("패키지 실행 파일 경로가 올바르지 않습니다.".into());
    }
    let rel = Path::new(relative.replace('\\', "/").as_str()).to_path_buf();
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("패키지 경로가 설치 폴더를 벗어납니다.".into());
    }
    if relative
        .replace('\\', "/")
        .split('/')
        .any(|part| part.ends_with([' ', '.']) || is_reserved_windows_name(part))
    {
        return Err("패키지 경로에 Windows에서 사용할 수 없는 이름이 있습니다.".into());
    }
    Ok(root.join(rel))
}
fn version_tag(tag: &str) -> Result<String, String> {
    let value = tag.trim().trim_start_matches(['v', 'V']);
    let core = value.split('+').next().unwrap_or("");
    let parsed =
        semver::Version::parse(core).map_err(|_| "릴리스 버전 형식이 올바르지 않습니다.")?;
    if parsed.build.as_str().len() > 128 {
        return Err("릴리스 버전이 너무 깁니다.".into());
    }
    Ok(value.to_owned())
}
fn package_for(app: &AppDefinition) -> Result<&PackageDefinition, String> {
    let arch = if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86"
    };
    app.packages
        .iter()
        .find(|p| p.architecture.eq_ignore_ascii_case(arch))
        .ok_or_else(|| format!("이 앱은 {arch} 프로세서를 지원하지 않습니다."))
}
pub(crate) fn wildcard_match(pattern: &str, name: &str) -> bool {
    // Small glob matcher: only `*` and `?`, case-insensitive, no path semantics.
    let (p, n) = (pattern.to_ascii_lowercase(), name.to_ascii_lowercase());
    let (mut pi, mut ni, mut star, mut retry) = (0usize, 0usize, None, 0usize);
    let (pb, nb) = (p.as_bytes(), n.as_bytes());
    while ni < nb.len() {
        if pi < pb.len() && (pb[pi] == b'?' || pb[pi] == nb[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < pb.len() && pb[pi] == b'*' {
            star = Some(pi);
            pi += 1;
            retry = ni;
        } else if let Some(s) = star {
            retry += 1;
            ni = retry;
            pi = s + 1;
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == b'*' {
        pi += 1;
    }
    pi == pb.len()
}
fn select_asset<'a>(
    release: &'a AppRelease,
    package: &PackageDefinition,
) -> Result<&'a ReleaseAsset, String> {
    let found: Vec<_> = release
        .assets
        .iter()
        .filter(|a| wildcard_match(&package.asset_pattern, &a.name))
        .collect();
    match found.as_slice() {
        [asset] => Ok(*asset),
        [] => Err(format!(
            "릴리스에서 '{}' 파일을 찾을 수 없습니다.",
            package.asset_pattern
        )),
        _ => Err(format!(
            "릴리스에서 '{}'에 맞는 파일이 여러 개 발견되었습니다.",
            package.asset_pattern
        )),
    }
}
fn is_installer(package: &PackageDefinition) -> bool {
    package.kind.eq_ignore_ascii_case("inno") || package.kind.eq_ignore_ascii_case("nsis")
}
fn install_dir(
    paths: &Paths,
    app: &AppDefinition,
    package: &PackageDefinition,
    version: Option<&str>,
) -> Result<PathBuf, String> {
    let id = segment(&app.id)?;
    if is_installer(package) {
        let relative = package
            .install_directory
            .as_deref()
            .ok_or("설치 패키지 설치 폴더가 지정되지 않았습니다.")?;
        let path = crate::storage::safe_relative(&paths.local_app_data, relative)?;
        Ok(path)
    } else {
        let v = version.ok_or("설치 버전을 확인할 수 없습니다.")?;
        Ok(paths.data_dir.join("apps").join(id).join(segment(v)?))
    }
}
fn state_path(paths: &Paths, app: &AppDefinition) -> Result<PathBuf, String> {
    Ok(paths
        .data_dir
        .join("installed")
        .join(format!("{}.json", segment(&app.id)?)))
}
fn path_eq(a: &Path, b: &Path) -> bool {
    crate::storage::normalize_path(a) == crate::storage::normalize_path(b)
}
fn sha_valid(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn find_installed(paths: &Paths, app: &AppDefinition) -> Result<Option<InstalledApp>, String> {
    let package = package_for(app)?;
    let state_path = state_path(paths, app)?;
    let state: Option<InstalledApp> = if state_path.is_file() {
        crate::storage::load_installed(paths, &app.id)?
    } else {
        None
    };
    let (root, version) = if is_installer(package) {
        (install_dir(paths, app, package, None)?, None)
    } else if let Some(ref stored) = state {
        (
            install_dir(
                paths,
                app,
                package,
                Some(stored.version.trim_start_matches(['v', 'V'])),
            )?,
            Some(stored.version.clone()),
        )
    } else {
        return Ok(None);
    };
    let exe = safe_relative(&root, &package.executable_path)?;
    if !exe.is_file() {
        return Ok(None);
    }
    if is_installer(package) {
        let Some(actual) = read_product_version(&exe) else {
            return Ok(None);
        };
        if let Some(old) = state.filter(|s| {
            s.app_id == app.id
                && path_eq(Path::new(&s.install_directory), &root)
                && path_eq(Path::new(&s.executable_path), &exe)
        }) {
            let mut updated = old;
            updated.version = actual;
            return Ok(Some(updated));
        }
        return Ok(Some(InstalledApp {
            app_id: app.id.clone(),
            version: actual,
            executable_path: exe.to_string_lossy().into_owned(),
            install_directory: root.to_string_lossy().into_owned(),
            package_kind: package.kind.clone(),
            installed_at: chrono::Utc::now().to_rfc3339(),
        }));
    }
    let Some(stored) = state else {
        return Ok(None);
    };
    if stored.app_id == app.id
        && version.as_deref() == Some(stored.version.as_str())
        && path_eq(Path::new(&stored.install_directory), &root)
        && path_eq(Path::new(&stored.executable_path), &exe)
    {
        Ok(Some(stored))
    } else {
        Ok(None)
    }
}

pub fn is_running(app: &InstalledApp) -> bool {
    let expected = crate::storage::normalize_path(Path::new(&app.executable_path));
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system.processes().values().any(|p| {
        p.exe()
            .is_some_and(|exe| crate::storage::normalize_path(exe) == expected)
    })
}
pub fn launch(app: &InstalledApp) -> Result<(), String> {
    let root = Path::new(&app.install_directory);
    let exe = Path::new(&app.executable_path);
    let root_key = crate::storage::normalize_path(root);
    let exe_key = crate::storage::normalize_path(exe);
    if !exe.is_file()
        || !(exe_key == root_key
            || exe_key.starts_with(&(root_key.trim_end_matches(['\\', '/']).to_owned() + "\\")))
    {
        return Err("설치된 프로그램 실행 파일을 찾을 수 없습니다.".into());
    }
    Command::new(exe)
        .current_dir(root)
        .spawn()
        .map(|_| ())
        .map_err(|_| "프로그램을 실행하지 못했습니다.".into())
}

pub async fn install(
    client: &reqwest::Client,
    paths: &Paths,
    app: &AppDefinition,
    release: &AppRelease,
    cancel: Arc<AtomicBool>,
    progress: Arc<dyn Fn(InstallProgress) + Send + Sync>,
) -> Result<InstalledApp, String> {
    if app.is_private != release.is_private {
        return Err(
            "앱 공개 설정과 릴리스 접근 범위가 다릅니다. 최신 릴리스를 다시 확인하세요.".into(),
        );
    }
    if app.id.is_empty()
        || app.id.len() > 64
        || !app
            .id
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && b == b'-'))
    {
        return Err("앱 ID가 올바르지 않습니다.".into());
    }
    let package = package_for(app)?;
    let asset = select_asset(release, package)?;
    if asset.size > MAX_PACKAGE_BYTES {
        return Err("릴리스 파일이 허용 크기를 초과했습니다.".into());
    }
    let current = find_installed(paths, app)?;
    if current.as_ref().is_some_and(is_running) {
        return Err(
            "프로그램이 실행 중이므로 업데이트하지 않았습니다. 앱을 닫고 다시 시도하세요.".into(),
        );
    }
    let dl_dir = paths.data_dir.join("downloads").join(segment(&app.id)?);
    fs::create_dir_all(&dl_dir).map_err(|_| "다운로드 폴더를 만들지 못했습니다.")?;
    let ext = Path::new(&asset.name)
        .extension()
        .and_then(|x| x.to_str())
        .filter(|s| s.len() < 16)
        .unwrap_or("pkg");
    let archive = dl_dir.join(format!("{}.{}", uuid::Uuid::new_v4().simple(), ext));
    let result = async {
        report(app, "다운로드 준비", 0, Some(asset.size), progress.as_ref());
        let token = if app.is_private {
            crate::credentials::read_token(paths)?
        } else {
            None
        };
        let got = download_asset(
            client,
            asset,
            &app.repository,
            app.is_private,
            token.as_deref(),
            &archive,
            MAX_PACKAGE_BYTES,
            &app.id,
            &cancel,
            progress.as_ref(),
        )
        .await?;
        if asset.size > 0 && got.bytes != asset.size {
            return Err("다운로드 파일 크기가 GitHub 릴리스 정보와 다릅니다.".into());
        }
        let expected = expected_sha256(
            client,
            paths,
            release,
            asset,
            app,
            token.as_deref(),
            &cancel,
            progress.as_ref(),
        )
        .await?;
        if !sha_valid(&expected) || !constant_eq(&got.sha256, &expected) {
            return Err("다운로드 파일의 SHA-256 검증에 실패했습니다.".into());
        }
        check_cancel(&cancel)?;
        if package.kind.eq_ignore_ascii_case("zip") {
            install_zip(
                paths,
                app,
                release,
                package,
                &archive,
                progress.clone(),
                cancel.clone(),
            )
            .await
        } else if is_installer(package) {
            install_executable(
                paths,
                app,
                release,
                package,
                &archive,
                progress.as_ref(),
                &cancel,
            )
            .await
        } else {
            Err(format!("지원하지 않는 설치 패키지입니다: {}", package.kind))
        }
    }
    .await;
    let _ = fs::remove_file(archive);
    result
}

fn constant_eq(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (hex::decode(a), hex::decode(b)) else {
        return false;
    };
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |x, (l, r)| x | (l ^ r)) == 0
}
async fn expected_sha256(
    client: &reqwest::Client,
    paths: &Paths,
    release: &AppRelease,
    asset: &ReleaseAsset,
    app: &AppDefinition,
    token: Option<&str>,
    cancel: &AtomicBool,
    cb: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<String, String> {
    if let Some(hash) = &asset.sha256 {
        if sha_valid(hash) {
            return Ok(hash.to_ascii_lowercase());
        }
    }
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
        .ok_or("릴리스에 일치하는 SHA-256 체크섬이 없습니다.")?;
    let temp = paths
        .data_dir
        .join("downloads")
        .join(format!("{}.checksum", uuid::Uuid::new_v4().simple()));
    let result = download_asset(
        client,
        checksum,
        &app.repository,
        app.is_private,
        token,
        &temp,
        1024 * 1024,
        &app.id,
        cancel,
        cb,
    )
    .await;
    let out = match result {
        Ok(_) => fs::read_to_string(&temp)
            .map_err(|_| "SHA-256 체크섬 파일을 읽지 못했습니다.".to_owned())
            .and_then(|text| {
                checksum_matches(&text, &asset.name)
                    .ok_or("SHA-256 체크섬 파일에 일치하는 항목이 없습니다.".into())
            }),
        Err(e) => Err(e),
    };
    let _ = fs::remove_file(temp);
    out
}

async fn install_zip(
    paths: &Paths,
    app: &AppDefinition,
    release: &AppRelease,
    package: &PackageDefinition,
    archive_path: &Path,
    cb: Arc<dyn Fn(InstallProgress) + Send + Sync>,
    cancel: Arc<AtomicBool>,
) -> Result<InstalledApp, String> {
    let version = version_tag(&release.tag)?;
    let root = install_dir(paths, app, package, Some(&version))?;
    let apps_root = root.parent().ok_or("설치 폴더가 올바르지 않습니다.")?;
    fs::create_dir_all(apps_root).map_err(|_| "앱 설치 폴더를 만들지 못했습니다.")?;
    reject_reparse_path(apps_root)?;
    let stage = apps_root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&stage).map_err(|_| "임시 설치 폴더를 만들지 못했습니다.")?;
    if let Err(e) = reject_reparse_path(&stage) {
        let _ = fs::remove_dir_all(&stage);
        return Err(e);
    }
    let result = extract_zip(archive_path, &stage, cb.clone(), &app.id, cancel.clone())
        .await
        .and_then(|_| {
            let exe = safe_relative(&stage, &package.executable_path)?;
            if !exe.is_file() {
                return Err("패키지에 지정된 실행 파일이 없습니다.".into());
            }
            check_cancel(&cancel)?;
            if root.exists() {
                return Err(
                    "같은 버전의 설치 폴더가 이미 있습니다. 기존 파일을 보존했습니다.".into(),
                );
            }
            fs::rename(&stage, &root).map_err(|_| "설치 폴더를 확정하지 못했습니다.")?;
            let installed = InstalledApp {
                app_id: app.id.clone(),
                version: release.tag.clone(),
                executable_path: safe_relative(&root, &package.executable_path)?
                    .to_string_lossy()
                    .into_owned(),
                install_directory: root.to_string_lossy().into_owned(),
                package_kind: package.kind.clone(),
                installed_at: chrono::Utc::now().to_rfc3339(),
            };
            if let Err(e) = crate::storage::save_installed(paths, &installed) {
                let _ = fs::remove_dir_all(&root);
                return Err(e);
            }
            report(app, "설치 완료", 1, Some(1), cb.as_ref());
            Ok(installed)
        });
    let _ = fs::remove_dir_all(stage);
    result
}

async fn extract_zip(
    archive_path: &Path,
    stage: &Path,
    cb: Arc<dyn Fn(InstallProgress) + Send + Sync>,
    app_id: &str,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let archive_path = archive_path.to_path_buf();
    let stage = stage.to_path_buf();
    let app_id = app_id.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let file = fs::File::open(&archive_path).map_err(|_| "ZIP 파일을 열지 못했습니다.")?;
        let mut zip =
            zip::ZipArchive::new(file).map_err(|_| "ZIP 파일 형식이 올바르지 않습니다.")?;
        if zip.len() > MAX_ZIP_ENTRIES {
            return Err("ZIP 항목 수가 허용 범위를 초과했습니다.");
        }
        let mut names = HashSet::new();
        let mut expanded = 0u64;
        let mut buffer = [0u8; 81920];
        for index in 0..zip.len() {
            if cancel.load(Ordering::Relaxed) {
                return Err("작업이 취소되었습니다.");
            }
            let mut entry = zip
                .by_index(index)
                .map_err(|_| "ZIP 항목을 읽지 못했습니다.")?;
            let raw_name = entry.name().replace('\\', "/");
            let directory = entry.is_dir() || raw_name.ends_with('/');
            let name = if directory {
                raw_name.trim_end_matches('/').to_owned()
            } else {
                raw_name
            };
            if name.is_empty() {
                continue;
            }
            if name.starts_with('/') || name.contains(':') {
                return Err("ZIP에 허용되지 않은 경로가 있습니다.");
            }
            if name.split('/').any(|part| {
                part.is_empty() || part.ends_with([' ', '.']) || is_reserved_windows_name(part)
            }) {
                return Err("ZIP에 Windows에서 사용할 수 없는 파일 이름이 있습니다.");
            }
            let relative = Path::new(&name);
            if relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            {
                return Err("ZIP 경로가 설치 폴더를 벗어납니다.");
            }
            let dest = stage.join(relative);
            let key = crate::storage::normalize_path(&dest);
            if !names.insert(key) {
                return Err("ZIP 파일에 중복 경로가 있습니다.");
            }
            let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
            if mode == 0o120000 || (entry.unix_mode().unwrap_or(0) & 0o6000) != 0 {
                return Err("ZIP 심볼릭 링크나 특수 파일은 허용되지 않습니다.");
            }
            if directory {
                fs::create_dir_all(&dest).map_err(|_| "ZIP 폴더를 만들지 못했습니다.")?;
                reject_reparse_path(&dest)
                    .map_err(|_| "ZIP 경로에 링크 또는 재분석 지점이 있습니다.")?;
                continue;
            }
            if entry.size() > MAX_ENTRY {
                return Err("ZIP 항목 크기가 허용 범위를 초과했습니다.");
            }
            expanded = expanded
                .checked_add(entry.size())
                .ok_or("ZIP 크기가 올바르지 않습니다.")?;
            if expanded > MAX_EXPANDED {
                return Err("ZIP 압축 해제 크기가 허용 범위를 초과했습니다.");
            }
            if (entry.size() > 0 && entry.compressed_size() == 0)
                || (entry.compressed_size() > 0 && entry.size() / entry.compressed_size() > 500)
            {
                return Err("ZIP 압축 비율이 허용 범위를 초과했습니다.");
            }
            let parent = dest.parent().ok_or("ZIP 파일 경로가 올바르지 않습니다.")?;
            fs::create_dir_all(parent).map_err(|_| "ZIP 폴더를 만들지 못했습니다.")?;
            reject_reparse_path(parent)
                .map_err(|_| "ZIP 경로에 링크 또는 재분석 지점이 있습니다.")?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&dest)
                .map_err(|_| "ZIP 파일을 만들지 못했습니다.")?;
            let mut copied = 0u64;
            loop {
                let n = entry
                    .read(&mut buffer)
                    .map_err(|_| "ZIP 파일 압축을 해제하지 못했습니다.")?;
                if n == 0 {
                    break;
                }
                copied = copied
                    .checked_add(n as u64)
                    .ok_or("ZIP 크기가 올바르지 않습니다.")?;
                if copied > entry.size() || copied > MAX_ENTRY {
                    return Err("ZIP 항목이 선언된 크기를 초과했습니다.");
                }
                std::io::Write::write_all(&mut output, &buffer[..n])
                    .map_err(|_| "ZIP 파일을 기록하지 못했습니다.")?;
            }
            if copied != entry.size() {
                return Err("ZIP 항목의 실제 크기가 선언된 크기와 다릅니다.");
            }
            cb(InstallProgress {
                app_id: app_id.clone(),
                stage: "압축 해제".into(),
                bytes_received: expanded,
                total_bytes: Some(MAX_EXPANDED),
            });
        }
        Ok::<(), &str>(())
    })
    .await
    .map_err(|_| "ZIP 압축 해제 작업이 중단되었습니다.")?
    .map_err(str::to_owned)
}

async fn install_executable(
    paths: &Paths,
    app: &AppDefinition,
    release: &AppRelease,
    package: &PackageDefinition,
    installer: &Path,
    cb: &(dyn Fn(InstallProgress) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<InstalledApp, String> {
    let root = install_dir(paths, app, package, None)?;
    let exe = safe_relative(&root, &package.executable_path)?;
    if exe.exists() {
        let probe = InstalledApp {
            app_id: app.id.clone(),
            version: String::new(),
            executable_path: exe.to_string_lossy().into_owned(),
            install_directory: root.to_string_lossy().into_owned(),
            package_kind: package.kind.clone(),
            installed_at: String::new(),
        };
        if is_running(&probe) {
            return Err("프로그램이 실행 중이므로 업데이트하지 않았습니다.".into());
        }
    }
    fs::create_dir_all(&root).map_err(|_| "설치 폴더를 만들지 못했습니다.")?;
    reject_reparse_path(&root)?;
    check_cancel(cancel)?;
    report(app, "설치 프로그램 실행", 0, None, cb);
    let mut cmd = Command::new(installer);
    cmd.current_dir(installer.parent().unwrap_or(Path::new(".")));
    if package.kind.eq_ignore_ascii_case("inno") {
        cmd.args([
            "/SILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/NOCLOSEAPPLICATIONS",
            "/NORESTARTAPPLICATIONS",
            "/SP-",
        ])
        .arg(format!("/DIR={}", root.display()));
    } else {
        let p = root.to_string_lossy();
        if p.contains('"') || p.chars().any(char::is_control) {
            return Err("NSIS 설치 폴더 경로가 올바르지 않습니다.".into());
        }
        cmd.arg("/S").arg(format!("/D={p}"));
    }
    let status = tokio::task::spawn_blocking(move || cmd.status())
        .await
        .map_err(|_| "설치 프로그램 작업이 중단되었습니다.")?
        .map_err(|_| "설치 프로그램을 시작하지 못했습니다.")?;
    if !status.success()
        && !(package.kind.eq_ignore_ascii_case("inno") && status.code() == Some(3010))
    {
        return Err(format!(
            "설치 프로그램이 오류 코드 {:?}로 종료되었습니다.",
            status.code()
        ));
    }
    if !exe.is_file() {
        return Err(
            "설치 후 실행 파일을 찾을 수 없습니다. 설치 프로그램의 설치 경로를 확인하세요.".into(),
        );
    }
    let actual = read_product_version(&exe).ok_or("설치된 프로그램 버전을 확인하지 못했습니다.")?;
    let expected = version_tag(&release.tag)?;
    if normalize_version(&actual) != normalize_version(&expected) {
        return Err(format!(
            "설치된 프로그램 버전({actual})이 릴리스 버전({expected})과 다릅니다."
        ));
    }
    let installed = InstalledApp {
        app_id: app.id.clone(),
        version: actual,
        executable_path: exe.to_string_lossy().into_owned(),
        install_directory: root.to_string_lossy().into_owned(),
        package_kind: package.kind.clone(),
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    crate::storage::save_installed(paths, &installed)?;
    report(app, "설치 완료", 1, Some(1), cb);
    Ok(installed)
}
pub(crate) fn normalize_version(v: &str) -> String {
    let mut s = v
        .trim()
        .trim_start_matches(['v', 'V'])
        .split('+')
        .next()
        .unwrap_or("")
        .to_owned();
    if s.ends_with(".0") && s.split('.').count() == 4 {
        s.truncate(s.len() - 2);
    }
    s
}
fn is_reserved_windows_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
fn reject_reparse_path(path: &Path) -> Result<(), String> {
    let full = path.to_path_buf();
    if full.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("경로에 상위 폴더 이동이 포함되어 있습니다.".into());
    }
    let mut current = PathBuf::new();
    for component in full.components() {
        current.push(component);
        if let Ok(meta) = fs::symlink_metadata(&current) {
            if meta.file_type().is_symlink() {
                return Err("경로에 링크 또는 재분석 지점이 있습니다.".into());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                GetFileAttributesW, FILE_ATTRIBUTE_REPARSE_POINT, INVALID_FILE_ATTRIBUTES,
            };
            let wide: Vec<u16> = current.as_os_str().encode_wide().chain(Some(0)).collect();
            let attrs = unsafe { GetFileAttributesW(wide.as_ptr()) };
            if attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err("경로에 링크 또는 재분석 지점이 있습니다.".into());
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn read_product_version(path: &Path) -> Option<String> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    #[repr(C)]
    struct Fixed {
        signature: u32,
        struct_version: u32,
        file_version_ms: u32,
        file_version_ls: u32,
        product_version_ms: u32,
        product_version_ls: u32,
        flags_mask: u32,
        flags: u32,
        file_os: u32,
        file_type: u32,
        file_subtype: u32,
        file_date_ms: u32,
        file_date_ls: u32,
    }
    let w: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
    let mut handle = 0u32;
    let size = unsafe { GetFileVersionInfoSizeW(w.as_ptr(), &mut handle) };
    if size == 0 || size > 16 * 1024 * 1024 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(w.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let key: Vec<u16> = "\\".encode_utf16().chain(Some(0)).collect();
    let mut value: *mut core::ffi::c_void = ptr::null_mut();
    let mut len = 0u32;
    if unsafe { VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut value, &mut len) } == 0
        || len < std::mem::size_of::<Fixed>() as u32
    {
        return None;
    }
    let fixed = unsafe { &*(value as *const Fixed) };
    if fixed.signature != 0xFEEF04BD {
        return None;
    }
    let ms = fixed.product_version_ms;
    let ls = fixed.product_version_ls;
    Some(format!(
        "{}.{}.{}.{}",
        ms >> 16,
        ms & 0xffff,
        ls >> 16,
        ls & 0xffff
    ))
    .map(|v| normalize_version(&v))
}
#[cfg(not(windows))]
pub(crate) fn read_product_version(_path: &Path) -> Option<String> {
    None
}
