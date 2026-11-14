use crate::{credentials, models::AppDefinition, storage::Paths};
use base64::Engine;
use reqwest::{Client, StatusCode, header};
use sha2::{Digest, Sha256};
use std::{fs, io::Cursor, path::PathBuf};
use uuid::Uuid;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
const MAX_PIXELS: u64 = 32_000_000;
const MAX_CACHE_FILES: usize = 64;
const PUBLIC_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

pub async fn load(
    client: &Client,
    paths: &Paths,
    app: &AppDefinition,
    value: &str,
) -> Result<Option<String>, String> {
    let Some((uri, _repo, is_private)) = artwork_uri(app, value) else {
        return Ok(None);
    };
    if !is_private {
        let path = cache_path(paths, value);
        let cached = read_public_cache(&path).and_then(|bytes| {
            let data = image_data_url(&bytes)?;
            Some((data, cache_is_fresh(&path, std::time::SystemTime::now())))
        });
        if let Some((data, true)) = &cached {
            return Ok(Some(data.clone()));
        }
        let response = match client
            .get(uri.as_str())
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Ok(cached_public_fallback(cached)),
        };
        if response.status() != StatusCode::OK || response.url().as_str() != uri.as_str() {
            return Ok(cached_public_fallback(cached));
        }
        let bytes = match read_limited(response).await {
            Some(b) => b,
            None => return Ok(cached_public_fallback(cached)),
        };
        let Some(data) = image_data_url(&bytes) else {
            return Ok(cached_public_fallback(cached));
        };
        write_public_cache(&path, &bytes);
        return Ok(Some(data));
    }

    // Private artwork is requested only from api.github.com with the token. Never cache these bytes.
    let token = match credentials::read_token(paths) {
        Ok(Some(t)) if valid_token(&t) => t,
        _ => return Ok(None),
    };
    let segments: Vec<String> = uri
        .path_segments()
        .map(|p| p.map(decode_segment).collect())
        .unwrap_or_default();
    if segments.len() < 4 {
        return Ok(None);
    }
    let owner = &segments[0];
    let name = &segments[1];
    let reference = &segments[2];
    let mut api_url = url::Url::parse("https://api.github.com/").expect("constant URL is valid");
    {
        let Ok(mut path) = api_url.path_segments_mut() else {
            return Ok(None);
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
    let api_url = api_url.to_string();
    let response = match client
        .get(&api_url)
        .timeout(std::time::Duration::from_secs(10))
        .bearer_auth(&token)
        .header(header::ACCEPT, "application/vnd.github.raw+json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    if response.status() != StatusCode::OK || response.url().as_str() != api_url {
        return Ok(None);
    }
    let Some(bytes) = read_limited(response).await else {
        return Ok(None);
    };
    if credentials::read_token(paths).ok().flatten().as_deref() != Some(token.as_str()) {
        return Ok(None);
    }
    Ok(image_data_url(&bytes))
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
        "{}.image",
        hex::encode(Sha256::digest(url.as_bytes()))
    ))
}
fn read_public_cache(path: &PathBuf) -> Option<Vec<u8>> {
    if fs::metadata(path).ok()?.len() > MAX_BYTES as u64 {
        return None;
    }
    fs::read(path).ok()
}
fn cache_is_fresh(path: &PathBuf, now: std::time::SystemTime) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .is_some_and(|modified| cache_is_fresh_at(modified, now))
}
fn cache_is_fresh_at(modified: std::time::SystemTime, now: std::time::SystemTime) -> bool {
    now.duration_since(modified)
        .is_ok_and(|age| age <= PUBLIC_CACHE_TTL)
}
fn cached_public_fallback(cached: Option<(String, bool)>) -> Option<String> {
    cached.map(|(data, _)| data)
}
fn write_public_cache(path: &PathBuf, bytes: &[u8]) {
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    if fs::write(&tmp, bytes).is_err() {
        return;
    }
    let _ = fs::rename(tmp, path);
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().ok(), e.path()))
        })
        .collect();
    if files.len() > MAX_CACHE_FILES {
        files.sort_by_key(|(t, _)| *t);
        for (_, p) in files.into_iter().take(MAX_CACHE_FILES / 4) {
            let _ = fs::remove_file(p);
        }
    }
}

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
    fn public_cache_freshness_expires_after_five_minutes() {
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
        assert!(cache_is_fresh_at(now - PUBLIC_CACHE_TTL, now));
        assert!(!cache_is_fresh_at(
            now - PUBLIC_CACHE_TTL - std::time::Duration::from_secs(1),
            now
        ));
    }

    #[test]
    fn expired_public_cache_is_kept_as_network_failure_fallback() {
        let stale = Some(("data:image/png;base64,AA==".to_string(), false));
        assert_eq!(
            cached_public_fallback(stale),
            Some("data:image/png;base64,AA==".to_string())
        );
        assert_eq!(cached_public_fallback(None), None);
    }
}
