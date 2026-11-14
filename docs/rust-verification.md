# Rust 보안 및 연결 검증

검토일: 2026-11-15

## Cargo 의존성

공식 [RustSec `cargo-audit`](https://github.com/rustsec/rustsec/tree/main/cargo-audit) 0.23.0으로 `src-tauri/Cargo.lock`의 533개 의존성 항목을 검사했습니다. 감사한 lockfile의 SHA-256은 `67ac5726b4a50aac1b615f79d4840485f97212691dfdce51c35f6ebae3ae8d52`입니다. 기준 데이터는 [RustSec Advisory Database](https://github.com/RustSec/advisory-db) 커밋 `4ef2df8e806a65e2f1ecc14f56f51249062b23d6` (2026-11-13)입니다. 실행은 저장소 바깥에서 `--file`로 lockfile만 읽었고 앱의 Cargo 파일이나 lockfile을 변경하지 않았습니다. `--no-fetch --no-yanked`를 사용해 고정된 advisory 스냅샷을 대상으로 했으며 registry yanked 상태 확인은 건너뛰었습니다.

- 알려진 보안 취약점: **0건**
- 미유지보수 경고: **4건** — `proc-macro-error 1.0.4` (RUSTSEC-2024-0370), `zbus_names 4.3.5` (RUSTSEC-2026-0371), `zvariant 5.15.1` (RUSTSEC-2026-0370), `zvariant_derive 5.15.1` (RUSTSEC-2026-0372)
- soundness 경고: **1건** — `glib 0.18.5` (RUSTSEC-2024-0429). 이 경고는 `glib::VariantStrIter`의 반복자 메서드에 대한 soundness 문제이며, advisory는 `0.20.0` 이상을 수정 버전으로 표시합니다.

`cargo tree --locked --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc -i <crate>`로 `glib`과 미유지보수 경고 4개를 각각 역방향 확인했습니다. 다섯 crate 모두 Windows 대상 그래프에는 없었습니다. `--target all` 그래프에서 `glib`과 `proc-macro-error`는 GTK/WebKit 의존성 아래에, `zbus_names`·`zvariant`·`zvariant_derive`는 `tauri-plugin-notification`의 zbus 그래프 아래에 나타났습니다. 따라서 이 advisory 경고들은 잠금 파일의 다른 플랫폼 의존성에 해당하며 Windows x64 런처 빌드에는 포함되지 않습니다.

감사는 모든 lockfile 항목을 대상으로 했고 crates.io yanked 상태를 건너뛰었으므로 yanked 검사는 이 결과에 포함되지 않습니다. 미유지보수 경고를 취약점 건수에 더하지 않았으며, 패키지를 자동 갱신하거나 advisory를 예외 처리하지 않았습니다.

## 프로필 연결 확인

별도 `verify_github` 도구로 기본 카탈로그 앱의 공개 GitHub 릴리스와 아이콘·커버, 개인 카탈로그 첫 비공개 항목의 릴리스와 아이콘·커버를 확인했습니다. 공개 및 비공개 요청은 모두 성공했고 각 이미지 응답 바이트 수가 0보다 컸습니다. 결과 파일 `artifacts/github-smoke.json`에는 성공 여부와 이미지 바이트 수만 기록하며 계정 토큰, 비공개 앱 ID와 저장소 주소는 기록하지 않습니다.

이 확인에서 프로필의 기존 파일 1,920개 해시가 실행 전후 동일했고 설치 상태 3건을 읽기만 했습니다. GitHub 자격 증명은 기존 Windows 자격 증명 저장소에서 읽었으며 저장·삭제하지 않았습니다. 공개 캐시는 격리된 임시 폴더를 사용하고 삭제했습니다. 이 프로필 무결성 및 연결 결과는 통합 작업에서 전달받은 실행 요약이며, 이 문서 작성자가 직접 실행을 반복한 결과는 아닙니다.

## 테스트 범위

작성 시 전달받은 백엔드 테스트 상태는 30개 통과, 1개 무시이며, 설치 통합 픽스처는 별도로 실행 중입니다. 즐겨찾기 null 처리 테스트 추가 후 최종 결과는 아직 확인하지 않았습니다. 프런트엔드 테스트 4개 통과와 `npm audit --omit=dev` 취약점 0건은 이전에 전달된 결과이며 이번 검토에서 다시 실행하지 않았습니다. 다른 에이전트의 진행 중 테스트 결과도 확인하지 않았습니다.
