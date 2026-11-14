# Rust + Tauri 2 이행 계약

작업 브랜치: `codex/rust-tauri2-rewrite`. 기존 WPF 코드는 비교·회귀 확인용으로 유지하고 새 Windows 앱은 `src-tauri`, 화면은 `ui`에 둡니다. 새 앱 버전은 0.3.0입니다.

## 기존 데이터 호환

- 기본 데이터 경로는 `%LOCALAPPDATA%\MoaLauncher`이며 `--data-dir`도 유지합니다.
- `catalog.json`, `settings.json`, `installed/<앱 ID>.json`의 camelCase JSON 필드를 그대로 읽고 씁니다. 구버전 InstalledApp PascalCase도 읽습니다.
- 처음 쓰기 전에 `migration-backup/tauri-v1/`에 기존 설정·카탈로그·설치 상태를 보존합니다. 읽기 실패 파일은 덮어쓰지 않습니다. 알 수 없는 설정 필드는 유지합니다.
- GitHub 자격 증명 대상 이름은 `MoaLauncher/GitHub/` + SHA256(절대 데이터 경로 끝 구분자를 제거하고 대문자로 만든 문자열의 UTF-8)의 소문자 16진수입니다. Generic Credential의 UTF-8 blob을 그대로 사용합니다. 토큰은 화면 응답/로그/파일에 반환하지 않습니다.
- 시작 프로그램 Run 값 이름 `MoaLauncher`, `--background`, 사용자 설치 경로와 실행 파일명 `MoaLauncher.exe`를 유지합니다.
- 설치기는 기존 Inno AppId와 파일명 `MoaLauncher-<버전>-Setup-x64.exe`를 유지해 WPF에서 Rust로 업데이트할 수 있게 합니다.

## Rust 모듈 인터페이스

공통 결과는 `Result<T, String>`. 비공개 요청 오류는 인증 정보·응답 본문을 포함하지 않는 한국어 메시지로 반환합니다.

- `models.rs`: `AppDefinition`, `PackageDefinition`, `Catalog`, `Settings`, `InstalledApp`, `ReleaseAsset`, `AppRelease`, `ReleaseResult`, `InstallProgress` (모두 Clone/Serialize/Deserialize; Rust snake_case ↔ JSON camelCase).
- 모델 필드는 기존 C#와 같으며 문자열 URI·날짜를 사용합니다. `ReleaseAsset { name, download_url, size:u64, sha256:Option<String>, api_url:Option<String> }`, `AppRelease { tag, name, notes, page_url, published_at, assets, is_private }`, `ReleaseResult { release, is_cached }`, `InstallProgress { app_id, stage, bytes_received:u64, total_bytes:Option<u64> }`.
- `storage.rs`: `Paths { data_dir:PathBuf, local_app_data:PathBuf }`, `Paths::new(Option<PathBuf>)`, `load_settings`, `save_settings`, `load_catalog`(기본+개인 합침, 런처 제외), `save_personal_catalog`, `load_personal_catalog`, `save_installed`, `load_installed`, 검증·원자 저장·백업. 각 함수는 `&Paths`를 첫 인자로 받습니다.
- `credentials.rs`: `read_token(&Paths) -> Result<Option<String>,String>`, `save_token(&Paths,&str)`, `delete_token(&Paths)`.
- `github.rs`: `client() -> Result<reqwest::Client,String>`(리디렉션 금지), `latest(client,&Paths,&AppDefinition) -> Result<ReleaseResult,String>`, `verify_token(client,&str) -> Result<String,String>`, `resolve_visibility(client,&Paths,&AppDefinition) -> Result<AppDefinition,String>`, `public_export(client,&[AppDefinition]) -> Result<Catalog,String>`.
- `artwork.rs`: `load(client,&Paths,&AppDefinition,url:&str) -> Result<Option<String>,String>`(data:image base64 응답). 비공개 이미지는 메모리만 사용하며 토큰을 프런트엔드에 전달하지 않습니다.
- `install.rs`: `find_installed(&Paths,&AppDefinition) -> Result<Option<InstalledApp>,String>`, `is_running(&InstalledApp)->bool`, `launch(&InstalledApp)`, `install(client,&Paths,&AppDefinition,&AppRelease,cancel:Arc<AtomicBool>,progress:Arc<dyn Fn(InstallProgress)+Send+Sync>) -> Result<InstalledApp,String>`.
- `download.rs`: GitHub 다운로드·체크섬 검증·취소·크기 제한 공통 처리.
- `self_update.rs`: `prepare(client,&Paths,&AppRelease,cancel,progress) -> Result<PreparedUpdate,String>`, `handoff(&Paths,&PreparedUpdate,background:bool)->Result<(),String>`, `run_helper(plan:&Path)->Result<(),String>`. handoff 성공 후 런타임이 앱 종료. helper 실행 전·부모 종료 후 해시 재확인, 파일 백업/실패 복구/동일 경로 재시작.
- `platform.rs`, `runtime.rs`, `lib.rs`, `main.rs`: Windows 시작 등록·단일 인스턴스, 트레이·알림 클릭 및 Tauri 명령·작업 상태·주기 확인 관리.

## 경계

- 공개 릴리스·이미지 요청은 익명입니다. 비공개 요청에만 `api.github.com`으로 토큰을 보냅니다. 자산 CDN에는 토큰을 보내지 않습니다.
- ZIP 경로·링크·압축 크기, 설치 경로, 버전·SHA256·실행 중 여부를 검증합니다. 임의 명령/임의 URL/임의 파일 읽기를 UI에 노출하지 않습니다.
- 모두 업데이트는 일반 앱을 순서대로 처리하고 런처를 마지막에 처리합니다. 실패·취소 시 남은 작업을 멈춥니다.
- 원격 HTML은 실행하지 않습니다. UI는 로컬 번들/CSP/최소 권한을 사용하며 릴리스 본문은 텍스트로 표시합니다.
