use crate::models::{Catalog, InstalledApp, Settings};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const BUNDLED_CATALOG: &str = include_str!("../../catalog/apps.json");
const SETTINGS_FILE: &str = "settings.json";
const CATALOG_FILE: &str = "catalog.json";
const INSTALLED_DIR: &str = "installed";
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub local_app_data: PathBuf,
}

impl Paths {
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let data_dir = data_dir.unwrap_or_else(|| local_app_data.join("MoaLauncher"));
        let data_dir = absolute_lexical(&data_dir);
        Self {
            data_dir,
            local_app_data,
        }
    }
}

fn absolute_lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut normalized = PathBuf::new();
    let mut rooted = false;
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => {
                normalized.push(component.as_os_str());
                rooted = true;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                } else if !rooted {
                    normalized.push("..");
                }
            }
            Component::Normal(name) => normalized.push(name),
        }
    }
    normalized
}

pub fn normalize_path(path: &Path) -> String {
    let normalized = absolute_lexical(path);
    // `components()` removes non-root trailing separators. Keep the root separator
    // intact, matching Path.TrimEndingDirectorySeparator rather than turning C:\ into C:.
    dunce::simplified(&normalized)
        .to_string_lossy()
        .chars()
        .map(|character| {
            let mut uppercase = character.to_uppercase();
            match (uppercase.next(), uppercase.next()) {
                (Some(single), None) => single,
                _ => character,
            }
        })
        .collect()
}

pub fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(name) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && !owner.is_empty()
        && owner.len() <= 39
        && owner
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !name.is_empty()
        && name.len() <= 100
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

pub fn safe_relative(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.len() > 240
        || relative.contains('\\')
        || relative.contains(':')
        || relative.starts_with('/')
    {
        return Err("상대 경로 형식이 올바르지 않습니다.".into());
    }
    let rel = Path::new(relative);
    if rel.components().any(|c| !matches!(c, Component::Normal(_)))
        || rel.components().count() == 0
        || relative.split('/').any(|part| {
            part.is_empty() || part.ends_with([' ', '.']) || part.chars().any(char::is_control)
        })
    {
        return Err("상대 경로 형식이 올바르지 않습니다.".into());
    }
    // Build with native separators: NSIS strips forward slashes in its /D= tail.
    let native_relative: PathBuf = rel.components().collect();
    Ok(root.join(native_relative))
}

pub fn validate_catalog(catalog: &Catalog) -> Result<(), String> {
    if catalog.schema_version != 1 {
        return Err("지원하지 않는 카탈로그 버전입니다.".into());
    }
    if catalog.apps.len() > 100 {
        return Err("앱 목록은 100개 이하여야 합니다.".into());
    }
    let mut ids = HashSet::new();
    for app in &catalog.apps {
        if app.id.is_empty()
            || app.id.len() > 64
            || !app
                .id
                .bytes()
                .enumerate()
                .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && b == b'-'))
        {
            return Err("앱 ID 형식이 올바르지 않습니다.".into());
        }
        if !ids.insert(app.id.to_ascii_lowercase()) {
            return Err("카탈로그에 중복된 앱 ID가 있습니다.".into());
        }
        if !valid_text(&app.name, 80, true)
            || !valid_text(&app.description, 500, false)
            || !valid_text(&app.category, 40, true)
            || !valid_text(&app.monogram, 4, true)
            || app.accent_color.len() != 7
            || !app.accent_color.starts_with('#')
            || !app.accent_color[1..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("앱 표시 정보 형식이 올바르지 않습니다.".into());
        }
        if !app.repository.is_empty() && !valid_repo(&app.repository) {
            return Err("GitHub 저장소 형식이 올바르지 않습니다.".into());
        }
        for url in [&app.icon_url, &app.cover_url].into_iter().flatten() {
            if !safe_image_url(url, &app.repository) {
                return Err("앱 이미지 주소 형식이 올바르지 않습니다.".into());
            }
        }
        if app.packages.is_empty() || app.packages.len() > 12 {
            return Err("앱 패키지 목록이 올바르지 않습니다.".into());
        }
        let mut archs = HashSet::new();
        for package in &app.packages {
            if !matches!(package.architecture.as_str(), "x64" | "x86" | "arm64")
                || !archs.insert(&package.architecture)
            {
                return Err("프로세서 패키지 형식이 올바르지 않습니다.".into());
            }
            if !matches!(package.kind.as_str(), "zip" | "inno" | "nsis")
                || package.asset_pattern.is_empty()
                || package.asset_pattern.len() > 160
                || package.asset_pattern.contains("..")
                || package.asset_pattern.contains(['/', '\\', ':'])
            {
                return Err("릴리스 패키지 형식이 올바르지 않습니다.".into());
            }
            safe_relative(Path::new("."), &package.executable_path)?;
            if !package
                .executable_path
                .to_ascii_lowercase()
                .ends_with(".exe")
            {
                return Err("실행 파일 경로 형식이 올바르지 않습니다.".into());
            }
            if package.kind == "zip" {
                if package.install_directory.is_some() {
                    return Err("압축 패키지 설치 경로 형식이 올바르지 않습니다.".into());
                }
                if !package.asset_pattern.to_ascii_lowercase().ends_with(".zip") {
                    return Err("릴리스 파일 형식이 올바르지 않습니다.".into());
                }
            } else {
                let dir = package
                    .install_directory
                    .as_deref()
                    .ok_or("설치 프로그램 경로가 필요합니다.")?;
                safe_relative(Path::new("."), dir)?;
                if !package.asset_pattern.to_ascii_lowercase().ends_with(".exe") {
                    return Err("릴리스 파일 형식이 올바르지 않습니다.".into());
                }
            }
        }
    }
    Ok(())
}

