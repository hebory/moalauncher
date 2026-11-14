use crate::{
    credentials,
    models::{AppDefinition, AppRelease, Catalog, ReleaseAsset, ReleaseResult},
    storage::Paths,
};
use reqwest::{header, redirect::Policy, Client, StatusCode};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};
use uuid::Uuid;

const API: &str = "https://api.github.com";
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

pub fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent("MoaLauncher/0.3.0")
        .timeout(Duration::from_secs(20))
        .redirect(Policy::none())
        .build()
        .map_err(|_| "GitHub 연결을 준비하지 못했습니다.".to_string())
}

pub async fn latest(
    client: &Client,
    paths: &Paths,
    app: &AppDefinition,
) -> Result<ReleaseResult, String> {
    if !valid_repository(&app.repository) {
        return Err("GitHub 저장소 주소 형식이 올바르지 않습니다.".into());
    }
    let cached = if app.is_private {
        None
    } else {
        read_cache(paths, &app.repository)
    };
    let url = format!("{API}/repos/{}/releases/latest", app.repository);
    let mut req = client
        .get(&url)
        .header(header::ACCEPT, "application/vnd.github+json");
    if app.is_private {
        let token = credentials::read_token(paths)?
            .ok_or_else(|| "비공개 저장소를 확인하려면 GitHub 인증을 설정하세요.".to_string())?;
        validate_token_text(&token)?;
        req = req.bearer_auth(token);
    } else if let Some(cached) = &cached {
        if let Some(etag) = cached.etag.as_deref() {
            req = req.header(header::IF_NONE_MATCH, etag);
        }
    }
    let response = req.send().await.map_err(|_| if app.is_private { "비공개 GitHub 릴리스 정보를 안전하게 가져오지 못했습니다. 네트워크 연결과 저장소 권한을 확인하세요." } else { "GitHub 릴리스 정보를 가져오지 못했습니다." }.to_string());
    let response = match response {
        Ok(response) => response,
        Err(error) => return cached_result(cached, &app.repository).unwrap_or(Err(error)),
    };
    if !response_stays_on_url(&url, response.url()) {
        return Err("GitHub 요청이 검사되지 않은 주소로 이동했습니다.".into());
    }
    if response.status() == StatusCode::NOT_MODIFIED {
        let cached = cached.ok_or_else(|| "저장된 GitHub 릴리스 정보가 없습니다.".to_string())?;
        let release = parse_release(cached.body.as_bytes(), &app.repository, false)?;
        return Ok(ReleaseResult {
            release,
            is_cached: false,
        });
    }
    if app.is_private
        && matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        )
    {
        return Err("GitHub 인증 또는 비공개 저장소 접근 권한을 확인하세요.".into());
    }
    if response.status() == StatusCode::NOT_FOUND {
        return Err("GitHub 저장소에서 안정 버전을 찾을 수 없습니다.".into());
    }
    if !response.status().is_success() {
        return cached_result(cached, &app.repository)
            .unwrap_or_else(|| Err("GitHub 릴리스 조회에 실패했습니다.".into()));
    }
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = read_bounded(response).await?;
    let body = String::from_utf8(body)
        .map_err(|_| "GitHub 릴리스 응답 인코딩이 올바르지 않습니다.".to_string())?;
    let release = parse_release(body.as_bytes(), &app.repository, app.is_private)?;
    if !app.is_private {
        write_cache(paths, &app.repository, &CachedRelease { etag, body });
    }
    Ok(ReleaseResult {
        release,
        is_cached: false,
    })
}

pub async fn verify_token(client: &Client, token: &str) -> Result<String, String> {
    validate_token_text(token)?;
    let response = client
        .get(format!("{API}/user"))
        .bearer_auth(token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| "GitHub 계정을 확인하지 못했습니다.".to_string())?;
    if !response_stays_on_url(&format!("{API}/user"), response.url()) {
        return Err("GitHub 요청이 검사되지 않은 주소로 이동했습니다.".into());
    }
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err("GitHub 인증 정보가 올바르지 않습니다.".into());
    }
    if !response.status().is_success() {
        return Err("GitHub 계정을 확인하지 못했습니다.".into());
    }
    #[derive(Deserialize)]
    struct User {
        login: String,
    }
    let user: User = serde_json::from_slice(&read_bounded(response).await?)
        .map_err(|_| "GitHub 계정 응답 형식이 올바르지 않습니다.")?;
    if user.login.is_empty() || user.login.len() > 39 {
        return Err("GitHub 계정 응답 형식이 올바르지 않습니다.".into());
    }
    Ok(user.login)
}

