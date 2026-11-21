//! Offline HTTP and cache lifecycle tests for artwork loading.

use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

struct Profile(PathBuf);

impl Profile {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("moa-artwork-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn paths(&self) -> Paths {
        Paths {
            data_dir: self.0.clone(),
            local_app_data: self.0.clone(),
        }
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct MockServer {
    url: String,
    requests: Arc<AtomicUsize>,
    request_headers: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl MockServer {
    fn new(status: &str, extra_headers: &str, body: &[u8], delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/icon.png", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let request_headers = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let requests = requests.clone();
            let request_headers = request_headers.clone();
            let stop = stop.clone();
            let body = body.to_vec();
            let status = status.to_owned();
            let extra_headers = extra_headers.to_owned();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            // Accepted Winsock sockets can inherit the listener's nonblocking mode.
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            requests.fetch_add(1, Ordering::SeqCst);
                            read_request_headers(&mut stream, &request_headers);
                            if !delay.is_zero() {
                                thread::sleep(delay);
                            }
                            let response = format!(
                                "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let mut response = response.into_bytes();
                            response.extend_from_slice(&body);
                            stream.write_all(&response).unwrap();
                            stream.flush().unwrap();
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Self {
            url,
            requests,
            request_headers,
            stop,
            worker: Some(worker),
        }
    }

    fn count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    fn wait_for(&self, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while self.count() < expected && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            self.count(),
            expected,
            "unexpected local HTTP request count"
        );
    }

    fn wait_quiet(&self) {
        thread::sleep(Duration::from_millis(80));
    }

    fn headers(&self) -> String {
        self.request_headers.lock().unwrap().join("\n")
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            if !thread::panicking() {
                worker.join().unwrap();
            } else {
                let _ = worker.join();
            }
        }
    }
}

fn read_request_headers(stream: &mut TcpStream, sink: &Mutex<Vec<String>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut bytes = Vec::new();
    let mut chunk = [0; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") && bytes.len() < 16 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
        }
    }
    sink.lock()
        .unwrap()
        .push(String::from_utf8_lossy(&bytes).to_ascii_lowercase());
}

fn png(color: [u8; 4]) -> Vec<u8> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&color, 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes
}

fn app() -> AppDefinition {
    AppDefinition {
        id: "artwork-fixture".into(),
        name: "Artwork fixture".into(),
        description: String::new(),
        category: String::new(),
        accent_color: String::new(),
        monogram: "A".into(),
        icon_url: None,
        cover_url: None,
        repository: "example-owner/artwork-fixture".into(),
        is_private: false,
        packages: Vec::new(),
    }
}

fn remote(paths: &Paths, server: &MockServer) -> RemoteArtwork {
    let mut remote = remote_artwork(
        paths,
        &app(),
        "https://raw.githubusercontent.com/example-owner/artwork-fixture/main/icon.png",
    )
    .unwrap();
    remote.uri = url::Url::parse(&server.url).unwrap();
    remote
}

fn image_record(bytes: &[u8], checked_at: u64, attempted_at: u64) -> CacheRecord {
    CacheRecord {
        checked_at,
        attempted_at,
        image: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        ..Default::default()
    }
}

fn stale_time() -> u64 {
    timestamp().saturating_sub(CACHE_TTL.as_secs() + 60)
}

#[tokio::test]
async fn fresh_public_cache_is_returned_without_http() {
    let profile = Profile::new();
    let paths = profile.paths();
    let image = png([12, 34, 56, 255]);
    let server = MockServer::new("200 OK", "", &png([200, 0, 0, 255]), Duration::ZERO);
    let remote = remote(&paths, &server);
    write_cache(
        &paths,
        &remote,
        &image_record(&image, timestamp(), timestamp()),
    );

    let cached_first = cached(
        &paths,
        &app(),
        "https://raw.githubusercontent.com/example-owner/artwork-fixture/main/icon.png",
    );
    assert_eq!(cached_first.as_deref(), image_data_url(&image).as_deref());
    let result = load_remote(&Client::new(), &paths, remote).await.unwrap();

    assert_eq!(result.as_deref(), image_data_url(&image).as_deref());
    server.wait_quiet();
    assert_eq!(server.count(), 0);
}