fn valid_text(s: &str, max: usize, required: bool) -> bool {
    s.encode_utf16().count() <= max
        && (!required || !s.trim().is_empty())
        && !s.chars().any(char::is_control)
}

fn safe_image_url(url: &str, repo: &str) -> bool {
    if url.is_empty() {
        return true;
    }
    if url.len() > 2048 || !valid_repo(repo) {
        return false;
    }
    let Ok(uri) = url::Url::parse(url) else {
        return false;
    };
    if uri.scheme() != "https"
        || !uri
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("raw.githubusercontent.com"))
        || uri.port().is_some()
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return false;
    }
    let Some(segments) = uri.path_segments() else {
        return false;
    };
    let Some(segments) = segments
        .map(decode_image_segment)
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    if segments.len() < 4
        || segments.iter().any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.contains('/')
                || part.contains('\\')
                || part.chars().any(char::is_control)
        })
    {
        return false;
    }
    let mut repo_parts = repo.split('/');
    if !segments[0].eq_ignore_ascii_case(repo_parts.next().unwrap_or(""))
        || !segments[1].eq_ignore_ascii_case(repo_parts.next().unwrap_or(""))
    {
        return false;
    }
    let last = segments.last().unwrap().to_ascii_lowercase();
    [".png", ".jpg", ".jpeg"]
        .iter()
        .any(|extension| last.ends_with(extension))
}