pub async fn resolve_visibility(
    client: &Client,
    paths: &Paths,
    app: &AppDefinition,
) -> Result<AppDefinition, String> {
    if !valid_repository(&app.repository) {
        return Err("GitHub 저장소 주소 형식이 올바르지 않습니다.".into());
    }
    let mut request = client
        .get(format!("{API}/repos/{}", app.repository))
        .header(header::ACCEPT, "application/vnd.github+json");
    if let Some(token) = credentials::read_token(paths)? {
        validate_token_text(&token)?;
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "GitHub 저장소 공개 여부를 확인하지 못했습니다.".to_string())?;
    if !response_stays_on_url(&format!("{API}/repos/{}", app.repository), response.url()) {
        return Err("GitHub 요청이 검사되지 않은 주소로 이동했습니다.".into());
    }
    if response.status() == StatusCode::NOT_FOUND {
        return Err("GitHub 저장소를 찾을 수 없거나 현재 계정에 접근 권한이 없습니다.".into());
    }
    if !response.status().is_success() {
        return Err(
            "GitHub 저장소 공개 여부를 확인하지 못했습니다. 인증과 저장소 권한을 확인하세요."
                .into(),
        );
    }
    #[derive(Deserialize)]
    struct Repo {
        #[serde(rename = "private")]
        private: bool,
    }
    let repo: Repo = serde_json::from_slice(&read_bounded(response).await?)
        .map_err(|_| "GitHub 저장소 응답 형식이 올바르지 않습니다.")?;
    let mut result = app.clone();
    result.is_private = repo.private;
    Ok(result)
}

pub async fn public_export(client: &Client, apps: &[AppDefinition]) -> Result<Catalog, String> {
    let mut public_apps = Vec::new();
    for app in apps {
        if app.is_private {
            continue;
        }
        if !valid_repository(&app.repository) {
            return Err("GitHub 저장소 주소 형식이 올바르지 않습니다.".into());
        }
        let url = format!("{API}/repos/{}", app.repository);
        let response = client
            .get(&url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|_| "공개 카탈로그에 내보낼 저장소를 확인하지 못했습니다.".to_string())?;
        if !response_stays_on_url(&url, response.url()) || !response.status().is_success() {
            return Err(
                "공개 카탈로그의 저장소 공개 여부를 확인할 수 없어 내보내기를 중단했습니다.".into(),
            );
        }
        #[derive(Deserialize)]
        struct Repo {
            #[serde(rename = "private")]
            private: bool,
        }
        let repo: Repo = serde_json::from_slice(&read_bounded(response).await?)
            .map_err(|_| "GitHub 저장소 응답 형식이 올바르지 않습니다.")?;
        if repo.private {
            return Err(
                "비공개 저장소가 포함되어 있어 공개 카탈로그 내보내기를 중단했습니다.".into(),
            );
        }
        public_apps.push(app.clone());
    }
    let catalog = Catalog {
        schema_version: 1,
        apps: public_apps,
    };
    crate::storage::validate_catalog(&catalog)?;
    Ok(catalog)
}

pub fn validate_asset_url(repository: &str, value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let parts = decoded_segments(&url);
    strict_https_host(&url, "github.com")
        && valid_repository(repository)
        && parts.len() == 6
        && eq(&parts[0], repository.split('/').next().unwrap_or_default())
        && eq(&parts[1], repository.split('/').nth(1).unwrap_or_default())
        && parts[2] == "releases"
        && parts[3] == "download"
        && safe_segment(&parts[4])
        && safe_segment(&parts[5])
}

pub fn validate_asset_api_url(repository: &str, value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let parts = decoded_segments(&url);
    strict_https_host(&url, "api.github.com")
        && valid_repository(repository)
        && parts.len() == 6
        && parts[0] == "repos"
        && eq(&parts[1], repository.split('/').next().unwrap_or_default())
        && eq(&parts[2], repository.split('/').nth(1).unwrap_or_default())
        && parts[3] == "releases"
        && parts[4] == "assets"
        && !parts[5].is_empty()
        && parts[5].bytes().all(|b| b.is_ascii_digit())
        && parts[5] != "0"
}

