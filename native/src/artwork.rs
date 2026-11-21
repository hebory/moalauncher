use crate::{credentials, models::AppDefinition, storage::Paths};
use base64::Engine;
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
const MAX_PIXELS: u64 = 32_000_000;
const MAX_CACHE_FILES: usize = 64;
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_CACHE_BYTES: u64 = 12 * 1024 * 1024;
static CACHE_IO: Mutex<()> = Mutex::new(());
type RequestLocks = HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>;
static REQUESTS: LazyLock<Mutex<RequestLocks>> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Default, Serialize, Deserialize)]
struct CacheRecord {
    checked_at: u64,
    attempted_at: u64,
    etag: Option<String>,
    last_modified: Option<String>,
    image: Option<String>,
}

struct RemoteArtwork {
    uri: url::Url,
    path: PathBuf,
    legacy: Option<PathBuf>,
    token: Option<String>,
    entropy: Vec<u8>,
}

/// Returns a validated local image without waiting for GitHub, including expired images.
pub fn cached(paths: &Paths, app: &AppDefinition, value: &str) -> Option<String> {
    let remote = remote_artwork(paths, app, value)?;
    let record = read_cache(&remote)?;
    if token_is_current(paths, &remote) {
        record_image(&record)
    } else {
        None
    }
}

pub fn clear_private_cache(paths: &Paths) {
    let _guard = CACHE_IO.lock().unwrap();
    let _ = fs::remove_dir_all(paths.data_dir.join("cache/artwork/private"));
}

pub async fn load(
    client: &Client,
    paths: &Paths,
    app: &AppDefinition,
    value: &str,
) -> Result<Option<String>, String> {
    let Some(remote) = remote_artwork(paths, app, value) else {
        return Ok(None);
    };
    load_remote(client, paths, remote).await
}

fn remote_artwork(paths: &Paths, app: &AppDefinition, value: &str) -> Option<RemoteArtwork> {
    let (uri, _, private) = artwork_uri(app, value)?;
    if !private {
        return Some(RemoteArtwork {
            uri,
            path: cache_path(paths, value),
            legacy: Some(legacy_path(paths, value)),
            token: None,
            entropy: vec![],
        });
    }
    let token = credentials::read_token(paths)
        .ok()
        .flatten()
        .filter(|t| valid_token(t))?;
    let segments: Vec<String> = uri
        .path_segments()
        .map(|p| p.map(decode_segment).collect())
        .unwrap_or_default();
    let owner = &segments[0];
    let name = &segments[1];
    let reference = &segments[2];
    let mut api_url = url::Url::parse("https://api.github.com/").expect("constant URL is valid");
    {
        let Ok(mut path) = api_url.path_segments_mut() else {
            return None;
        };
        path.pop_if_empty()
            .push("repos")
            .push(owner)
            .push(name)
            .push("contents");
        for part in &segments[3..] {
            path.push(part);
        }
    }
    api_url.query_pairs_mut().append_pair("ref", reference);
    let mut scope = Sha256::new();
    scope.update(b"MoaLauncher/private-artwork/v1\0");
    scope.update(crate::storage::normalize_path(&paths.data_dir).as_bytes());
    scope.update(b"\0");
    scope.update(token.as_bytes());
    scope.update(b"\0");
    scope.update(value.as_bytes());
    let entropy = scope.finalize().to_vec();
    Some(RemoteArtwork {
        uri: api_url,
        path: paths
            .data_dir
            .join("cache/artwork/private")
            .join(format!("{}.cache", hex::encode(&entropy))),
        legacy: None,
        token: Some(token),
        entropy,
    })
}