fn decode_image_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = *bytes.get(index + 1)?;
            let lo = *bytes.get(index + 2)?;
            decoded.push(hex_nibble(hi)? * 16 + hex_nibble(lo)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub fn load_settings(paths: &Paths) -> Result<Settings, String> {
    read_json(&paths.data_dir.join(SETTINGS_FILE))
        .map(|v| normalize_settings(v.unwrap_or_default()))
}

pub fn save_settings(paths: &Paths, settings: &Settings) -> Result<(), String> {
    let path = paths.data_dir.join(SETTINGS_FILE);
    ensure_existing_valid::<Settings>(&path)?;
    backup_existing(paths)?;
    atomic_json(&path, &normalize_settings(settings.clone()))
}

fn normalize_settings(mut settings: Settings) -> Settings {
    if ![30, 60, 180, 360, 720, 1440].contains(&settings.check_interval_minutes) {
        settings.check_interval_minutes = 60;
    }
    let mut seen = HashSet::new();
    settings
        .favorite_app_ids
        .retain(|id| valid_id(id) && seen.insert(id.clone()));
    settings.favorite_app_ids.truncate(256);
    settings
}

pub fn load_personal_catalog(paths: &Paths) -> Result<Catalog, String> {
    let catalog: Catalog = read_json(&paths.data_dir.join(CATALOG_FILE))?.unwrap_or_default();
    validate_catalog(&catalog)?;
    Ok(catalog)
}

pub fn save_personal_catalog(paths: &Paths, catalog: &Catalog) -> Result<(), String> {
    validate_catalog(catalog)?;
    let path = paths.data_dir.join(CATALOG_FILE);
    ensure_existing_valid::<Catalog>(&path)?;
    backup_existing(paths)?;
    atomic_json(&path, catalog)
}

pub fn load_catalog(paths: &Paths) -> Result<Catalog, String> {
    let bundled: Catalog = serde_json::from_str(BUNDLED_CATALOG)
        .map_err(|_| "기본 카탈로그를 읽지 못했습니다.".to_string())?;
    validate_catalog(&bundled)?;
    let personal = load_personal_catalog(paths)?;
    let mut merged = bundled.apps;
    let mut indices: HashMap<String, usize> = merged
        .iter()
        .enumerate()
        .map(|(index, app)| (app.id.to_ascii_lowercase(), index))
        .collect();
    for app in personal.apps {
        let key = app.id.to_ascii_lowercase();
        if let Some(index) = indices.get(&key).copied() {
            let base = &merged[index];
            let mut copy = app;
            if copy.repository.eq_ignore_ascii_case(&base.repository) && copy.icon_url.is_none() {
                copy.icon_url = base.icon_url.clone();
            }
            if copy.repository.eq_ignore_ascii_case(&base.repository) && copy.cover_url.is_none() {
                copy.cover_url = base.cover_url.clone();
            }
            merged[index] = copy;
        } else {
            indices.insert(key, merged.len());
            merged.push(app);
        }
    }
    merged.retain(|app| app.id != "moa-launcher");
    let result = Catalog {
        schema_version: 1,
        apps: merged,
    };
    validate_catalog(&result)?;
    Ok(result)
}

pub fn save_installed(paths: &Paths, app: &InstalledApp) -> Result<(), String> {
    if !valid_id(&app.app_id) {
        return Err("설치 앱 ID 형식이 올바르지 않습니다.".into());
    }
    let path = paths
        .data_dir
        .join(INSTALLED_DIR)
        .join(format!("{}.json", app.app_id));
    ensure_existing_valid::<InstalledApp>(&path)?;
    backup_existing(paths)?;
    atomic_json(&path, app)
}

pub fn load_installed(paths: &Paths, app_id: &str) -> Result<Option<InstalledApp>, String> {
    if !valid_id(app_id) {
        return Err("설치 앱 ID 형식이 올바르지 않습니다.".into());
    }
    let path = paths
        .data_dir
        .join(INSTALLED_DIR)
        .join(format!("{app_id}.json"));
    read_json(&path)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && b == b'-'))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| format!("저장 파일을 읽지 못했습니다: {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(format!("저장 파일을 읽지 못했습니다: {}", path.display())),
    }
}

fn ensure_existing_valid<T: DeserializeOwned>(path: &Path) -> Result<(), String> {
    if path.exists() {
        read_json::<T>(path)?
            .ok_or_else(|| "저장 파일을 확인하지 못했습니다.".to_string())
            .map(|_| ())?;
    }
    Ok(())
}

