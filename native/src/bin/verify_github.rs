use base64::{Engine, engine::general_purpose::STANDARD};
use moalauncher_lib::{
    artwork, github,
    models::AppDefinition,
    storage::{self, Paths},
};
use serde::Serialize;
use std::{fs, io::Write, path::PathBuf};
use uuid::Uuid;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    public_release_available: bool,
    public_icon_bytes: usize,
    public_cover_bytes: usize,
    private_app_configured: bool,
    private_release_available: bool,
    private_icon_bytes: usize,
    private_cover_bytes: usize,
}

#[tokio::main]
async fn main() {
    let report = run_smoke().await;
    let json = serde_json::to_vec_pretty(&report).unwrap_or_else(|_| b"{}".to_vec());
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from);
    if let Some(root) = repository_root {
        let artifact_dir = root.join("artifacts");
        if fs::create_dir_all(&artifact_dir).is_ok() {
            let _ = fs::write(artifact_dir.join("github-smoke.json"), &json);
        }
    }
    let _ = std::io::stdout().write_all(&json);
    let _ = std::io::stdout().write_all(b"\n");
}

async fn run_smoke() -> Report {
    let mut report = Report::default();
    let Ok(client) = github::client() else {
        return report;
    };
    let profile_paths = Paths::new(None);
    let isolated_root =
        std::env::temp_dir().join(format!("moalauncher-public-smoke-{}", Uuid::new_v4()));
    let public_paths = Paths::new(Some(isolated_root.clone()));

    // Public release and artwork calls use an isolated cache directory to leave profile data untouched.
    if let Ok(catalog) = storage::load_catalog(&profile_paths)
        && let Some(app) = catalog
            .apps
            .iter()
            .find(|app| app.repository.eq_ignore_ascii_case("hebory/moaview") && !app.is_private)
    {
        report.public_release_available = github::latest(&client, &public_paths, app).await.is_ok();
        report.public_icon_bytes =
            artwork_bytes(&client, &public_paths, app, app.icon_url.as_deref()).await;
        report.public_cover_bytes =
            artwork_bytes(&client, &public_paths, app, app.cover_url.as_deref()).await;
    }

    // Private app selection is data-driven. The stored credential is only read by the existing API modules.
    if let Ok(catalog) = storage::load_personal_catalog(&profile_paths)
        && let Some(app) = catalog.apps.iter().find(|app| app.is_private)
    {
        report.private_app_configured = true;
        report.private_release_available =
            github::latest(&client, &profile_paths, app).await.is_ok();
        report.private_icon_bytes =
            artwork_bytes(&client, &profile_paths, app, app.icon_url.as_deref()).await;
        report.private_cover_bytes =
            artwork_bytes(&client, &profile_paths, app, app.cover_url.as_deref()).await;
    }

    let _ = fs::remove_dir_all(isolated_root);
    report
}

async fn artwork_bytes(
    client: &reqwest::Client,
    paths: &Paths,
    app: &AppDefinition,
    url: Option<&str>,
) -> usize {
    let Some(url) = url else { return 0 };
    let Ok(Some(data_url)) = artwork::load(client, paths, app, url).await else {
        return 0;
    };
    let Some((_, encoded)) = data_url.split_once(',') else {
        return 0;
    };
    STANDARD.decode(encoded).map_or(0, |bytes| bytes.len())
}
