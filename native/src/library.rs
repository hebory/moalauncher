use crate::engine::{AppView, has_update};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Favorites,
    Installed,
    Updates,
}

pub fn visible_apps<'a>(
    apps: &'a [AppView],
    filter: Filter,
    query: &str,
    favorites: &[String],
    notified: &[String],
) -> Vec<&'a AppView> {
    let query = query.trim().to_lowercase();
    let mut visible: Vec<_> = apps
        .iter()
        .filter(|app| {
            (filter != Filter::Favorites || favorites.contains(&app.definition.id))
                && (filter != Filter::Installed || app.installed.is_some())
                && (filter != Filter::Updates || has_update(app))
                && format!(
                    "{} {} {}",
                    app.definition.name, app.definition.description, app.definition.repository
                )
                .to_lowercase()
                .contains(&query)
        })
        .collect();
    if filter == Filter::Updates {
        visible.sort_by_key(|app| !notified.contains(&app.definition.id));
    }
    visible
}

pub fn update_count(apps: &[AppView]) -> usize {
    apps.iter()
        .filter(|app| has_update(app) && app.error.is_none())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AppRelease, InstalledApp};
    fn app(id: &str, name: &str, version: Option<&str>, latest: Option<&str>) -> AppView {
        AppView {
            definition: serde_json::from_value(
                serde_json::json!({"id":id,"name":name,"repository":"owner/tool"}),
            )
            .unwrap(),
            installed: version.map(|version| InstalledApp {
                app_id: id.into(),
                version: version.into(),
                executable_path: "C:/fixture/tool.exe".into(),
                install_directory: "C:/fixture".into(),
                package_kind: "zip".into(),
                installed_at: String::new(),
            }),
            release: latest.map(|tag| AppRelease {
                tag: tag.into(),
                name: String::new(),
                notes: String::new(),
                page_url: String::new(),
                published_at: String::new(),
                assets: vec![],
                is_private: false,
            }),
            is_cached: false,
            is_launcher: false,
            busy: false,
            can_cancel: false,
            error: None,
            progress: None,
        }
    }
    #[test]
    fn favorites_and_search_are_combined() {
        let apps = vec![
            app("alpha", "Alpha Viewer", None, None),
            app("beta", "Beta Editor", None, None),
        ];
        assert_eq!(
            visible_apps(&apps, Filter::Favorites, "VIEW", &["alpha".into()], &[]).len(),
            1
        );
        assert!(
            visible_apps(&apps, Filter::Favorites, "editor", &["alpha".into()], &[]).is_empty()
        );
    }
    #[test]
    fn update_filter_excludes_uninstalled_and_current_versions() {
        let apps = vec![
            app("new", "New", None, Some("2.0.0")),
            app("current", "Current", Some("2.0.0"), Some("2.0.0")),
            app("old", "Old", Some("1.0.0"), Some("2.0.0")),
        ];
        assert_eq!(
            visible_apps(&apps, Filter::Updates, "", &[], &[])
                .iter()
                .map(|app| app.definition.id.as_str())
                .collect::<Vec<_>>(),
            vec!["old"]
        );
        assert_eq!(
            visible_apps(&apps, Filter::Installed, "", &[], &[]).len(),
            2
        );
    }
    #[test]
    fn clicked_notification_is_prioritized_in_updates() {
        let apps = vec![
            app("first", "First", Some("1.0.0"), Some("2.0.0")),
            app("clicked", "Clicked", Some("1.0.0"), Some("2.0.0")),
        ];
        assert_eq!(
            visible_apps(&apps, Filter::Updates, "", &[], &["clicked".into()])[0]
                .definition
                .id,
            "clicked"
        );
    }
    #[test]
    fn batch_count_excludes_apps_with_verification_errors() {
        let mut broken = app("broken", "Broken", Some("1.0.0"), Some("2.0.0"));
        broken.error = Some("release unavailable".into());
        assert_eq!(
            update_count(&[broken, app("ready", "Ready", Some("1.0.0"), Some("2.0.0"))]),
            1
        );
    }
}