fn backup_existing(paths: &Paths) -> Result<(), String> {
    let backup = paths.data_dir.join("migration-backup").join("tauri-v1");
    for name in [SETTINGS_FILE, CATALOG_FILE] {
        let source = paths.data_dir.join(name);
        backup_one(&source, &backup.join(name))?;
    }
    let installed = paths.data_dir.join(INSTALLED_DIR);
    if installed.is_dir() {
        for entry in
            fs::read_dir(&installed).map_err(|_| "기존 설치 상태를 백업하지 못했습니다.")?
        {
            let source = entry
                .map_err(|_| "기존 설치 상태를 백업하지 못했습니다.")?
                .path();
            if source.is_file() {
                let target = backup
                    .join(INSTALLED_DIR)
                    .join(source.file_name().unwrap_or_default());
                backup_one(&source, &target)?;
            }
        }
    }
    Ok(())
}
fn backup_one(source: &Path, target: &Path) -> Result<(), String> {
    if !source.is_file() || target.exists() {
        return Ok(());
    }
    let bytes = fs::read(source).map_err(|_| "기존 사용자 데이터를 백업하지 못했습니다.")?;
    fs::create_dir_all(target.parent().unwrap_or(Path::new(".")))
        .map_err(|_| "백업 폴더를 만들지 못했습니다.")?;
    write_atomic_bytes(target, &bytes)
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| "저장 형식으로 변환하지 못했습니다.".to_string())?;
    write_atomic_bytes(path, &bytes)
}
fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "저장 경로가 올바르지 않습니다.".to_string())?;
    fs::create_dir_all(parent).map_err(|_| "저장 폴더를 만들지 못했습니다.".to_string())?;
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".moalauncher-{}-{id}.tmp", std::process::id()));
    let result = (|| {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|_| "임시 파일을 만들지 못했습니다.")?;
        f.write_all(bytes)
            .map_err(|_| "파일을 저장하지 못했습니다.")?;
        f.sync_all()
            .map_err(|_| "파일 저장을 완료하지 못했습니다.")?;
        drop(f);
        replace_file(&tmp, path)?;
        Ok(())
    })();
    if tmp.exists() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        return Err("파일을 원자적으로 저장하지 못했습니다.".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> Result<(), String> {
    fs::rename(source, target).map_err(|_| "파일을 원자적으로 저장하지 못했습니다.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AppDefinition;

    fn app(id: &str, name: &str, repository: &str, icon_url: Option<&str>) -> AppDefinition {
        AppDefinition {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            category: "유틸리티".into(),
            accent_color: "#7C9CFF".into(),
            monogram: "모아".into(),
            icon_url: icon_url.map(str::to_owned),
            cover_url: None,
            repository: repository.into(),
            is_private: false,
            packages: vec![crate::models::PackageDefinition {
                architecture: "x64".into(),
                kind: "zip".into(),
                asset_pattern: "tool.zip".into(),
                executable_path: "tool.exe".into(),
                install_directory: None,
            }],
        }
    }

    fn paths() -> Paths {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "moalauncher-storage-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        Paths {
            data_dir: dir,
            local_app_data: std::env::temp_dir(),
        }
    }
    #[test]
    fn bundled_catalog_loads_and_excludes_launcher() {
        let c = load_catalog(&paths()).unwrap();
        assert!(!c.apps.iter().any(|a| a.id == "moa-launcher"));
        assert!(!c.apps.is_empty());
    }

    #[test]
    fn catalog_accepts_csharp_utf16_monogram_and_empty_image_override() {
        let catalog = Catalog {
            schema_version: 1,
            apps: vec![app("fixture-app", "도구", "hebory/moaview", Some(""))],
        };
        validate_catalog(&catalog).unwrap();
    }

    #[test]
    fn personal_catalog_keeps_bundled_order_and_filters_launcher_id() {
        let paths = paths();
        let personal = Catalog {
            schema_version: 1,
            apps: vec![
                app("moa-launcher", "Moa Launcher", "hebory/moalauncher", None),
                app("personal-tool", "개인 도구", "owner/private-tool", None),
            ],
        };
        save_personal_catalog(&paths, &personal).unwrap();
        let catalog = load_catalog(&paths).unwrap();
        let ids: Vec<_> = catalog.apps.iter().map(|app| app.id.as_str()).collect();
        assert_eq!(&ids[..3], &["moaview", "moavcs", "moasearch"]);
        assert_eq!(ids.last(), Some(&"personal-tool"));
        assert!(!ids.contains(&"moa-launcher"));
    }
    #[test]
    fn settings_backup_and_unknown_setting_survive() {
        let p = paths();
        fs::create_dir_all(&p.data_dir).unwrap();
        fs::write(
            p.data_dir.join(SETTINGS_FILE),
            r#"{"checkIntervalMinutes":180,"futureOption":{"x":1}}"#,
        )
        .unwrap();
        let mut s = load_settings(&p).unwrap();
        assert_eq!(s.check_interval_minutes, 180);
        s.use_grid_view = false;
        save_settings(&p, &s).unwrap();
        assert!(p
            .data_dir
            .join("migration-backup/tauri-v1/settings.json")
            .exists());
        let v: serde_json::Value =
            serde_json::from_slice(&fs::read(p.data_dir.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(v["futureOption"]["x"], 1);
        assert!(!v["useGridView"].as_bool().unwrap());
    }
    #[test]
    fn unsafe_paths_are_rejected() {
        assert!(safe_relative(Path::new("C:/root"), "../secret.exe").is_err());
        assert!(safe_relative(Path::new("root"), "folder/app.exe").is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn data_path_normalization_matches_dotnet_roots_and_unc_paths() {
        assert_eq!(normalize_path(Path::new(r"\\?\C:\Users\user\Data")), normalize_path(Path::new(r"C:\Users\user\Data")));
        assert_eq!(
            normalize_path(Path::new(r"C:\Users\tester\..\Data\MoaLauncher\")),
            r"C:\USERS\DATA\MOALAUNCHER"
        );
        assert_eq!(normalize_path(Path::new("C:\\")), "C:\\");
        assert_eq!(
            normalize_path(Path::new(r"\\server\share\folder\..\data")),
            r"\\SERVER\SHARE\DATA"
        );
        assert_eq!(normalize_path(Path::new("C:\\ß")), "C:\\ß");
    }
}
