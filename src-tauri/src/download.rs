//! Bounded, redirect-checked GitHub release downloads shared by installers.
use crate::github;
use crate::models::{InstallProgress, ReleaseAsset};
use reqwest::{header, StatusCode, Url};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::{io::AsyncWriteExt, time::Duration};

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
async fn await_or_cancel<T, F>(
    cancel: &AtomicBool,
    future: F,
    failure: &'static str,
) -> Result<T, String>
where
    F: Future<Output = Result<T, reqwest::Error>>,
{
    tokio::pin!(future);
    loop {
        tokio::select! {
            response = &mut future => return response.map_err(|_| failure.to_owned()),
            _ = tokio::time::sleep(Duration::from_millis(50)) => cancelled(cancel)?,
        }
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
pub(super) fn allowed_uri(url: &Url) -> bool {
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
        let response = await_or_cancel(
            cancel,
            client.execute(req),
            "릴리스 파일 다운로드가 중단되었습니다.",
        )
        .await?;
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

// The security boundary parameters stay explicit so callers cannot accidentally infer trust.
#[allow(clippy::too_many_arguments)]
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
    let response = await_or_cancel(
        cancel,
        client
            .get(api)
            .timeout(Duration::from_secs(600))
            .bearer_auth(token)
            .header(header::ACCEPT, "application/octet-stream")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send(),
        "비공개 릴리스 다운로드가 중단되었습니다.",
    )
    .await?;
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
        while let Some(chunk) = await_or_cancel(
            cancel,
            response.chunk(),
            "릴리스 파일 다운로드 중 연결이 끊겼습니다.",
        )
        .await?
        {
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
        let mut fields = line.split_whitespace();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        credentials, github, install,
        models::ReleaseResult,
        models::{AppDefinition, AppRelease},
        storage::{self, Paths},
    };
    use std::path::PathBuf;

    async fn download_verified_release_asset(
        client: &reqwest::Client,
        app: &AppDefinition,
        release: &AppRelease,
        token: Option<&str>,
        destination: &Path,
    ) -> Result<u64, String> {
        let package = install::package_for_arch(app, "x64")?;
        let asset = install::select_asset(release, package)?;
        let cancel = AtomicBool::new(false);
        let callback = |_progress: InstallProgress| {};
        let downloaded = download_asset(
            client,
            asset,
            &app.repository,
            app.is_private,
            token,
            destination,
            MAX_PACKAGE_BYTES,
            &app.id,
            &cancel,
            &callback,
        )
        .await?;
        if asset.size > 0 && downloaded.bytes != asset.size {
            return Err("릴리스 자산 크기 검증에 실패했습니다.".into());
        }

        let expected = if let Some(hash) = asset
            .sha256
            .as_deref()
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            hash.to_ascii_lowercase()
        } else {
            let checksum = release
                .assets
                .iter()
                .find(|candidate| {
                    candidate
                        .name
                        .eq_ignore_ascii_case(&(asset.name.clone() + ".sha256"))
                })
                .or_else(|| {
                    release
                        .assets
                        .iter()
                        .find(|candidate| candidate.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
                })
                .ok_or("릴리스에 SHA-256 체크섬이 없습니다.")?;
            let checksum_path = destination.with_extension("checksum");
            download_asset(
                client,
                checksum,
                &app.repository,
                app.is_private,
                token,
                &checksum_path,
                1024 * 1024,
                &app.id,
                &cancel,
                &callback,
            )
            .await?;
            let checksum_text = tokio::fs::read_to_string(&checksum_path)
                .await
                .map_err(|_| "SHA-256 체크섬 파일을 읽지 못했습니다.")?;
            let result = checksum_matches(&checksum_text, &asset.name)
                .ok_or("릴리스 SHA-256 체크섬과 자산 이름이 일치하지 않습니다.");
            let _ = tokio::fs::remove_file(checksum_path).await;
            result?
        };
        if !constant_hash_eq(&downloaded.sha256, &expected) {
            return Err("다운로드 파일 SHA-256 검증에 실패했습니다.".into());
        }
        Ok(downloaded.bytes)
    }

    fn constant_hash_eq(actual: &str, expected: &str) -> bool {
        let (Ok(actual), Ok(expected)) = (hex::decode(actual), hex::decode(expected)) else {
            return false;
        };
        actual.len() == expected.len()
            && actual
                .iter()
                .zip(expected)
                .fold(0u8, |difference, (a, b)| difference | (a ^ b))
                == 0
    }

    #[test]
    fn checksum_parser_ignores_empty_lines_and_requires_matching_name() {
        let hash = "a".repeat(64);
        assert_eq!(
            checksum_matches(&format!("\n{hash} *MoaView.zip\n"), "moaview.ZIP"),
            Some(hash.clone())
        );
        assert_eq!(
            checksum_matches(&format!("{hash} other.zip\n"), "moaview.zip"),
            None
        );
        assert_eq!(checksum_matches("not-a-checksum", "MoaView.zip"), None);
    }

    #[test]
    fn redirect_allowlist_accepts_github_cdn_only() {
        for url in [
            "https://github.com/owner/repo/releases/download/v1/file.zip",
            "https://objects.githubusercontent.com/release/item?sig=abc",
            "https://release-assets.githubusercontent.com/file",
            "https://github-releases.githubusercontent.com/file",
        ] {
            assert!(allowed_uri(&Url::parse(url).unwrap()), "{url}");
        }
        for url in [
            "http://github.com/owner/repo/releases/download/v1/file.zip",
            "https://evil.example/file",
            "https://user@github.com/owner/repo/releases/download/v1/file.zip",
            "https://github.com:444/owner/repo/releases/download/v1/file.zip",
        ] {
            assert!(!allowed_uri(&Url::parse(url).unwrap()), "{url}");
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_stalled_request() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            signal.store(true, Ordering::SeqCst);
        });
        let result = await_or_cancel(
            &cancel,
            std::future::pending::<Result<(), reqwest::Error>>(),
            "취소됨",
        )
        .await;
        assert_eq!(result.unwrap_err(), "작업이 취소되었습니다.");
    }

    #[tokio::test]
    async fn cancelling_stream_removes_partial_download() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n").unwrap();
            stream.flush().unwrap();
            thread::sleep(Duration::from_millis(400));
            let _ = stream.write_all(b"3\r\ndef\r\n0\r\n\r\n");
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = client
            .get(format!("http://{addr}/fixture"))
            .send()
            .await
            .unwrap();
        let temp_root =
            std::env::temp_dir().join(format!("moa-cancel-{}", uuid::Uuid::new_v4().simple()));
        tokio::fs::create_dir_all(&temp_root).await.unwrap();
        let dest = temp_root.join("fixture.zip");
        let cancel = AtomicBool::new(false);
        let callback = |p: InstallProgress| {
            if p.bytes_received > 0 {
                cancel.store(true, Ordering::SeqCst);
            }
        };
        let result = save_response(response, &dest, 1024, "fixture", &cancel, &callback).await;
        assert!(result.is_err());
        assert!(!dest.exists());
        assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);
        server.join().unwrap();
        tokio::fs::remove_dir(temp_root).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "읽기 전용 실프로필 검증으로 GitHub 인증과 네트워크가 필요합니다. 다운로드 파일은 실행하지 않습니다."]
    async fn live_public_and_private_assets_verify() {
        let profile = Paths::new(None);
        let catalog = storage::load_catalog(&profile).expect("기본 카탈로그를 읽어야 합니다.");
        let personal =
            storage::load_personal_catalog(&profile).expect("개인 카탈로그를 읽어야 합니다.");
        let public_app = catalog
            .apps
            .into_iter()
            .find(|app| app.repository.eq_ignore_ascii_case("hebory/moaview") && !app.is_private)
            .expect("공개 모아뷰 항목이 기본 카탈로그에 있어야 합니다.");
        let private_app = personal
            .apps
            .into_iter()
            .find(|app| app.is_private)
            .expect("개인 카탈로그에 비공개 앱이 있어야 합니다.");
        let token = credentials::read_token(&profile)
            .expect("GitHub 인증 정보 읽기에 실패했습니다.")
            .expect("GitHub 인증 정보가 필요합니다.");
        let client = github::client().expect("GitHub 클라이언트를 만들어야 합니다.");

        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("저장소 경로를 확인해야 합니다.")
            .to_path_buf();
        let validation_root = workspace
            .join("artifacts")
            .join("rust-download-validation")
            .join(format!("run-{}", uuid::Uuid::new_v4().simple()));
        let public_paths = Paths {
            data_dir: validation_root.join("public-cache"),
            local_app_data: profile.local_app_data.clone(),
        };

        // Public release metadata is cached only under the isolated validation folder.
        let ReleaseResult {
            release: public_release,
            is_cached: public_cached,
        } = github::latest(&client, &public_paths, &public_app)
            .await
            .expect("공개 릴리스 정보를 읽어야 합니다.");
        assert!(!public_cached, "공개 릴리스 정보를 새로 확인해야 합니다.");
        let public_bytes = download_verified_release_asset(
            &client,
            &public_app,
            &public_release,
            None,
            &validation_root.join("public.asset"),
        )
        .await
        .expect("공개 릴리스 자산 다운로드와 SHA-256 검증에 실패했습니다.");

        // Private release metadata uses the profile credential but does not write to profile storage.
        let ReleaseResult {
            release: private_release,
            is_cached: private_cached,
        } = github::latest(&client, &profile, &private_app)
            .await
            .expect("비공개 릴리스 정보를 읽어야 합니다.");
        assert!(
            !private_cached,
            "비공개 릴리스 정보를 새로 확인해야 합니다."
        );
        let private_bytes = download_verified_release_asset(
            &client,
            &private_app,
            &private_release,
            Some(&token),
            &validation_root.join("private.asset"),
        )
        .await
        .expect("비공개 릴리스 자산 다운로드와 SHA-256 검증에 실패했습니다.");

        println!("public_sha256_verified=true bytes={public_bytes}");
        println!("private_sha256_verified=true bytes={private_bytes}");
    }
}