#[tokio::test]
async fn expired_cache_sends_etag_and_not_modified_refreshes_ttl() {
    let profile = Profile::new();
    let paths = profile.paths();
    let image = png([12, 34, 56, 255]);
    let server = MockServer::new(
        "304 Not Modified",
        "ETag: \"fixture-v1\"\r\n",
        &[],
        Duration::ZERO,
    );
    let remote = remote(&paths, &server);
    let mut record = image_record(&image, stale_time(), stale_time());
    record.etag = Some("\"fixture-v1\"".into());
    write_cache(&paths, &remote, &record);

    assert_eq!(
        cached(
            &paths,
            &app(),
            "https://raw.githubusercontent.com/example-owner/artwork-fixture/main/icon.png",
        )
        .as_deref(),
        image_data_url(&image).as_deref()
    );
    let before = read_cache(&remote).unwrap().checked_at;
    assert_eq!(
        load_remote(&Client::new(), &paths, remote.clone_for_test())
            .await
            .unwrap()
            .as_deref(),
        image_data_url(&image).as_deref()
    );
    server.wait_for(1);
    assert!(server.headers().contains("if-none-match: \"fixture-v1\""));
    let refreshed = read_cache(&remote).unwrap();
    assert!(refreshed.checked_at >= before);

    let second = remote.clone_for_test();
    assert!(
        load_remote(&Client::new(), &paths, second)
            .await
            .unwrap()
            .is_some()
    );
    server.wait_quiet();
    assert_eq!(server.count(), 1);
}

#[tokio::test]
async fn changed_response_replaces_the_cached_image() {
    let profile = Profile::new();
    let paths = profile.paths();
    let old = png([12, 34, 56, 255]);
    let new = png([98, 76, 54, 255]);
    let server = MockServer::new("200 OK", "ETag: \"fixture-v2\"\r\n", &new, Duration::ZERO);
    let remote = remote(&paths, &server);
    let mut record = image_record(&old, stale_time(), stale_time());
    record.etag = Some("\"fixture-v1\"".into());
    write_cache(&paths, &remote, &record);

    assert_eq!(
        cached(
            &paths,
            &app(),
            "https://raw.githubusercontent.com/example-owner/artwork-fixture/main/icon.png",
        )
        .as_deref(),
        image_data_url(&old).as_deref()
    );
    let result = load_remote(&Client::new(), &paths, remote.clone_for_test())
        .await
        .unwrap();

    assert_eq!(result.as_deref(), image_data_url(&new).as_deref());
    assert_eq!(
        record_image(&read_cache(&remote).unwrap()).as_deref(),
        image_data_url(&new).as_deref()
    );
    assert!(server.headers().contains("if-none-match: \"fixture-v1\""));
}

#[tokio::test]
async fn failure_uses_stale_image_and_suppresses_retries_for_one_hour() {
    let profile = Profile::new();
    let paths = profile.paths();
    let image = png([12, 34, 56, 255]);
    let server = MockServer::new("503 Service Unavailable", "", b"offline", Duration::ZERO);
    let remote = remote(&paths, &server);
    write_cache(
        &paths,
        &remote,
        &image_record(&image, stale_time(), stale_time()),
    );

    let first = load_remote(&Client::new(), &paths, remote.clone_for_test())
        .await
        .unwrap();
    assert_eq!(first.as_deref(), image_data_url(&image).as_deref());
    server.wait_for(1);
    let attempted = read_cache(&remote).unwrap().attempted_at;
    assert!(attempted > 0);
    let second = load_remote(&Client::new(), &paths, remote).await.unwrap();
    assert_eq!(second.as_deref(), image_data_url(&image).as_deref());
    server.wait_quiet();
    assert_eq!(server.count(), 1);
}

#[tokio::test]
async fn failed_cache_without_image_records_negative_retry_window() {
    let profile = Profile::new();
    let paths = profile.paths();
    let server = MockServer::new("503 Service Unavailable", "", b"offline", Duration::ZERO);
    let remote = remote(&paths, &server);

    assert!(
        load_remote(&Client::new(), &paths, remote.clone_for_test())
            .await
            .unwrap()
            .is_none()
    );
    server.wait_for(1);
    assert!(read_cache(&remote).unwrap().attempted_at > 0);
    assert!(
        load_remote(&Client::new(), &paths, remote)
            .await
            .unwrap()
            .is_none()
    );
    server.wait_quiet();
    assert_eq!(server.count(), 1);
}