pub fn allowed_download_url(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if strict_https_host(&url, "github.com") {
        let p = decoded_segments(&url);
        return p.len() == 6
            && valid_repository(&format!("{}/{}", p[0], p[1]))
            && p[2] == "releases"
            && p[3] == "download"
            && p[4..].iter().all(|s| safe_segment(s));
    }
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let allowed = [
        "objects.githubusercontent.com",
        "release-assets.githubusercontent.com",
        "github-releases.githubusercontent.com",
    ]
    .iter()
    .any(|h| host.eq_ignore_ascii_case(h));
    allowed
        && !url.path().is_empty()
        && url
            .path()
            .split('/')
            .all(|s| !s.contains('\\') && !s.bytes().any(|b| b < 0x20 || b == 0x7f))
}

fn strict_https_host(url: &url::Url, host: &str) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|h| h.eq_ignore_ascii_case(host))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}
fn response_stays_on_url(expected: &str, actual: &url::Url) -> bool {
    actual.as_str() == expected
}
fn decoded_segments(url: &url::Url) -> Vec<String> {
    url.path_segments()
        .map(|s| s.map(decode_path_segment).collect())
        .unwrap_or_default()
}
fn decode_path_segment(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex_digit(b[i + 1]), hex_digit(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
fn valid_repository(repo: &str) -> bool {
    let p: Vec<_> = repo.split('/').collect();
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
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && p[1] != "."
        && p[1] != ".."
}
fn safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.bytes().any(|b| b < 0x20 || b == 0x7f)
}
fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}
fn validate_token_text(t: &str) -> Result<(), String> {
    if t.is_empty() || t.len() > 4096 || t.bytes().any(|b| !b.is_ascii_graphic()) {
        return Err("GitHub 인증 정보 형식이 올바르지 않습니다.".into());
    }
    Ok(())
}
async fn read_bounded(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE as u64)
    {
        return Err("GitHub 응답이 허용된 크기보다 큽니다.".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "GitHub 응답을 읽지 못했습니다.".to_string())?
    {
        append_bounded(&mut body, &chunk, MAX_RESPONSE)?;
    }
    Ok(body)
}
fn append_bounded(body: &mut Vec<u8>, chunk: &[u8], maximum: usize) -> Result<(), String> {
    if body.len().saturating_add(chunk.len()) > maximum {
        return Err("GitHub 응답이 허용된 크기보다 큽니다.".into());
    }
    body.extend_from_slice(chunk);
    Ok(())
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    published_at: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ApiAsset>,
}
#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    url: String,
    size: u64,
    digest: Option<String>,
}
fn parse_release(bytes: &[u8], repo: &str, private: bool) -> Result<AppRelease, String> {
    let raw: ApiRelease = serde_json::from_slice(bytes)
        .map_err(|_| "GitHub 릴리스 응답 형식이 올바르지 않습니다.".to_string())?;
    let semver_tag = raw
        .tag_name
        .strip_prefix(['v', 'V'])
        .unwrap_or(&raw.tag_name);
    let ver = semver::Version::parse(semver_tag)
        .map_err(|_| "GitHub 안정 버전 태그가 올바른 SemVer 형식이 아닙니다.".to_string())?;
    if raw.draft || raw.prerelease || !ver.pre.is_empty() {
        return Err("GitHub 안정 버전 릴리스를 찾을 수 없습니다.".into());
    }
    let page = url::Url::parse(&raw.html_url)
        .map_err(|_| "GitHub 릴리스 페이지 주소가 올바르지 않습니다.".to_string())?;
    if !strict_https_host(&page, "github.com")
        || page.path() != format!("/{repo}/releases/tag/{}", raw.tag_name)
    {
        return Err("GitHub 릴리스 페이지 주소가 올바르지 않습니다.".into());
    }
    let published = chrono::DateTime::parse_from_rfc3339(&raw.published_at)
        .map_err(|_| "GitHub 릴리스 게시 시각이 올바르지 않습니다.".to_string())?;
    let mut assets = Vec::with_capacity(raw.assets.len());
    if raw.assets.len() > 1000 {
        return Err("GitHub 릴리스 파일 목록이 너무 큽니다.".into());
    }
    for a in raw.assets {
        if a.name.is_empty()
            || a.name.len() > 255
            || a.name
                .chars()
                .any(|c| c == '/' || c == '\\' || c.is_control())
        {
            return Err("GitHub 릴리스 파일 이름이 올바르지 않습니다.".into());
        }
        if !validate_asset_url(repo, &a.browser_download_url) {
            return Err("GitHub 릴리스 파일 주소가 현재 저장소와 일치하지 않습니다.".into());
        }
        let api_url = if private {
            if !validate_asset_api_url(repo, &a.url) {
                return Err("비공개 릴리스 파일의 API 주소가 저장소와 일치하지 않습니다.".into());
            }
            Some(a.url)
        } else {
            None
        };
        let sha256 = match a.digest {
            None => None,
            Some(s) => {
                let h = s
                    .strip_prefix("sha256:")
                    .ok_or("GitHub SHA-256 형식이 올바르지 않습니다.")?;
                if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("GitHub SHA-256 형식이 올바르지 않습니다.".into());
                }
                Some(h.to_ascii_lowercase())
            }
        };
        assets.push(ReleaseAsset {
            name: a.name,
            download_url: a.browser_download_url,
            size: a.size,
            sha256,
            api_url,
        });
    }
    let tag = raw.tag_name;
    let name = raw
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| tag.clone());
    Ok(AppRelease {
        tag,
        name,
        notes: raw.body.unwrap_or_default().chars().take(100_000).collect(),
        page_url: raw.html_url,
        published_at: published.to_rfc3339(),
        assets,
        is_private: private,
    })
}

