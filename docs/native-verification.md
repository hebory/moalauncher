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
