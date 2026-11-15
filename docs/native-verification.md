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
- 위 개발 단계 검증에서는 원본 사용자 프로필·GitHub 자격 증명·기존 PC 설치본을 수정하거나 GitHub Release를 게시하지 않았습니다. 이후 공식 배포와 PC 업데이트 결과는 아래에 기록합니다.

```powershell
cargo test --locked --manifest-path native/Cargo.toml
cargo test --locked --manifest-path native/Cargo.toml live_public_and_private_assets_verify -- --ignored --nocapture
./scripts/Test-RustInstall.ps1 -MakensisPath <makensis.exe> -IsccPath <ISCC.exe>
./scripts/Test-RustSelfUpdate.ps1
./scripts/Test-NativeLifecycle.ps1 -Executable native/target/release/MoaLauncher.exe
```

Windows 로그인 전체 과정, 실제 새 릴리스 알림 클릭, Explorer 프로세스 재시작은 자동화 시험에서 재현하지 않았습니다. 시작 등록 명령·클릭 이벤트 연결·Explorer broadcast 복구는 코드 검토와 관련 상태 테스트로 확인했습니다.

## 0.3.1 공식 배포와 PC 업데이트

2026-11-15에 순수 Rust 버전 [v0.3.1](https://github.com/hebory/moalauncher/releases/tag/v0.3.1)을 게시하고 기존 WPF 0.2.5 설치본을 업데이트했습니다. 태그의 소스 커밋은 `f15e90e5e357a6bb0acccffb9c3bfd9c452c7d73`이며 개발 브랜치는 `codex/rust-native-rewrite`입니다.

- [GitHub Actions 실행](https://github.com/hebory/moalauncher/actions/runs/43657882986)의 `build` 작업에서 형식 검사, Clippy, 단위 테스트 40개, Windows 빌드와 Inno 패키징이 통과했습니다. 기본 실행에서 무시되는 실제 설치·네트워크 검사 2개는 위 개발 단계에서 별도로 통과했습니다.
- 최초 패키징에서 설치 컴파일러 후보가 하나일 때 PowerShell이 배열을 문자열로 축약해 경로 첫 글자를 선택하는 문제를 수정했습니다. 워크플로와 배포 스크립트 모두 공백이 있는 단일 컴파일러 경로로 재현 검증했습니다.
- 자동 게시 작업은 `gh release view`의 예상된 종료 코드 1(기존 릴리스 없음)을 Actions PowerShell 래퍼에 전달해 실패했습니다. 성공한 동일 `build` 작업의 원본 artifact를 내려받아 API digest와 태그 커밋을 확인한 뒤, 다섯 자산을 draft에 업로드하고 digest를 대조한 후 공개했습니다. 후속 자동화는 정상 종료 코드로 초기화하도록 수정했고, 기존 릴리스 있음·없음·예상 밖 오류의 세 분기를 검증했습니다. 기존 태그나 릴리스 자산은 덮어쓰지 않았습니다.
- 공개된 다섯 자산을 다시 다운로드해 GitHub API의 크기·SHA-256 digest, 두 `.sha256` 파일과 `SHA256SUMS.txt`를 확인했습니다. ZIP의 다섯 payload 파일, MIT 라이선스와 manifest도 확인했으며 번들 카탈로그에는 공개 앱만 들어 있습니다.
- 공개 GitHub 설치 파일로 기존과 같은 AppId·경로에 설치했고 설치기 종료 코드는 0이었습니다. 실행 파일 ProductVersion과 Windows 설치 앱의 DisplayVersion이 모두 `0.3.1`이며, 설치된 payload 다섯 파일의 SHA-256이 공식 ZIP과 일치합니다.
- 설치 전 기존 설치 디렉터리와 개인 카탈로그·설정·설치 상태를 로컬에 백업했습니다. 설치 직후와 실제 GUI 실행 후 모두 원본 JSON 다섯 파일의 해시, Windows 자동 실행 등록의 존재 여부·전체 값, GitHub 자격 증명 blob의 지문이 동일했습니다. 자격 증명 원문은 출력하거나 파일로 저장하지 않았습니다.
- 실제 설치본을 기존 프로필로 `--background` 실행해 4초간 주 창이 없음을 확인했습니다. 같은 프로필로 다시 실행하면 기존 프로세스가 창을 만들며 설치 경로의 인스턴스는 하나였습니다. CLR·WebView 런타임 모듈은 0개였습니다.
- 설치된 GUI에서 런처 `0.3.1`, 기존 앱 설치 버전, 그리드, 비공개 개인 앱의 아이콘·커버, GitHub 연결과 1시간 확인 간격을 확인했습니다. 창의 닫기 버튼을 누르면 주 창이 창 목록에서 사라지고 설치된 프로세스는 계속 실행됐습니다. 다시 실행해 같은 창과 프로세스가 복귀하는 것까지 확인한 뒤 창을 닫아 트레이 상주 상태로 두었습니다.

공개 패키지 SHA-256:

| 자산 | SHA-256 |
| --- | --- |
| `moa-launcher-0.3.1-windows-x64.zip` | `578bdc1f64e1a092a2c427bfcf2b5036f5649a6700ef3da4da748d01b88ac130` |
| `MoaLauncher-0.3.1-Setup-x64.exe` | `7c963c076e788173888a4c2f556a61aaf1b119ed0b0ed247642aac803d76b7ea` |

0.3.1의 잠금 파일 SHA-256은 `7fffa8a15eb207b62710bf2d8db2ae1ebd3e4327d5e7a027e1f3664b3a4fa3b4`입니다. 위 감사에 사용한 0.3.0 잠금 파일과 비교해 루트 앱 버전만 변경했으며 외부 의존성 버전과 목록은 동일합니다. 로컬 백업·개인 데이터·인증 지문·검증 산출물은 무시되는 `artifacts/`에만 보관하며 Git에 포함하지 않습니다.
