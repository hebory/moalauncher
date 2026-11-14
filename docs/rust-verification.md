# Rust 보안·연결 검증

검토일: 2026-11-15

## 의존성 감사

`src-tauri/Cargo.lock`의 533개 의존성 항목을 공식 [RustSec cargo-audit](https://github.com/rustsec/rustsec/tree/main/cargo-audit) 0.23.0으로 확인했습니다. 감사 lockfile SHA-256은 `22f98a68f508aae66a00f7f15c4021e51f33b9093bb0de5771ac3038704e9f6a`입니다. 사용한 [RustSec Advisory Database](https://github.com/RustSec/advisory-db)는 커밋 `4ef2df8e806a65e2f1ecc14f56f51249062b23d6` (2026-11-13)입니다.

- 알려진 취약점: **0건**
- 미유지보수: 4건 — `proc-macro-error 1.0.4` (RUSTSEC-2024-0370), `zbus_names 4.3.5` (RUSTSEC-2026-0371), `zvariant 5.15.1` (RUSTSEC-2026-0370), `zvariant_derive 5.15.1` (RUSTSEC-2026-0372)
- soundness advisory: `glib 0.18.5` (RUSTSEC-2024-0429). 영향 함수는 `glib::VariantStrIter`의 반복자 메서드이며 advisory의 수정 버전은 `0.20.0` 이상입니다.

`cargo tree --locked --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc -i <crate>` 조회에서 `glib`과 위 미유지보수 경고 패키지 4개는 모두 Windows x64 의존성 그래프에 없었습니다. 전체 대상 그래프에서는 `glib`과 `proc-macro-error`가 GTK/WebKit 계열 아래에, `zbus_names`·`zvariant`·`zvariant_derive`가 Linux 알림용 zbus 계열 아래에 있습니다. 따라서 위 5개 advisory 대상은 Windows x64 런처 빌드에 포함되지 않습니다.

재현 명령:

```powershell
cargo audit --file src-tauri/Cargo.lock --db <RustSec advisory-db 경로> --no-fetch --no-yanked
cargo tree --locked --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc -i glib
cargo tree --locked --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc -i proc-macro-error
cargo tree --locked --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc -i zbus_names
cargo tree --locked --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc -i zvariant
cargo tree --locked --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc -i zvariant_derive
```

감사는 전체 lockfile을 대상으로 했습니다. `--no-yanked` 옵션으로 crates.io yanked 상태는 검사하지 않았습니다. 패키지 자동 갱신이나 advisory 예외 처리는 하지 않았습니다.

## GitHub 프로필 smoke 확인

기본 카탈로그의 공개 앱과 개인 카탈로그의 첫 비공개 앱에서 GitHub 릴리스와 아이콘·커버를 확인했습니다. 공개·비공개 릴리스 응답과 두 이미지가 모두 성공했고 SHA-256 검증도 통과했습니다.

| 범위 | 릴리스 파일 바이트 | 아이콘 바이트 | 커버 바이트 |
|---|---:|---:|---:|
| 공개 | 69,316,124 | 48,548 | 2,114,994 |
| 비공개 | 3,258,546 | 1,443,045 | 1,479,592 |

프로필 원본 파일 1,920개의 해시는 실행 전후 같았고 설치 상태 3건은 읽기만 했습니다. GitHub 자격 증명은 Windows 자격 증명 저장소에서 읽었고 변경하지 않았습니다. 테스트는 토큰이나 비공개 앱 식별자를 기록하지 않으며 공개 이미지 캐시는 격리된 임시 폴더에 저장 후 삭제합니다. 요약 결과는 `artifacts/github-smoke.json`에 성공 여부와 이미지 바이트 수만 기록합니다.

프로필 확인 명령:

```powershell
cargo run --locked --manifest-path src-tauri/Cargo.toml --bin verify_github
```

## 테스트 상태

- 백엔드 단위 테스트: 마지막 집계에서 **32 통과, 2 무시**. 설치 상태 PascalCase 호환 테스트가 추가되어 최종 재실행에서 개수가 늘어날 수 있습니다.
- 프런트엔드 테스트: **4 통과**
- `npm audit --omit=dev`: **취약점 0건**
- 무시된 실제 설치·다운로드 통합 사례는 별도 실행해 통과했습니다.
- 자체 업데이트 helper 실제 검증 3개 사례는 모두 통과했습니다. 정상 업데이트는 `0.3.0`에서 `0.3.1`로 진행했고, 부분 설치 실패와 설치 후 버전 불일치는 각각 기존 `0.3.0` 실행 파일 및 전체 대상 파일 해시를 복구했습니다. 세 사례 모두 사용자 상태를 유지했고 재시작 표식을 확인했습니다.
- 백엔드 테스트의 최종 통과·무시 수는 전체 재실행 뒤 확정해야 합니다.

재현 명령:

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm test -- --passWithNoTests
npm audit --omit=dev
```
