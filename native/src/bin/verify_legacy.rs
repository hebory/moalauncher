use moalauncher_lib::{credentials, storage, storage::Paths};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct Args {
    data_dir: Option<PathBuf>,
    output: Option<PathBuf>,
    allow_real_profile_read: bool,
}

#[derive(Serialize)]
struct Report {
    mode: &'static str,
    settings_readable: bool,
    favorite_count: usize,
    unknown_setting_field_count: usize,
    catalog_readable: bool,
    catalog_app_count: usize,
    private_app_count: usize,
    installed_state_file_count: usize,
    installed_state_readable_count: usize,
    installed_state_unreadable_count: usize,
    installed_state_readable: bool,
    installed_state_listing_readable: bool,
    credential_read_succeeded: bool,
    credential_present: bool,
    source_file_count: usize,
    source_hashes_unchanged: bool,
}

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("호환성 검증을 완료하지 못했습니다: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let output = args.output.as_ref().ok_or_else(usage)?;
    let (paths, mode, _fixture_guard) = if args.allow_real_profile_read {
        (Paths::new(args.data_dir.clone()), "real-profile-read", None)
    } else {
        if args.data_dir.is_some() {
            return Err(
                "실제 데이터 폴더를 읽으려면 --allow-real-profile-read를 명시하세요.".into(),
            );
        }
        let guard = create_fixture()?;
        (
            Paths::new(Some(guard.0.clone())),
            "temporary-fixture",
            Some(guard),
        )
    };
    let data_dir = fs::canonicalize(&paths.data_dir)
        .map_err(|_| "검증할 데이터 폴더를 찾지 못했습니다.".to_string())?;
    let output_path = resolve_output(output)?;
    if is_within(&output_path, &data_dir) {
        return Err("검증 결과 경로는 원본 데이터 폴더 바깥이어야 합니다.".into());
    }

    let (before_hash, file_count) = hash_tree(&data_dir)?;
    let mut report = inspect(&paths, mode, file_count);
    let (after_hash, after_count) = hash_tree(&data_dir)?;
    report.source_file_count = after_count;
    report.source_hashes_unchanged = file_count == after_count && before_hash == after_hash;
    let json = serde_json::to_vec_pretty(&report)
        .map_err(|_| "검증 결과를 만들지 못했습니다.".to_string())?;
    use std::io::Write;
    let mut output_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|_| {
            "검증 결과 파일을 만들지 못했습니다. 기존 파일은 덮어쓰지 않습니다.".to_string()
        })?;
    output_file
        .write_all(&json)
        .map_err(|_| "검증 결과 파일을 저장하지 못했습니다.".to_string())?;
    output_file
        .sync_all()
        .map_err(|_| "검증 결과 파일 저장을 완료하지 못했습니다.".to_string())?;
    if !report.source_hashes_unchanged {
        return Err("검증 중 원본 파일의 해시가 변경되었습니다. 결과 파일을 확인하세요.".into());
    }
    println!(
        "호환성 검증 완료: 모드={}, 설정={}, 카탈로그={}, 설치 상태={}/{}, 자격 증명 읽기={}, 원본 변경 없음={}",
        report.mode,
        report.settings_readable,
        report.catalog_readable,
        report.installed_state_readable_count,
        report.installed_state_file_count,
        report.credential_read_succeeded,
        report.source_hashes_unchanged,
    );
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut iter = env::args_os().skip(1);
    while let Some(arg) = iter.next() {
        match arg.to_string_lossy().as_ref() {
            "--help" | "-h" => return Err(usage()),
            "--data-dir" => {
                if args.data_dir.is_some() {
                    return Err("--data-dir는 한 번만 지정할 수 있습니다.".into());
                }
                args.data_dir = Some(PathBuf::from(next_value(&mut iter, "--data-dir")?));
            }
            "--output" => {
                if args.output.is_some() {
                    return Err("--output는 한 번만 지정할 수 있습니다.".into());
                }
                args.output = Some(PathBuf::from(next_value(&mut iter, "--output")?));
            }
            "--allow-real-profile-read" => {
                if args.allow_real_profile_read {
                    return Err("--allow-real-profile-read는 한 번만 지정할 수 있습니다.".into());
                }
                args.allow_real_profile_read = true;
            }
            _ => return Err(usage()),
        }
    }
    Ok(args)
}

fn next_value(iter: &mut impl Iterator<Item = OsString>, option: &str) -> Result<OsString, String> {
    iter.next()
        .filter(|value| !value.to_string_lossy().starts_with('-'))
        .ok_or_else(|| format!("{option} 뒤에 경로가 필요합니다."))
}

fn usage() -> String {
    "사용법: verify_legacy --output <결과.json> [--allow-real-profile-read] [--data-dir <폴더>]"
        .into()
}

