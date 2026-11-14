//! Bounded, redirect-checked GitHub release downloads shared by installers.
use crate::github;
use crate::models::{InstallProgress, ReleaseAsset};
use reqwest::{header, StatusCode, Url};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::{
    io::AsyncWriteExt,
    time::{timeout, Duration},
};

pub(super) const MAX_PACKAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

pub(super) struct Downloaded {
    pub bytes: u64,
    pub sha256: String,
}

fn progress(
    app_id: &str,
    stage: &str,
    bytes: u64,
    total: Option<u64>,
    cb: &(dyn Fn(InstallProgress) + Send + Sync),
) {
    cb(InstallProgress {
        app_id: app_id.to_owned(),
        stage: stage.to_owned(),
        bytes_received: bytes,
        total_bytes: total,
    });
}
fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("작업이 취소되었습니다.".into())
    } else {
        Ok(())
    }
}
fn redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}
fn allowed_uri(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "github.com"
                | "objects.githubusercontent.com"
                | "release-assets.githubusercontent.com"
                | "github-releases.githubusercontent.com"
        )
}

async fn get_anonymous(
    client: &reqwest::Client,
    start: Url,
    dest: &Path,
    max: u64,
    app_id: &str,
    cancel: &AtomicBool,
    cb: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<Downloaded, String> {
    if !allowed_uri(&start) {
        return Err("GitHub 릴리스의 HTTPS 다운로드 주소만 허용됩니다.".into());
    }
    let mut current = start;
    for hop in 0..=MAX_REDIRECTS {
        cancelled(cancel)?;
        let req = client
            .get(current.clone())
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|_| "다운로드 요청을 만들지 못했습니다.")?;
        let response = timeout(Duration::from_secs(600), client.execute(req))
            .await
            .map_err(|_| "다운로드 시간이 초과되었습니다.")?
            .map_err(|_| "릴리스 파일을 다운로드하지 못했습니다.")?;
        if redirect(response.status()) {
            if hop == MAX_REDIRECTS {
                return Err("GitHub 다운로드 리디렉션 횟수가 허용 범위를 초과했습니다.".into());
            }
            let next = response
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| current.join(v).ok())
                .ok_or("GitHub 다운로드 리디렉션 주소가 올바르지 않습니다.")?;
            if !allowed_uri(&next) {
                return Err("검증되지 않은 다운로드 주소로 리디렉션되었습니다.".into());
            }
            current = next;
            continue;
        }
        if response.url() != &current {
            return Err("자동 리디렉션을 사용하지 않는 HTTP 클라이언트가 필요합니다.".into());
        }
        if !response.status().is_success() {
            return Err(format!(
                "릴리스 파일 다운로드에 실패했습니다 (HTTP {}).",
                response.status().as_u16()
            ));
        }
        return save_response(response, dest, max, app_id, cancel, cb).await;
    }
    Err("다운로드를 완료하지 못했습니다.".into())
}

