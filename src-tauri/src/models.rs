use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    #[serde(default = "schema_version", alias = "SchemaVersion")]
    pub schema_version: u32,
    #[serde(default, alias = "Apps")]
    pub apps: Vec<AppDefinition>,
}

fn schema_version() -> u32 { 1 }
impl Default for Catalog {
    fn default() -> Self { Self { schema_version: 1, apps: Vec::new() } }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDefinition {
    #[serde(default, alias = "Id")] pub id: String,
    #[serde(default, alias = "Name")] pub name: String,
    #[serde(default, alias = "Description")] pub description: String,
    #[serde(default = "default_category", alias = "Category")] pub category: String,
    #[serde(default = "default_accent", alias = "AccentColor")] pub accent_color: String,
    #[serde(default = "default_monogram", alias = "Monogram")] pub monogram: String,
    #[serde(default, alias = "IconUrl")] pub icon_url: Option<String>,
    #[serde(default, alias = "CoverUrl")] pub cover_url: Option<String>,
    #[serde(default, alias = "Repository")] pub repository: String,
    #[serde(default, alias = "IsPrivate")] pub is_private: bool,
    #[serde(default, alias = "Packages")] pub packages: Vec<PackageDefinition>,
}
fn default_category() -> String { "유틸리티".into() }
fn default_accent() -> String { "#7C9CFF".into() }
fn default_monogram() -> String { "M".into() }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageDefinition {
    #[serde(default = "default_architecture", alias = "Architecture")] pub architecture: String,
    #[serde(default = "default_kind", alias = "Kind")] pub kind: String,
    #[serde(default, alias = "AssetPattern")] pub asset_pattern: String,
    #[serde(default, alias = "ExecutablePath")] pub executable_path: String,
    #[serde(default, alias = "InstallDirectory")] pub install_directory: Option<String>,
}
impl Default for PackageDefinition {
    fn default() -> Self { Self { architecture: default_architecture(), kind: default_kind(), asset_pattern: String::new(), executable_path: String::new(), install_directory: None } }
}
fn default_architecture() -> String { "x64".into() }
fn default_kind() -> String { "zip".into() }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "yes", alias = "CheckOnStartup")] pub check_on_startup: bool,
    #[serde(default = "yes", alias = "PeriodicChecks")] pub periodic_checks: bool,
    #[serde(default = "yes", alias = "UpdateNotifications")] pub update_notifications: bool,
    #[serde(default = "yes", alias = "UseGridView")] pub use_grid_view: bool,
    #[serde(default, alias = "FavoriteAppIds")] pub favorite_app_ids: Vec<String>,
    #[serde(default = "default_interval", alias = "CheckIntervalMinutes")] pub check_interval_minutes: u32,
    #[serde(flatten)] pub extra: Map<String, Value>,
}
fn yes() -> bool { true }
fn default_interval() -> u32 { 60 }
impl Default for Settings {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            periodic_checks: true,
            update_notifications: true,
            use_grid_view: true,
            favorite_app_ids: Vec::new(),
            check_interval_minutes: 60,
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    #[serde(alias = "AppId")] pub app_id: String,
    #[serde(alias = "Version")] pub version: String,
    #[serde(alias = "ExecutablePath")] pub executable_path: String,
    #[serde(alias = "InstallDirectory")] pub install_directory: String,
    #[serde(alias = "PackageKind")] pub package_kind: String,
    #[serde(alias = "InstalledAt")] pub installed_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    pub name: String, pub download_url: String, pub size: u64,
    #[serde(default)] pub sha256: Option<String>,
    #[serde(default)] pub api_url: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRelease {
    pub tag: String, pub name: String, pub notes: String, pub page_url: String,
    pub published_at: String, pub assets: Vec<ReleaseAsset>,
    #[serde(default)] pub is_private: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseResult { pub release: AppRelease, pub is_cached: bool }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub app_id: String, pub stage: String, pub bytes_received: u64,
    pub total_bytes: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_defaults_and_unknown_fields_round_trip() {
        let s: Settings = serde_json::from_str(r#"{"futureFlag":true,"favoriteAppIds":["moaview"]}"#).unwrap();
        assert!(s.check_on_startup && s.periodic_checks && s.update_notifications && s.use_grid_view);
        assert_eq!(s.check_interval_minutes, 60);
        assert_eq!(s.extra["futureFlag"], true);
        let v = serde_json::to_value(s).unwrap();
        assert_eq!(v["futureFlag"], true);
    }
    #[test]
    fn installed_pascal_case_is_read() {
        let app: InstalledApp = serde_json::from_str(r#"{"AppId":"moaview","Version":"1","ExecutablePath":"MoaView.exe","InstallDirectory":"Programs/MoaView","PackageKind":"inno","InstalledAt":"2025-01-01T00:00:00Z"}"#).unwrap();
        assert_eq!(app.app_id, "moaview");
    }
}