fn create_fixture() -> Result<TempDir, String> {
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!("moalauncher-legacy-fixture-{}-{id}", process::id()));
    fs::create_dir_all(path.join("installed"))
        .map_err(|_| "임시 호환성 검증 데이터를 만들지 못했습니다.".to_string())?;
    fs::write(
        path.join("settings.json"),
        br#"{
  "CheckOnStartup": true,
  "PeriodicChecks": true,
  "UpdateNotifications": true,
  "UseGridView": true,
  "FavoriteAppIds": ["fixture-app"],
  "CheckIntervalMinutes": 60,
  "legacyUnknownSetting": {"retained": true}
}"#,
    )
    .map_err(|_| "임시 설정 데이터를 만들지 못했습니다.".to_string())?;
    fs::write(
        path.join("catalog.json"),
        br#"{"schemaVersion":1,"apps":[]}"#,
    )
    .map_err(|_| "임시 카탈로그 데이터를 만들지 못했습니다.".to_string())?;
    fs::write(
        path.join("installed/fixture-app.json"),
        br#"{
  "AppId": "fixture-app",
  "Version": "1.2.3",
  "ExecutablePath": "Programs/Fixture/Fixture.exe",
  "InstallDirectory": "Programs/Fixture",
  "PackageKind": "inno",
  "InstalledAt": "2025-01-01T00:00:00Z"
}"#,
    )
    .map_err(|_| "임시 설치 상태 데이터를 만들지 못했습니다.".to_string())?;
    Ok(TempDir(path))
}

fn inspect(paths: &Paths, mode: &'static str, source_file_count: usize) -> Report {
    let settings = storage::load_settings(paths).ok();
    let catalog = storage::load_catalog(paths).ok();
    let (
        installed_state_file_count,
        installed_state_readable_count,
        installed_state_listing_readable,
    ) = installed_counts(&paths.data_dir);
    let credential_result = credentials::read_token(paths);
    let (credential_read_succeeded, credential_present) = match credential_result {
        Ok(Some(token)) => {
            let mut bytes = token.into_bytes();
            bytes.fill(0);
            (true, true)
        }
        Ok(None) => (true, false),
        Err(_) => (false, false),
    };
    let settings_readable = settings.is_some();
    let catalog_readable = catalog.is_some();
    let private_app_count = catalog
        .as_ref()
        .map(|c| c.apps.iter().filter(|a| a.is_private).count())
        .unwrap_or(0);
    Report {
        mode,
        settings_readable,
        favorite_count: settings
            .as_ref()
            .map(|s| s.favorite_app_ids.len())
            .unwrap_or(0),
        unknown_setting_field_count: settings.as_ref().map(|s| s.extra.len()).unwrap_or(0),
        catalog_readable,
        catalog_app_count: catalog.as_ref().map(|c| c.apps.len()).unwrap_or(0),
        private_app_count,
        installed_state_file_count,
        installed_state_readable_count,
        installed_state_unreadable_count: installed_state_file_count
            .saturating_sub(installed_state_readable_count),
        installed_state_readable: installed_state_listing_readable
            && installed_state_file_count == installed_state_readable_count,
        installed_state_listing_readable,
        credential_read_succeeded,
        credential_present,
        source_file_count,
        source_hashes_unchanged: false,
    }
}

fn installed_counts(data_dir: &Path) -> (usize, usize, bool) {
    let dir = data_dir.join("installed");
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (0, 0, true),
        Err(_) => return (0, 0, false),
    };
    let mut total = 0;
    let mut readable = 0;
    let mut listing_readable = true;
    for entry in entries {
        let Ok(entry) = entry else {
            listing_readable = false;
            continue;
        };
        let path = entry.path();
        if !path.is_file()
            || !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        total += 1;
        let Some(app_id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if matches!(
            storage::load_installed(
                &Paths {
                    data_dir: data_dir.to_path_buf(),
                    local_app_data: PathBuf::new()
                },
                app_id
            ),
            Ok(Some(_))
        ) {
            readable += 1;
        }
    }
    (total, readable, listing_readable)
}

fn resolve_output(output: &Path) -> Result<PathBuf, String> {
    let name = output
        .file_name()
        .ok_or_else(|| "결과 파일 경로가 올바르지 않습니다.".to_string())?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent)
        .map_err(|_| "결과 파일의 상위 폴더가 존재하지 않습니다.".to_string())?;
    Ok(parent.join(name))
}

fn is_within(candidate: &Path, parent: &Path) -> bool {
    let candidate = candidate
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase();
    let parent = parent
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    candidate == parent || candidate.starts_with(&(parent + "\\"))
}

fn hash_tree(root: &Path) -> Result<(String, usize), String> {
    fn visit(
        root: &Path,
        current: &Path,
        hash: &mut Sha256,
        count: &mut usize,
    ) -> Result<(), String> {
        let mut entries = fs::read_dir(current)
            .map_err(|_| "원본 파일을 확인하지 못했습니다.".to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "원본 파일 목록을 읽지 못했습니다.".to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| "원본 파일을 확인하지 못했습니다.".to_string())?;
            if metadata.file_type().is_symlink() {
                return Err("원본 폴더에 링크가 있어 안전한 검증을 진행할 수 없습니다.".into());
            }
            if metadata.is_dir() {
                visit(root, &path, hash, count)?;
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "원본 경로를 확인하지 못했습니다.".to_string())?;
            let name = relative.to_string_lossy();
            let bytes = fs::read(&path).map_err(|_| "원본 파일을 읽지 못했습니다.".to_string())?;
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
            *count += 1;
        }
        Ok(())
    }
    let mut hasher = Sha256::new();
    let mut count = 0;
    visit(root, root, &mut hasher, &mut count)?;
    Ok((hex::encode(hasher.finalize()), count))
}