pub(super) async fn download_asset(
    client: &reqwest::Client,
    asset: &ReleaseAsset,
    repository: &str,
    private: bool,
    token: Option<&str>,
    dest: &Path,
    max: u64,
    app_id: &str,
    cancel: &AtomicBool,
    cb: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<Downloaded, String> {
    if !private {
        if !github::validate_asset_url(repository, &asset.download_url) {
            return Err("릴리스 파일이 지정된 GitHub 저장소에서 오지 않았습니다.".into());
        }
        let url =
            Url::parse(&asset.download_url).map_err(|_| "다운로드 주소가 올바르지 않습니다.")?;
        return get_anonymous(client, url, dest, max, app_id, cancel, cb).await;
    }
    let api = asset
        .api_url
        .as_deref()
        .ok_or("비공개 릴리스 파일 API 주소가 없습니다.")?;
    if !github::validate_asset_api_url(repository, api) {
        return Err("비공개 릴리스 파일 API 주소가 지정된 저장소와 일치하지 않습니다.".into());
    }
    let token = token.ok_or("비공개 저장소 다운로드를 위해 GitHub 인증을 설정하세요.")?;
    if token.is_empty() || token.len() > 4096 || token.bytes().any(|c| c <= 0x20 || c >= 0x7f) {
        return Err("GitHub 인증 정보 형식이 올바르지 않습니다.".into());
    }
    cancelled(cancel)?;
    let response = timeout(
        Duration::from_secs(600),
        client
            .get(api)
            .timeout(Duration::from_secs(600))
            .bearer_auth(token)
            .header(header::ACCEPT, "application/octet-stream")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send(),
    )
    .await
    .map_err(|_| "비공개 릴리스 다운로드 시간이 초과되었습니다.".to_string())?
    .map_err(|_| {
        "비공개 릴리스 파일을 다운로드하지 못했습니다. 저장소 권한을 확인하세요.".to_string()
    })?;
    if response.url().as_str() != api {
        return Err("인증된 GitHub 요청이 예상하지 않은 주소로 리디렉션되었습니다.".into());
    }
    if redirect(response.status()) {
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or("비공개 릴리스 다운로드 주소가 없습니다.")?;
        let base = Url::parse(api).map_err(|_| "비공개 자산 API 주소가 올바르지 않습니다.")?;
        let url = base
            .join(location)
            .map_err(|_| "비공개 자산 다운로드 주소가 올바르지 않습니다.")?;
        if !github::allowed_download_url(url.as_str()) || !allowed_uri(&url) {
            return Err("비공개 다운로드가 허용되지 않은 주소로 이동했습니다.".into());
        }
        // The authenticated response is dropped here; CDN request is intentionally anonymous.
        drop(response);
        return get_anonymous(client, url, dest, max, app_id, cancel, cb).await;
    }
    if !response.status().is_success() {
        return Err(
            "비공개 GitHub 릴리스 파일에 접근할 수 없습니다. 인증과 저장소 권한을 확인하세요."
                .into(),
        );
    }
    save_response(response, dest, max, app_id, cancel, cb).await
}

async fn save_response(
    response: reqwest::Response,
    dest: &Path,
    max: u64,
    app_id: &str,
    cancel: &AtomicBool,
    cb: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<Downloaded, String> {
    if response.content_length().is_some_and(|n| n > max) {
        return Err("다운로드 파일이 허용 크기를 초과했습니다.".into());
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| "다운로드 임시 폴더를 만들지 못했습니다.")?;
    }
    let temp = dest.with_extension(format!("{}.part", uuid::Uuid::new_v4().simple()));
    let total = response.content_length();
    let mut response = response;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .await
        .map_err(|_| "임시 다운로드 파일을 만들지 못했습니다.")?;
    let temp_write = temp.clone();
    let dest_write = dest.to_path_buf();
    let result = async move {
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "릴리스 파일 다운로드 중 연결이 끊겼습니다.")?
        {
            cancelled(cancel)?;
            bytes = bytes
                .checked_add(chunk.len() as u64)
                .ok_or("다운로드 파일 크기가 올바르지 않습니다.")?;
            if bytes > max {
                return Err("다운로드 파일이 허용 크기를 초과했습니다.".into());
            }
            hash.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|_| "다운로드 파일을 저장하지 못했습니다.")?;
            progress(app_id, "다운로드", bytes, total, cb);
        }
        file.flush()
            .await
            .map_err(|_| "다운로드 파일을 저장하지 못했습니다.")?;
        drop(file);
        tokio::fs::rename(&temp_write, &dest_write)
            .await
            .map_err(|_| "다운로드 파일을 확정하지 못했습니다.")?;
        Ok(Downloaded {
            bytes,
            sha256: hex::encode(hash.finalize()),
        })
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    result
}

pub(super) fn checksum_matches(text: &str, asset_name: &str) -> Option<String> {
    for line in text.lines() {
        let mut fields = line.trim().split_whitespace();
        let Some(hash) = fields.next() else {
            continue;
        };
        let name = fields.next().unwrap_or("").trim_start_matches('*');
        let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
        if name.eq_ignore_ascii_case(asset_name)
            && hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Some(hash.to_ascii_lowercase());
        }
    }
    let bare = text.trim();
    (bare.len() == 64 && bare.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| bare.to_ascii_lowercase())
}
