# Rust 네이티브 의존성 보안 감사

검토일: 2026-11-15

## Cargo.lock 감사

`native/Cargo.lock`의 **462개 패키지 항목**을 RustSec 공식 [`cargo-audit` 0.23.0](https://github.com/rustsec/rustsec/releases/tag/cargo-audit/v0.23.0)으로 검사했습니다. 사용한 공식 [RustSec Advisory Database](https://github.com/RustSec/advisory-db)는 커밋 `4ef2df8e806a65e2f1ecc14f56f51249062b23d6` (2026-11-13)입니다.

- 알려진 취약점: **0건**
- 미유지보수 경고: **3건** — `zbus_names 4.3.5` (RUSTSEC-2026-0371), `zvariant 5.15.1` (RUSTSEC-2026-0370), `zvariant_derive 5.15.1` (RUSTSEC-2026-0372)
- Windows x64 대상 의존성 그래프에서 위 3개 crate는 모두 제외됩니다. `cargo tree --locked --manifest-path native/Cargo.toml --target x86_64-pc-windows-msvc -i <crate>` 조회 결과에 의존 경로가 없었습니다.
- Cargo.lock에는 Tauri, tauri-build, Tauri 플러그인, wry, webview2-com 또는 tokio-tauri 패키지가 없습니다.

잠금 파일 SHA-256: `f3d4676ca7dd849910a45a4171516d82449f8da08d44bdbffe00d422980c897d`

재현 명령:

```powershell
cargo-audit.exe audit --file native/Cargo.lock --db <RustSec advisory-db 경로> --no-fetch --no-yanked
cargo tree --locked --manifest-path native/Cargo.toml --target x86_64-pc-windows-msvc -i zbus_names
cargo tree --locked --manifest-path native/Cargo.toml --target x86_64-pc-windows-msvc -i zvariant
cargo tree --locked --manifest-path native/Cargo.toml --target x86_64-pc-windows-msvc -i zvariant_derive
```

감사는 Cargo 잠금 파일의 의존성 advisory와 미유지보수 경고를 대상으로 했습니다. `--no-yanked`로 crates.io yanked 상태는 확인하지 않았으며, 이 결과는 애플리케이션 코드에 대한 보안 검토를 뜻하지 않습니다.

## 구현 검증

- `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings`, Rust 단위 테스트 **40개**를 통과했습니다. 실제 네트워크·설치기가 필요한 두 검사는 기본 실행에서 무시하며 별도로 실행해 통과했습니다.
- 실제 기존 인증으로 공개·비공개 릴리스와 아이콘·커버를 읽었습니다. 설치 자산 69,316,124바이트(공개)와 3,258,546바이트(비공개)를 격리 폴더에 다운로드해 SHA-256을 확인했고 실행하지 않았습니다.
- NSIS·Inno는 공백 포함 격리 경로에서 설치, 동일 버전 재설치, 업데이트, 실패 후 상태 보존을 확인했습니다.
- 런처 helper는 정상 업데이트, 부분 설치 후 실패, 설치 버전 불일치 세 시나리오를 통과했습니다. 실패 시 기존 실행 파일과 대상 파일 해시를 복구하고 사용자 상태를 유지하며 재시작했습니다.
- 실제 Rust GUI에서 한글, 설치 버전·이미지, 즐겨찾기 필터, 그리드·리스트 설정 복원과 24시간 확인 간격 저장을 확인했습니다. 실행 중인 네이티브 프로세스의 WebView 관련 모듈은 0개였습니다.
- `Test-NativeLifecycle.ps1`은 `--background` 시작 동안 주 창이 없고, 같은 프로필로 다시 실행하면 기존 프로세스가 창을 생성하는 것을 확인합니다. 배경 시작에는 eframe 창과 GPU 컨텍스트를 만들지 않으므로 첫 프레임의 창 표시도 발생하지 않습니다.
- 원본 사용자 프로필·GitHub 자격 증명·기존 PC 설치본은 수정하지 않았습니다. GitHub Release 게시도 진행하지 않았습니다.

```powershell
cargo test --locked --manifest-path native/Cargo.toml
cargo test --locked --manifest-path native/Cargo.toml live_public_and_private_assets_verify -- --ignored --nocapture
./scripts/Test-RustInstall.ps1 -MakensisPath <makensis.exe> -IsccPath <ISCC.exe>
./scripts/Test-RustSelfUpdate.ps1
./scripts/Test-NativeLifecycle.ps1 -Executable native/target/release/MoaLauncher.exe
```

Windows 로그인 전체 과정, 실제 새 릴리스 알림 클릭, Explorer 프로세스 재시작은 자동화 시험에서 재현하지 않았습니다. 시작 등록 명령·클릭 이벤트 연결·Explorer broadcast 복구는 코드 검토와 관련 상태 테스트로 확인했습니다.