#[tokio::test]
async fn concurrent_loads_of_the_same_cache_use_one_request() {
    let profile = Profile::new();
    let paths = profile.paths();
    let image = png([12, 34, 56, 255]);
    let server = MockServer::new("200 OK", "", &image, Duration::from_millis(120));
    let client = Client::new();
    let first = remote(&paths, &server);
    let second = remote(&paths, &server);

    let (a, b) = tokio::join!(
        load_remote(&client, &paths, first),
        load_remote(&client, &paths, second)
    );
    assert_eq!(a.unwrap().as_deref(), image_data_url(&image).as_deref());
    assert_eq!(b.unwrap().as_deref(), image_data_url(&image).as_deref());
    server.wait_for(1);
    assert_eq!(server.count(), 1);
}

#[test]
fn legacy_image_cache_is_available_immediately_without_network() {
    let profile = Profile::new();
    let paths = profile.paths();
    let image = png([12, 34, 56, 255]);
    let remote = remote_artwork(
        &paths,
        &app(),
        "https://raw.githubusercontent.com/example-owner/artwork-fixture/main/icon.png",
    )
    .unwrap();
    fs::create_dir_all(remote.legacy.as_ref().unwrap().parent().unwrap()).unwrap();
    fs::write(remote.legacy.as_ref().unwrap(), &image).unwrap();

    let shown = cached(
        &paths,
        &app(),
        "https://raw.githubusercontent.com/example-owner/artwork-fixture/main/icon.png",
    );
    assert_eq!(shown.as_deref(), image_data_url(&image).as_deref());
}

#[tokio::test]
async fn corrupt_cache_is_replaced_by_a_valid_local_response() {
    let profile = Profile::new();
    let paths = profile.paths();
    let image = png([12, 34, 56, 255]);
    let server = MockServer::new("200 OK", "", &image, Duration::ZERO);
    let remote = remote(&paths, &server);
    fs::create_dir_all(remote.path.parent().unwrap()).unwrap();
    fs::write(&remote.path, b"not a cache record").unwrap();

    let result = load_remote(&Client::new(), &paths, remote.clone_for_test())
        .await
        .unwrap();

    assert_eq!(result.as_deref(), image_data_url(&image).as_deref());
    server.wait_for(1);
    assert_eq!(
        record_image(&read_cache(&remote).unwrap()).as_deref(),
        image_data_url(&image).as_deref()
    );
}

#[cfg(windows)]
#[test]
fn private_disk_cache_is_encrypted_profile_scoped_and_removable() {
    let profile = Profile::new();
    let paths = profile.paths();
    let path = paths.data_dir.join("cache/artwork/private/fixture.cache");
    let public = paths.data_dir.join("cache/artwork/public.cache");
    let entropy = b"private-artwork-profile-and-url-scope".to_vec();
    let image = png([12, 34, 56, 255]);
    let record = image_record(&image, timestamp(), timestamp());
    let plaintext = serde_json::to_vec(&record).unwrap();
    let ciphertext = crate::artwork_protection::protect(&plaintext, &entropy).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &ciphertext).unwrap();
    fs::write(&public, b"public cache fixture").unwrap();
    let remote = RemoteArtwork {
        uri: url::Url::parse(
            "https://api.github.com/repos/example-owner/private/contents/icon.png",
        )
        .unwrap(),
        path: path.clone(),
        legacy: None,
        token: Some("test-only-placeholder-token".into()),
        entropy,
    };

    assert!(
        !ciphertext
            .windows(plaintext.len())
            .any(|window| window == plaintext)
    );
    assert_eq!(
        record_image(&read_cache(&remote).unwrap()).as_deref(),
        image_data_url(&image).as_deref()
    );
    clear_private_cache(&paths);
    assert!(!path.exists());
    assert!(public.exists());
}

trait CloneRemoteArtwork {
    fn clone_for_test(&self) -> RemoteArtwork;
}

impl CloneRemoteArtwork for RemoteArtwork {
    fn clone_for_test(&self) -> RemoteArtwork {
        RemoteArtwork {
            uri: self.uri.clone(),
            path: self.path.clone(),
            legacy: self.legacy.clone(),
            token: self.token.clone(),
            entropy: self.entropy.clone(),
        }
    }
}