async fn load_remote(
    client: &Client,
    paths: &Paths,
    remote: RemoteArtwork,
) -> Result<Option<String>, String> {
    // Coalesce concurrent calls, then re-read the result saved by the first caller.
    let request_lock = {
        let mut locks = REQUESTS.lock().unwrap();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&remote.path).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(tokio::sync::Mutex::new(()));
            locks.insert(remote.path.clone(), Arc::downgrade(&lock));
            lock
        }
    };
    let _guard = request_lock.lock().await;
    if !token_is_current(paths, &remote) {
        return Ok(None);
    }
    let mut record = read_cache(&remote).unwrap_or_default();
    let image = record_image(&record);
    if record.image.is_some() && image.is_none() {
        record = CacheRecord::default();
    }
    let now = timestamp();
    if !should_check(&record, now) {
        return Ok(image);
    }
    record.attempted_at = now;
    let mut request = client
        .get(remote.uri.clone())
        .timeout(Duration::from_secs(10));
    if let Some(token) = &remote.token {
        request = request
            .bearer_auth(token)
            .header(header::ACCEPT, "application/vnd.github.raw+json");
    }
    if image.is_some() {
        if let Some(etag) = &record.etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &record.last_modified {
            request = request.header(header::IF_MODIFIED_SINCE, modified);
        }
    }
    let response = request.send().await.ok();
    let Some(response) = response.filter(|r| r.url() == &remote.uri) else {
        write_cache(paths, &remote, &record);
        return Ok(if token_is_current(paths, &remote) {
            image
        } else {
            None
        });
    };
    if !token_is_current(paths, &remote) {
        return Ok(None);
    }
    if remote.token.is_some()
        && matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        )
    {
        record.image = None;
        record.etag = None;
        record.last_modified = None;
        write_cache(paths, &remote, &record);
        return Ok(None);
    }
    let etag = validator(response.headers(), header::ETAG);
    let modified = validator(response.headers(), header::LAST_MODIFIED);
    if response.status() == StatusCode::NOT_MODIFIED && image.is_some() {
        record.checked_at = timestamp();
        if etag.is_some() {
            record.etag = etag;
        }
        if modified.is_some() {
            record.last_modified = modified;
        }
        write_cache(paths, &remote, &record);
        return Ok(if token_is_current(paths, &remote) {
            image
        } else {
            None
        });
    }
    if response.status() == StatusCode::OK
        && let Some(bytes) = read_limited(response).await
        && let Some(data) = image_data_url(&bytes)
    {
        record.image = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
        record.checked_at = timestamp();
        record.etag = etag;
        record.last_modified = modified;
        write_cache(paths, &remote, &record);
        return Ok(if token_is_current(paths, &remote) {
            Some(data)
        } else {
            None
        });
    }
    write_cache(paths, &remote, &record);
    Ok(if token_is_current(paths, &remote) {
        image
    } else {
        None
    })
}

fn token_is_current(paths: &Paths, remote: &RemoteArtwork) -> bool {
    remote
        .token
        .as_ref()
        .is_none_or(|token| credentials::read_token(paths).ok().flatten().as_ref() == Some(token))
}

fn validator(headers: &header::HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)?
        .to_str()
        .ok()
        .filter(|s| !s.is_empty() && s.len() <= 1024)
        .map(str::to_owned)
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn recent(at: u64, now: u64, duration: Duration) -> bool {
    at != 0
        && now
            .checked_sub(at)
            .is_some_and(|age| age <= duration.as_secs())
}

fn should_check(record: &CacheRecord, now: u64) -> bool {
    !(record.image.is_some() && recent(record.checked_at, now, CACHE_TTL))
        && !recent(record.attempted_at, now, RETRY_INTERVAL)
}

fn artwork_uri(app: &AppDefinition, value: &str) -> Option<(url::Url, String, bool)> {
    if !valid_repo(&app.repository) {
        return None;
    }
    let uri = url::Url::parse(value).ok()?;
    if uri.scheme() != "https"
        || uri.host_str()? != "raw.githubusercontent.com"
        || uri.port().is_some()
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return None;
    }
    let p: Vec<String> = uri.path_segments()?.map(decode_segment).collect();
    if p.len() < 4
        || p.iter().any(|s| {
            s.is_empty()
                || s == "."
                || s == ".."
                || s.contains('/')
                || s.contains('\\')
                || s.bytes().any(|b| b < 0x20 || b == 0x7f)
        })
    {
        return None;
    }
    let mut repo = p[0].clone();
    repo.push('/');
    repo.push_str(&p[1]);
    if !repo.eq_ignore_ascii_case(&app.repository) {
        return None;
    }
    Some((uri, repo, app.is_private))
}
fn valid_repo(s: &str) -> bool {
    let p: Vec<_> = s.split('/').collect();
    p.len() == 2
        && p[0].len() <= 39
        && !p[0].is_empty()
        && p[0]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && p[0].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !p[1].is_empty()
        && p[1].len() <= 100
        && p[1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && p[1] != "."
        && p[1] != ".."
}
fn valid_token(s: &str) -> bool {
    !s.is_empty() && s.len() <= 4096 && s.bytes().all(|b| b.is_ascii_graphic())
}
fn decode_segment(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
async fn read_limited(mut response: reqwest::Response) -> Option<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}
fn image_data_url(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return None;
    }
    let format = image::guess_format(bytes).ok()?;
    let mime = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        _ => return None,
    };
    let dims = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .ok()?;
    let (w, h) = dims;
    if w == 0
        || h == 0
        || w > MAX_DIMENSION
        || h > MAX_DIMENSION
        || u64::from(w) * u64::from(h) > MAX_PIXELS
    {
        return None;
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}
fn cache_path(paths: &Paths, url: &str) -> PathBuf {
    paths.data_dir.join("cache/artwork").join(format!(
        "{}.cache",
        hex::encode(Sha256::digest(url.as_bytes()))
    ))
}
fn legacy_path(paths: &Paths, url: &str) -> PathBuf {
    cache_path(paths, url).with_extension("image")
}