#[derive(Deserialize, serde::Serialize)]
struct CachedRelease {
    #[serde(rename = "eTag", alias = "etag", alias = "ETag")]
    etag: Option<String>,
    body: String,
}
fn legacy_cache_path(paths: &Paths, repo: &str) -> PathBuf {
    let h = hex::encode(Sha256::digest(repo.to_ascii_lowercase().as_bytes()));
    paths
        .data_dir
        .join("cache/releases")
        .join(format!("{h}.json"))
}
fn cache_path(paths: &Paths, repo: &str) -> PathBuf {
    let h = hex::encode(Sha256::digest(repo.to_ascii_lowercase().as_bytes()));
    paths
        .data_dir
        .join("cache/releases")
        .join(format!("rust-{h}.json"))
}
fn read_cache(paths: &Paths, repo: &str) -> Option<CachedRelease> {
    for p in [cache_path(paths, repo), legacy_cache_path(paths, repo)] {
        let Ok(b) = fs::read(p) else { continue };
        if b.len() > MAX_RESPONSE * 2 {
            continue;
        }
        if let Ok(c) = serde_json::from_slice(&b) {
            return Some(c);
        }
    }
    None
}
fn write_cache(paths: &Paths, repo: &str, c: &CachedRelease) {
    let p = cache_path(paths, repo);
    if let (Some(parent), Ok(bytes)) = (p.parent(), serde_json::to_vec(c)) {
        let _ = fs::create_dir_all(parent);
        let tmp = p.with_extension(format!("tmp-{}", Uuid::new_v4()));
        if fs::write(&tmp, bytes).is_ok() {
            let _ = fs::rename(tmp, p);
        }
    }
}
fn cached_result(c: Option<CachedRelease>, repo: &str) -> Option<Result<ReleaseResult, String>> {
    let c = c?;
    Some(
        parse_release(c.body.as_bytes(), repo, false).map(|release| ReleaseResult {
            release,
            is_cached: true,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_urls_are_repository_pinned() {
        assert!(validate_asset_url(
            "hebory/moaview",
            "https://github.com/hebory/moaview/releases/download/v1.2.3/MoaView.zip"
        ));
        assert!(!validate_asset_url(
            "hebory/moaview",
            "https://evil.example/hebory/moaview/releases/download/v1.2.3/MoaView.zip"
        ));
        assert!(!validate_asset_url(
            "hebory/moaview",
            "https://github.com/other/moaview/releases/download/v1.2.3/MoaView.zip"
        ));
        assert!(!validate_asset_url(
            "hebory/moaview",
            "https://github.com/hebory/moaview/releases/download/v1.2.3%2f..%2fsecret/a.zip"
        ));
        assert!(validate_asset_api_url(
            "example-owner/private-tool",
            "https://api.github.com/repos/example-owner/private-tool/releases/assets/1234"
        ));
        assert!(!validate_asset_api_url(
            "example-owner/private-tool",
            "https://api.github.com/repos/example-owner/other/releases/assets/1234"
        ));
        assert!(allowed_download_url(
            "https://github.com/hebory/moaview/releases/download/v1/MoaView.zip"
        ));
        assert!(!allowed_download_url(
            "https://github.com/hebory/moaview/releases/download/v1/MoaView.zip?token=secret"
        ));
        assert!(allowed_download_url(
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/file?signature=abc"
        ));
        assert!(!allowed_download_url(
            "https://example.com/github-production-release-asset/1/file"
        ));
    }

    fn release_payload(
        tag: &str,
        draft: bool,
        prerelease: bool,
        asset_url: &str,
        digest: Option<&str>,
    ) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "tag_name": tag,
            "name": "Release",
            "body": "Notes",
            "html_url": format!("https://github.com/hebory/moaview/releases/tag/{tag}"),
            "published_at": "2026-01-02T03:04:05Z",
            "draft": draft,
            "prerelease": prerelease,
            "assets": [{
                "name": "MoaView.zip",
                "browser_download_url": asset_url,
                "url": "https://api.github.com/repos/hebory/moaview/releases/assets/1234",
                "size": 42,
                "digest": digest
            }]
        }))
        .unwrap()
    }

    #[test]
    fn release_parser_accepts_stable_and_checks_release_flags_assets_and_digest() {
        let asset = "https://github.com/hebory/moaview/releases/download/v1.2.3/MoaView.zip";
        let good = release_payload(
            "v1.2.3",
            false,
            false,
            asset,
            Some(&format!("sha256:{}", "a".repeat(64))),
        );
        let parsed = parse_release(&good, "hebory/moaview", false).unwrap();
        assert_eq!(parsed.tag, "v1.2.3");
        assert_eq!(
            parsed.assets[0].sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert!(parse_release(
            &release_payload("v1.2.4-beta.1", false, false, asset, None),
            "hebory/moaview",
            false
        )
        .is_err());
        assert!(parse_release(
            &release_payload("v1.2.3", false, true, asset, None),
            "hebory/moaview",
            false
        )
        .is_err());
        assert!(parse_release(
            &release_payload("v1.2.3", true, false, asset, None),
            "hebory/moaview",
            false
        )
        .is_err());
        let wrong_repo =
            "https://github.com/elsewhere/moaview/releases/download/v1.2.3/MoaView.zip";
        assert!(parse_release(
            &release_payload("v1.2.3", false, false, wrong_repo, None),
            "hebory/moaview",
            false
        )
        .is_err());
        assert!(parse_release(
            &release_payload("v1.2.3", false, false, asset, Some("sha256:not-a-digest")),
            "hebory/moaview",
            false
        )
        .is_err());
    }

    #[test]
    fn unknown_legacy_cache_is_preserved_when_writing_rust_cache() {
        let root = std::env::temp_dir().join(format!("moalauncher-cache-test-{}", Uuid::new_v4()));
        let paths = Paths {
            data_dir: root.clone(),
            local_app_data: root.clone(),
        };
        let legacy = legacy_cache_path(&paths, "hebory/moaview");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let unknown = br#"{"futureCacheShape":{"keep":true}}"#;
        fs::write(&legacy, unknown).unwrap();
        assert!(read_cache(&paths, "hebory/moaview").is_none());
        write_cache(
            &paths,
            "hebory/moaview",
            &CachedRelease {
                etag: None,
                body: "not a release yet".into(),
            },
        );
        assert_eq!(fs::read(legacy).unwrap(), unknown);
        assert!(cache_path(&paths, "hebory/moaview").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn response_body_limit_accepts_boundary_and_rejects_oversize() {
        let mut body = Vec::new();
        let exact = vec![b'x'; MAX_RESPONSE];
        append_bounded(&mut body, &exact, MAX_RESPONSE).unwrap();
        assert_eq!(body.len(), MAX_RESPONSE);
        assert!(append_bounded(&mut body, b"x", MAX_RESPONSE).is_err());
        assert_eq!(body.len(), MAX_RESPONSE);
    }

    #[test]
    fn redirected_api_response_destination_is_rejected() {
        let requested =
            url::Url::parse("https://api.github.com/repos/hebory/moaview/releases/latest").unwrap();
        let same =
            url::Url::parse("https://api.github.com/repos/hebory/moaview/releases/latest").unwrap();
        let redirected = url::Url::parse("https://attacker.example/collect").unwrap();
        assert!(response_stays_on_url(requested.as_str(), &same));
        assert!(!response_stays_on_url(requested.as_str(), &redirected));
    }
}
