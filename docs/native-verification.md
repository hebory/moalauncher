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

## 0.3.2 화면 재설계와 로컬 PC 적용

2026-11-15에 UI 개선 요청에 따라 0.3.2를 로컬 빌드해 PC 설치본에 적용했습니다. 이 단계는 UI 개선과 로컬 적용이며 GitHub Release는 새로 게시하지 않았습니다. 소스는 `codex/rust-native-rewrite` 브랜치에서 관리합니다.

- 왼쪽 탐색, 고정 검색 도구 모음, 업데이트 안내와 모두 업데이트, 그리드·리스트, 설정 및 상세 모달을 같은 시각 체계로 구성했습니다. [Fluent 2 Layout](https://fluent2.microsoft.design/layout), [Typography](https://fluent2.microsoft.design/typography), [Accessibility](https://fluent2.microsoft.design/accessibility)를 참고해 본문 14px·보조 12px, 좌측 정렬, 명확한 제목 굵기와 키보드 포커스를 적용했습니다. 기본 보조 글자/캔버스 대비는 4.58:1, 흰 글자/주요 버튼 대비는 5.32:1입니다.
- 기본 창의 세 열과 약 832px 너비 창의 두 열 그리드를 실제 UI로 확인했습니다. 카드의 설명 길이가 달라도 버튼과 하단 높이가 정렬되며, 좁은 리스트에서도 상태·오류 표시를 유지합니다. 좁은 설정에서도 오른쪽 스위치와 확인 간격이 잘리지 않도록 가용 폭을 제한했습니다.
- 별도 프로필에서 즐겨찾기 필터, 그리드·리스트 선택 복원, 설정 토글, 24시간 선택·저장, 상세 모달 표시와 Escape 닫기를 확인했습니다. 실제 PC의 자동 실행이나 인증을 시험용 프로필로 바꾸지 않았습니다.
- 초기 창 표시 뒤 추가 프레임을 갱신해 첫 표시를 보완했습니다. 최적화 빌드의 최초 캡처에서도 입력 전에 화면이 표시됐습니다.
- Rust 테스트 40개, 형식 검사와 경고 없는 Clippy가 통과했습니다. 새 외부 의존성은 추가하지 않았으며, 실제 네트워크·설치기 검사는 기존 두 검사의 기본 무시 설정을 유지했습니다.
- 화면 재설계 당시 로컬 설치 패키지는 UI 커밋 `1d16e4e`를 포함합니다. ZIP·설치 파일의 SHA-256과 sidecar·manifest를 대조했고, 설치된 다섯 payload 파일이 로컬 ZIP과 일치했습니다. 설치기 종료 코드는 0, 실행 파일과 Windows 설치 등록 버전은 모두 `0.3.2`입니다.
- 기존 설치 디렉터리와 개인 데이터 파일을 백업했습니다. 설치 전후 및 GUI 실행 후 원본 개인 카탈로그·설정·설치 상태 JSON 다섯 파일의 해시, GitHub 자격 증명 지문, 자동 실행 등록의 전체 값이 모두 동일했습니다. 실제 개인 프로필에서 비공개 앱 아이콘·커버, GitHub 연결, 기존 설치 버전과 1시간 확인 간격을 확인했습니다.
- 최종 설치본의 창 없는 시작, 단일 인스턴스 활성화, 닫은 뒤 프로세스 유지 및 다시 열기를 확인했습니다. CLR·WebView 런타임 모듈은 0개였습니다. 검증 뒤 새 디자인의 프로그램 화면을 열어두었습니다.

최종 로컬 패키지·설치 로그·백업·개인 상태 비교 자료는 무시되는 `artifacts/`에만 보관합니다. 공개 카탈로그에 개인 앱 정보를 추가하지 않았습니다.

### 설치 상태 배지 후속 조정

- 커밋 `5b03a87`에서 목록의 ‘설치됨’을 높이 20px의 작은 배지로 줄이고, 그리드에서는 커버의 우측 상단 안쪽 10px 위치로 옮겼습니다. 카드 본문의 중복 표시는 제거하고 상태 행의 높이를 유지해 버튼 정렬을 보존했습니다. 업데이트·오류·작업 중 상태가 있을 때는 기존 상태를 우선 표시합니다.
- 형식 검사와 경고 없는 Clippy를 통과했으며 별도 프로필에서 그리드·목록을 실제 화면으로 확인했습니다. 로컬 0.3.2 설치본에도 적용해 공개·비공개 앱 커버의 배지와 GitHub 연결을 확인했습니다.
- 적용 직전 최신 PC 설치 상태를 새로 백업했습니다. 설치 후 payload 다섯 파일이 검증한 로컬 ZIP과 일치하며 원본 개인 카탈로그·설정·설치 상태 JSON 다섯 파일, GitHub 인증 지문, 자동 실행 등록이 동일했습니다. 창 없는 시작과 단일 인스턴스 활성화도 확인했습니다. 이 후속 조정에서도 GitHub Release는 새로 게시하지 않았습니다.

### 설치됨·업데이트 배지 규칙 통일

- 커밋 `8af1c10`에서 두 상태의 라벨·색상을 하나의 규칙으로 결정하도록 통합했습니다. ‘업데이트 가능’은 ‘업데이트’로 줄였고 목록은 동일한 20px 배지, 그리드는 동일한 커버 우측 상단 배지로 표시합니다. 오류·작업 중 상태의 우선순위와 카드 정렬은 유지합니다.
- 실제 프로그램을 변경하지 않는 별도 ZIP 설치 표식과 공개 릴리스 조회로 업데이트 상태를 재현해 목록·그리드를 확인했습니다. 시험용 설치 파일 패턴은 실제 배포 자산과 일치하지 않게 지정해 설치 실행을 막았습니다. 형식 검사와 경고 없는 Clippy를 통과했습니다.
- 로컬 0.3.2 PC 설치본에도 반영해 공개·비공개 업데이트 대상의 커버 배지와 설치됨 배지가 같은 위치에 있는 것을 확인했습니다. 적용 직전 백업과 비교해 개인 데이터 JSON 다섯 파일·인증 지문·자동 실행 값이 동일하며 설치 payload도 검증한 ZIP과 일치했습니다. 창 없는 시작과 단일 인스턴스 활성화를 확인하고 프로그램 화면을 열어두었습니다. GitHub Release는 새로 게시하지 않았습니다.

### 실행 중인 앱 종료 후 업데이트

2026-11-21, 커밋 `de4e573`의 변경을 로컬 0.3.2 PC 설치본에 적용했습니다. GitHub Release는 새로 게시하지 않았습니다.

- 다운로드와 SHA-256 검증을 마친 뒤 대상 앱에 일반 종료를 요청합니다. ZIP은 압축 해제와 실행 파일 확인도 먼저 마칩니다. 5초 뒤에도 실행 중이면 미저장 작업 손실 안내와 함께 ‘강제 종료 후 업데이트’ 또는 ‘취소’를 표시합니다. 확인 대기 중 앱을 직접 종료하면 업데이트를 이어갑니다.
- 실행 파일 전체 경로를 대조하고 프로세스 핸들을 유지합니다. 같은 이름의 다른 경로 프로그램과 현재 런처는 종료 대상에서 제외합니다. 종료를 확인하지 못하거나 설치 직전에 다시 실행되면 설치를 중단합니다. Inno의 `/NOCLOSEAPPLICATIONS` 옵션을 유지합니다.
- 개별·모두 업데이트는 같은 설치 경로를 사용하며 런처는 계속 마지막에 처리합니다. 확인 거절·개별 취소·일괄 취소·런처 창 닫기를 처리하고, 종료 대기 작업이 사라지면 해당 확인창만 정리합니다. 오래된 요청의 응답이 새 요청에 적용되지 않도록 요청 ID를 확인합니다.
- 일반 Rust 검증 46개, 형식 검사와 경고 없는 Clippy를 통과했습니다. 별도 실제 프로세스 검증 6개, ZIP·NSIS·Inno의 실행 중 업데이트 검증 7개, 기존 설치·업데이트·실패 보존 검증 1개도 통과했습니다. 공개·비공개 자산 다운로드의 기존 실프로필 네트워크 검증 1개는 이번에 다시 실행하지 않았습니다.
- 실제 검증은 복사한 전용 fixture 프로세스와 격리 설치 폴더를 사용했습니다. 정상 종료, 숨긴 창·창 없는 프로세스, 여러 프로세스, 동일 파일명/다른 경로, 강제 종료 승인·거절, 대기 중 취소·수동 종료, 파일·설치 상태 보존을 확인했습니다. 사용자 MoaVCS 프로세스를 강제 종료하거나 업데이트하지 않았습니다.
- 기존 설치와 최신 개인 데이터를 새로 백업한 뒤 검증한 로컬 설치 파일을 적용했습니다. 설치기 종료 코드 0, 설치 payload 다섯 파일이 ZIP과 일치했습니다. 개인 카탈로그·설정·설치 상태 JSON 여섯 파일의 해시, GitHub 인증 지문, 자동 실행 등록 값이 설치 전후 및 실행 후 동일했습니다.
- 설치본의 창 없는 시작, 단일 인스턴스 활성화, CLR·WebView 모듈 0개를 확인했습니다. 실제 화면의 기존 앱 여섯 개, GitHub 연결, 업데이트 상태를 확인하고 창을 열어두었습니다. 백업·패키지·로그와 개인 비교 자료는 무시되는 `artifacts/running-update-verification/`에만 보관합니다.

재현할 때는 Windows에서 `scripts/Test-RustInstall.ps1 -TestRunningApps -MakensisPath <makensis.exe> -IsccPath <ISCC.exe>`를 실행합니다. 스크립트가 전용 프로세스 fixture와 실제 설치기를 빌드하고 격리 검증을 실행하며, fixture는 배포 ZIP·설치 프로그램에 포함하지 않습니다.