fn read_bounded(path: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

fn read_cache(remote: &RemoteArtwork) -> Option<CacheRecord> {
    let _guard = CACHE_IO.lock().unwrap();
    if let Some(bytes) = read_bounded(&remote.path, MAX_CACHE_BYTES) {
        let plain = if remote.token.is_some() {
            crate::artwork_protection::unprotect(&bytes, &remote.entropy)?
        } else {
            bytes
        };
        if let Ok(record) = serde_json::from_slice::<CacheRecord>(&plain) {
            return Some(record);
        }
    }
    // Keep existing public .image files usable without downloading again after an upgrade.
    let legacy = remote.legacy.as_ref()?;
    let bytes = read_bounded(legacy, MAX_BYTES as u64)?;
    image_data_url(&bytes)?;
    let checked_at = fs::metadata(legacy)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(CacheRecord {
        checked_at,
        attempted_at: checked_at,
        image: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        ..Default::default()
    })
}

fn record_image(record: &CacheRecord) -> Option<String> {
    let encoded = record.image.as_ref()?;
    if encoded.len() > MAX_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    image_data_url(&bytes)
}

fn write_cache(paths: &Paths, remote: &RemoteArtwork, record: &CacheRecord) {
    // The same gate is used by logout/credential replacement so old requests cannot restore deleted private files.
    let _guard = CACHE_IO.lock().unwrap();
    if !token_is_current(paths, remote) {
        return;
    }
    let Ok(plain) = serde_json::to_vec(record) else {
        return;
    };
    let bytes = if remote.token.is_some() {
        let Some(bytes) = crate::artwork_protection::protect(&plain, &remote.entropy) else {
            return;
        };
        bytes
    } else {
        plain
    };
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return;
    }
    if write_atomic_cache(&remote.path, &bytes)
        && let Some(legacy) = &remote.legacy
    {
        let _ = fs::remove_file(legacy);
    }
}

fn write_atomic_cache(path: &Path, bytes: &[u8]) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    if fs::write(&tmp, bytes).is_err() {
        let _ = fs::remove_file(tmp);
        return false;
    }
    if fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(tmp);
        return false;
    }
    let Ok(entries) = fs::read_dir(parent) else {
        return true;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            (m.is_file()
                && matches!(
                    e.path().extension().and_then(|s| s.to_str()),
                    Some("cache" | "image")
                ))
            .then(|| (m.modified().ok(), e.path()))
        })
        .collect();
    if files.len() > MAX_CACHE_FILES {
        files.sort_by_key(|(t, _)| *t);
        for (_, p) in files.into_iter().take(MAX_CACHE_FILES / 4) {
            let _ = fs::remove_file(p);
        }
    }
    true
}

#[cfg(test)]
#[path = "artwork_network_tests.rs"]
mod network_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_urls_are_pinned_to_configured_repository() {
        let app = AppDefinition {
            id: "private-tool".into(),
            name: "Private Tool".into(),
            description: String::new(),
            category: String::new(),
            accent_color: String::new(),
            monogram: String::new(),
            icon_url: None,
            cover_url: None,
            repository: "example-owner/private-tool".into(),
            is_private: true,
            packages: Vec::new(),
        };
        assert!(
            artwork_uri(
                &app,
                "https://raw.githubusercontent.com/example-owner/private-tool/main/icon.png"
            )
            .is_some()
        );
        assert!(
            artwork_uri(
                &app,
                "https://raw.githubusercontent.com/hebory/other/main/icon.png"
            )
            .is_none()
        );
        assert!(artwork_uri(
            &app,
            "https://raw.githubusercontent.com/example-owner/private-tool/main%2f..%2fsecret/icon.png"
        )
        .is_none());
        assert!(artwork_uri(
            &app,
            "https://raw.githubusercontent.com/example-owner/private-tool/main/icon.png?token=secret"
        )
        .is_none());
    }

    #[test]
    fn cache_freshness_and_failure_retry_are_independent() {
        let now = 200_000;
        let mut record = CacheRecord {
            checked_at: now - CACHE_TTL.as_secs(),
            image: Some("fixture".into()),
            ..Default::default()
        };
        assert!(!should_check(&record, now));
        record.checked_at -= 1;
        assert!(should_check(&record, now));
        record.attempted_at = now;
        assert!(!should_check(&record, now + RETRY_INTERVAL.as_secs()));
        assert!(should_check(&record, now + RETRY_INTERVAL.as_secs() + 1));
        // Clock rollback must not indefinitely postpone checking.
        assert!(should_check(&record, 1));
    }
}
