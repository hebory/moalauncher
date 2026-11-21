use crate::{
    engine::{AppView, EngineEvent, SharedEngine, Snapshot, has_update},
    gui_theme::{self as theme, ACCENT as PURPLE, BACKGROUND, BORDER, Icon, MUTED},
    library::{self, Filter},
    models::{AppDefinition, Catalog, PackageDefinition},
    platform::InstanceGuard,
    runtime::Session,
    windows::{NativeEvent, Tray},
};
use base64::Engine;
use eframe::egui::{self, Align, Color32, FontId, RichText, TextureHandle, Vec2};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc::Receiver},
};

#[derive(Default)]
struct Artwork {
    icon: Option<TextureHandle>,
    cover: Option<TextureHandle>,
}
#[derive(PartialEq)]
enum Page {
    Library,
    Settings,
}
struct Editor {
    definition: AppDefinition,
    existing: bool,
}

pub struct NativeApp {
    tray: Tray,
    _guard: InstanceGuard,
    engine: SharedEngine,
    _runtime: Arc<tokio::runtime::Runtime>,
    wake_context: Arc<Mutex<Option<egui::Context>>>,
    native_events: Receiver<NativeEvent>,
    snapshot: Snapshot,
    artwork: HashMap<String, Artwork>,
    generation: u64,
    launcher_icon: TextureHandle,
    launcher_cover: TextureHandle,
    page: Page,
    filter: Filter,
    query: String,
    selected: Option<String>,
    editor: Option<Editor>,
    imported: Option<Catalog>,
    notified: Vec<String>,
    password: String,
    error: String,
    visible: bool,
    quitting: bool,
    catalog_pending: bool,
    initial_repaints: u8,
}

impl NativeApp {
    pub fn new(cc: &eframe::CreationContext<'_>, session: Session) -> Result<Self, String> {
        configure(&cc.egui_ctx);
        let Session {
            tray,
            guard,
            engine,
            runtime,
            native_events,
            wake_context,
            notified,
            error,
            ..
        } = session;
        *wake_context.lock().unwrap() = Some(cc.egui_ctx.clone());
        let snapshot = engine.snapshot();
        let launcher_icon = texture(
            &cc.egui_ctx,
            "launcher-icon",
            include_bytes!("../icons/icon.png"),
        )?;
        let launcher_cover = texture(
            &cc.egui_ctx,
            "launcher-cover",
            include_bytes!("../../cover.png"),
        )?;
        let filter = if notified.is_empty() {
            Filter::All
        } else {
            Filter::Updates
        };
        Ok(Self {
            tray,
            _guard: guard,
            engine,
            _runtime: runtime,
            wake_context,
            native_events,
            snapshot,
            artwork: HashMap::new(),
            generation: u64::MAX,
            launcher_icon,
            launcher_cover,
            page: Page::Library,
            filter,
            query: String::new(),
            selected: None,
            editor: None,
            imported: None,
            notified,
            password: String::new(),
            error,
            visible: true,
            quitting: false,
            catalog_pending: false,
            initial_repaints: 3,
        })
    }

    fn report(&mut self, result: Result<(), String>) {
        if let Err(message) = result {
            self.error = message;
        }
    }
    fn busy(&self) -> bool {
        self.catalog_pending
            || self.snapshot.working
            || self.snapshot.checking
            || self.snapshot.batch_updating
            || self.snapshot.apps.iter().any(|app| app.busy)
    }
    fn save_patch(&mut self, patch: serde_json::Value) {
        match self.engine.save_settings(patch) {
            Ok(()) => self.snapshot = self.engine.snapshot(),
            Err(error) => self.error = error,
        }
    }
    fn show(&mut self, ctx: &egui::Context) {
        self.visible = true;
        self.engine.set_background(false);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    fn quit(&mut self, ctx: &egui::Context) {
        if let Some(prompt) = self.snapshot.force_close_prompt.as_ref() {
            self.engine.respond_force_close(&prompt.request_id, false);
            self.snapshot = self.engine.snapshot();
        }
        match self.engine.request_exit() {
            Ok(()) => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(error) => {
                self.error = error;
                self.show(ctx);
            }
        }
    }

    fn process(&mut self, ctx: &egui::Context) {
        let events = self.engine.drain_events();
        if events
            .iter()
            .any(|event| matches!(event, EngineEvent::Changed))
        {
            self.snapshot = self.engine.snapshot();
        }
        for event in events {
            match event {
                EngineEvent::Changed => {}
                EngineEvent::ForceCloseRequested => {
                    self.snapshot = self.engine.snapshot();
                    self.show(ctx);
                }
                EngineEvent::Notify { ids, body } => self.tray.notify(ids, &body),
                EngineEvent::Error(error) => {
                    self.error = error;
                    self.catalog_pending = false;
                }
                EngineEvent::ExitRequested => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                EngineEvent::ImportPreview(catalog) => {
                    self.imported = Some(catalog);
                    self.catalog_pending = false;
                }
                EngineEvent::CatalogSaved => {
                    self.editor = None;
                    self.imported = None;
                    self.catalog_pending = false;
                    self.page = Page::Library;
                    self.filter = Filter::All;
                    self.query.clear();
                }
                EngineEvent::Artwork {
                    id,
                    icon,
                    cover,
                    generation,
                } => {
                    if generation == self.snapshot.auth_generation {
                        let icon = icon
                            .as_deref()
                            .and_then(|value| data_texture(ctx, "program-icon", value));
                        let cover = cover
                            .as_deref()
                            .and_then(|value| data_texture(ctx, "program-cover", value));
                        self.artwork.insert(id, Artwork { icon, cover });
                    }
                }
            }
        }
        let native: Vec<_> = self.native_events.try_iter().collect();
        for event in native {
            match event {
                NativeEvent::Open => self.show(ctx),
                NativeEvent::ShowUpdates(ids) => {
                    self.notified = ids;
                    self.filter = Filter::Updates;
                    self.page = Page::Library;
                    self.selected = None;
                    self.editor = None;
                    self.imported = None;
                    self.query.clear();
                    self.show(ctx);
                }
                NativeEvent::UpdateAll => {
                    let result = self.engine.update_all();
                    self.report(result);
                }
                NativeEvent::Quit => self.quit(ctx),
            }
        }
        if self.generation != self.snapshot.auth_generation {
            self.generation = self.snapshot.auth_generation;
            self.artwork.clear();
            for app in &self.snapshot.apps {
                if !app.is_launcher {
                    let _ = self.engine.load_artwork(app.definition.id.clone());
                }
            }
        }
        self.tray
            .set_update_enabled(!self.busy() && library::update_count(&self.snapshot.apps) > 0);
        if ctx.input(|input| input.viewport().close_requested()) && !self.quitting {
            if let Some(prompt) = self.snapshot.force_close_prompt.as_ref() {
                self.engine.respond_force_close(&prompt.request_id, false);
                self.snapshot = self.engine.snapshot();
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.visible = false;
            self.engine.set_background(true);
        }
    }

    fn primary(&mut self, app: &AppView) {
        if app.definition.repository.is_empty() {
            self.edit(Some(&app.definition));
            return;
        }
        let result = if app.installed.is_some() && !has_update(app) && !app.is_launcher {
            self.engine.launch(&app.definition.id)
        } else if app.release.is_none() || (app.is_launcher && !has_update(app)) {
            self.engine.check_updates()
        } else {
            self.engine.install(app.definition.id.clone())
        };
        self.report(result);
    }
    fn favorite(&mut self, id: &str) {
        let mut favorites = self.snapshot.settings.favorite_app_ids.clone();
        if favorites.iter().any(|item| item == id) {
            favorites.retain(|item| item != id);
        } else {
            favorites.push(id.to_string());
        }
        self.save_patch(serde_json::json!({"favoriteAppIds":favorites}));
    }
    fn edit(&mut self, definition: Option<&AppDefinition>) {
        self.error.clear();
        self.selected = None;
        self.editor = Some(Editor {
            definition: definition.cloned().unwrap_or_else(empty_definition),
            existing: definition.is_some(),
        });
    }
    fn image<'a>(&'a self, app: &AppView, cover: bool) -> Option<&'a TextureHandle> {
        if app.is_launcher {
            Some(if cover {
                &self.launcher_cover
            } else {
                &self.launcher_icon
            })
        } else {
            self.artwork.get(&app.definition.id).and_then(|art| {
                if cover {
                    art.cover.as_ref()
                } else {
                    art.icon.as_ref()
                }
            })
        }
    }
    fn icon(&self, ui: &mut egui::Ui, app: &AppView, size: f32) {
        if let Some(texture) = self.image(app, false) {
            ui.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(Vec2::splat(size))
                    .corner_radius(8),
            );
        } else {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 8, accent(&app.definition.accent_color));
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &app.definition.monogram,
                FontId::proportional(size * 0.42),
                Color32::WHITE,
            );
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let updates = library::update_count(&self.snapshot.apps);
        let installed = self
            .snapshot
            .apps
            .iter()
            .filter(|app| app.installed.is_some())
            .count();
        let favorites = self
            .snapshot
            .apps
            .iter()
            .filter(|app| {
                self.snapshot
                    .settings
                    .favorite_app_ids
                    .contains(&app.definition.id)
            })
            .count();
        let response = egui::Panel::left("workspace-navigation")
            .exact_size(212.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(16, 24)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(&self.launcher_icon)
                            .fit_to_exact_size(Vec2::splat(38.0))
                            .corner_radius(10),
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(theme::strong("moa", 25.0));
                        ui.label(RichText::new("Launcher").size(12.0).color(MUTED));
                    });
                });
                ui.add_space(36.0);
                ui.label(RichText::new("라이브러리").size(12.0).color(MUTED));
                ui.add_space(8.0);
                for (filter, icon, title, count) in [
                    (
                        Filter::All,
                        Icon::Library,
                        "모든 프로그램",
                        self.snapshot.apps.len(),
                    ),
                    (Filter::Favorites, Icon::Star, "즐겨찾기", favorites),
                    (Filter::Installed, Icon::Installed, "설치됨", installed),
                    (Filter::Updates, Icon::Refresh, "업데이트", updates),
                ] {
                    if theme::nav_item(
                        ui,
                        icon,
                        title,
                        (count > 0).then_some(count),
                        self.page == Page::Library && self.filter == filter,
                    )
                    .clicked()
                    {
                        self.page = Page::Library;
                        self.filter = filter;
                    }
                    ui.add_space(2.0);
                }
                ui.with_layout(egui::Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(format!("Moa Launcher  {}", self.snapshot.version))
                            .size(12.0)
                            .color(MUTED),
                    );
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        theme::icon(ui, Icon::Github, 16.0, MUTED);
                        ui.label(
                            RichText::new(if self.snapshot.github_connected {
                                "GitHub 연결됨"
                            } else {
                                "개인 워크스페이스"
                            })
                            .size(12.0)
                            .color(MUTED),
                        );
                        if self.snapshot.github_connected {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(6.0), egui::Sense::hover());
                            ui.painter()
                                .circle_filled(rect.center(), 3.0, theme::SUCCESS);
                        }
                    });
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(12.0);
                    if theme::nav_item(
                        ui,
                        Icon::Settings,
                        "설정",
                        None,
                        self.page == Page::Settings,
                    )
                    .clicked()
                    {
                        self.page = Page::Settings;
                    }
                });
            });
        let rect = response.response.rect;
        ui.painter().line_segment(
            [rect.right_top(), rect.right_bottom()],
            egui::Stroke::new(1.0, BORDER),
        );
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("workspace-toolbar")
            .exact_size(72.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(28, 16)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.page == Page::Library {
                        let width = (ui.available_width() - 240.0).clamp(160.0, 360.0);
                        egui::Frame::new()
                            .fill(BACKGROUND)
                            .stroke(egui::Stroke::new(1.0, BORDER))
                            .corner_radius(9)
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(width);
                                ui.spacing_mut().interact_size.y = 20.0;
                                ui.horizontal(|ui| {
                                    theme::icon(ui, Icon::Search, 17.0, MUTED);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.query)
                                            .id_salt("library-search")
                                            .hint_text("프로그램 검색")
                                            .frame(egui::Frame::NONE)
                                            .desired_width(width - 28.0),
                                    );
                                });
                            });
                    } else {
                        ui.label(RichText::new("워크스페이스").size(13.0).color(MUTED));
                        theme::icon(ui, Icon::ChevronRight, 14.0, MUTED);
                        ui.label(theme::strong("설정", 13.0));
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if self.page == Page::Library
                            && theme::button(
                                ui,
                                "프로그램 추가",
                                Some(Icon::Plus),
                                false,
                                !self.busy(),
                            )
                            .clicked()
                        {
                            self.edit(None);
                        }
                        if self.snapshot.checking {
                            ui.spinner();
                        } else if ui
                            .add_enabled_ui(!self.busy(), |ui| {
                                theme::icon_button(ui, Icon::Refresh, false, "업데이트 확인")
                            })
                            .inner
                            .clicked()
                        {
                            let result = self.engine.check_updates();
                            self.report(result);
                        }
                    });
                });
            });
    }

    fn footer(&self, ui: &mut egui::Ui) {
        egui::Panel::bottom("workspace-status")
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(28, 10)),
            )
            .show(ui, |ui| {
                let available = ui.available_width();
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(Vec2::splat(6.0), egui::Sense::hover());
                    ui.painter().circle_filled(
                        dot.center(),
                        3.0,
                        if self.snapshot.checking {
                            PURPLE
                        } else {
                            theme::SUCCESS
                        },
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(&self.snapshot.status).size(12.0).color(MUTED),
                        )
                        .truncate()
                        .selectable(false),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if available > 540.0 {
                            ui.label(
                                RichText::new(
                                    self.snapshot
                                        .last_checked
                                        .as_ref()
                                        .map(|time| format!("마지막 확인 {time}"))
                                        .unwrap_or_else(|| "아직 확인하지 않음".into()),
                                )
                                .size(12.0)
                                .color(MUTED),
                            );
                        }
                    });
                });
            });
    }

    fn notices(&mut self, ui: &mut egui::Ui) {
        if let Some(result) = self.snapshot.update_result.clone() {
            egui::Frame::new()
                .fill(theme::ACCENT_SOFT)
                .inner_margin(16)
                .corner_radius(10)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        theme::icon(ui, Icon::Check, 18.0, PURPLE);
                        ui.label(&result.message);
                        if theme::icon_button(ui, Icon::Close, false, "결과 닫기").clicked() {
                            let result = self.engine.dismiss_update_result();
                            self.report(result);
                        }
                    });
                });
            ui.add_space(16.0);
        }
        if !self.error.is_empty() {
            egui::Frame::new()
                .fill(Color32::from_rgb(255, 239, 241))
                .inner_margin(16)
                .corner_radius(10)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(Color32::from_rgb(162, 47, 66), &self.error);
                        if theme::icon_button(ui, Icon::Close, false, "오류 닫기").clicked() {
                            self.error.clear();
                        }
                    });
                });
            ui.add_space(16.0);
        }
    }

    fn library(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let busy = self.busy();
        let updates = library::update_count(&self.snapshot.apps);
        let (title, subtitle) = match self.filter {
            Filter::Favorites => ("즐겨찾기", "자주 쓰는 도구를 한곳에 모아두세요."),
            Filter::Installed => ("설치된 프로그램", "바로 실행할 수 있는 나의 도구들입니다."),
            Filter::Updates => ("업데이트", "도구를 최신 상태로 유지하세요."),
            Filter::All => (
                "모든 프로그램",
                "필요한 도구를 찾고, 실행하고, 최신으로 유지하세요.",
            ),
        };
        ui.label(theme::strong(title, 27.0));
        ui.add_space(3.0);
        ui.label(RichText::new(subtitle).size(14.0).color(MUTED));
        ui.add_space(24.0);
        if updates > 0 || self.snapshot.batch_updating {
            egui::Frame::new()
                .fill(theme::ACCENT_SOFT)
                .corner_radius(12)
                .inner_margin(20)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        egui::Frame::new()
                            .fill(theme::SURFACE)
                            .corner_radius(10)
                            .inner_margin(10)
                            .show(ui, |ui| {
                                theme::icon(ui, Icon::Download, 22.0, PURPLE);
                            });
                        ui.add_space(4.0);
                        let reserved = if ui.available_width() > 430.0 {
                            164.0
                        } else {
                            142.0
                        };
                        ui.allocate_ui_with_layout(
                            Vec2::new((ui.available_width() - reserved).max(130.0), 44.0),
                            egui::Layout::top_down(Align::Min),
                            |ui| {
                                ui.label(theme::strong(
                                    if self.snapshot.batch_updating {
                                        "도구를 업데이트하고 있어요".into()
                                    } else {
                                        format!("새 버전이 준비된 도구 {updates}개")
                                    },
                                    15.0,
                                ));
                                ui.add_space(2.0);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(
                                            "런처 업데이트는 다른 도구가 끝난 뒤 진행합니다.",
                                        )
                                        .size(12.0)
                                        .color(MUTED),
                                    )
                                    .truncate(),
                                );
                            },
                        );
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            if self.snapshot.batch_updating {
                                if theme::button(
                                    ui,
                                    "업데이트 취소",
                                    Some(Icon::Close),
                                    false,
                                    true,
                                )
                                .clicked()
                                {
                                    self.engine.cancel(None);
                                }
                            } else if theme::button(
                                ui,
                                "모두 업데이트",
                                Some(Icon::Refresh),
                                true,
                                !busy,
                            )
                            .clicked()
                            {
                                let result = self.engine.update_all();
                                self.report(result);
                            }
                        });
                    });
                });
            ui.add_space(22.0);
        }
        let apps: Vec<_> = library::visible_apps(
            &self.snapshot.apps,
            self.filter,
            &self.query,
            &self.snapshot.settings.favorite_app_ids,
            &self.notified,
        )
        .into_iter()
        .cloned()
        .collect();
        ui.horizontal(|ui| {
            ui.label(theme::strong(format!("프로그램 {}", apps.len()), 14.0));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, BORDER))
                    .corner_radius(8)
                    .inner_margin(3)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if theme::icon_button(
                            ui,
                            Icon::Grid,
                            self.snapshot.settings.use_grid_view,
                            "그리드 보기",
                        )
                        .clicked()
                        {
                            self.save_patch(serde_json::json!({"useGridView":true}));
                        }
                        if theme::icon_button(
                            ui,
                            Icon::List,
                            !self.snapshot.settings.use_grid_view,
                            "리스트 보기",
                        )
                        .clicked()
                        {
                            self.save_patch(serde_json::json!({"useGridView":false}));
                        }
                    });
            });
        });
        ui.add_space(14.0);
        if apps.is_empty() {
            ui.add_space(52.0);
            ui.vertical_centered(|ui| {
                theme::icon(
                    ui,
                    if self.filter == Filter::Favorites {
                        Icon::Star
                    } else {
                        Icon::Search
                    },
                    40.0,
                    PURPLE,
                );
                ui.add_space(14.0);
                ui.label(theme::strong(
                    if self.filter == Filter::Favorites {
                        "자주 쓰는 도구를 모아보세요"
                    } else {
                        "표시할 프로그램이 없습니다"
                    },
                    19.0,
                ));
                ui.add_space(6.0);
                ui.label(
                    RichText::new(if self.filter == Filter::Favorites {
                        "프로그램의 별 버튼을 누르면 여기에 표시됩니다."
                    } else {
                        "검색어나 필터를 바꿔보세요."
                    })
                    .color(MUTED),
                );
            });
        } else if self.snapshot.settings.use_grid_view {
            let columns = ((ui.available_width() + 20.0) / 282.0)
                .floor()
                .clamp(1.0, 4.0) as usize;
            let width = (ui.available_width() - 20.0 * (columns - 1) as f32) / columns as f32;
            for row in apps.chunks(columns) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 20.0;
                    for app in row {
                        ui.allocate_ui_with_layout(
                            Vec2::new(width, 0.0),
                            egui::Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_width(width);
                                ui.push_id(&app.definition.id, |ui| self.card(ui, app, busy));
                            },
                        );
                    }
                });
                ui.add_space(20.0);
            }
        } else {
            for app in apps {
                ui.push_id(&app.definition.id, |ui| self.list_row(ui, &app, busy));
                ui.add_space(10.0);
            }
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, app: &AppView, busy: bool) {
        let width = ui.available_width();
        let badge = installation_badge(app);
        panel().inner_margin(0).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(width);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 128.0), egui::Sense::hover());
            if let Some(image) = self.image(app, true) {
                paint_cover(ui, image, rect);
            } else {
                ui.painter().rect_filled(
                    rect,
                    12,
                    accent(&app.definition.accent_color).gamma_multiply(0.18),
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &app.definition.monogram,
                    FontId::proportional(36.0),
                    accent(&app.definition.accent_color),
                );
            }
            if let Some((label, color, _)) = badge {
                let badge_rect = egui::Rect::from_min_max(
                    rect.left_top() + Vec2::new(10.0, 10.0),
                    rect.right_top() + Vec2::new(-10.0, 30.0),
                );
                let mut badge_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("cover-installation-badge")
                        .max_rect(badge_rect)
                        .layout(egui::Layout::right_to_left(Align::Min)),
                );
                theme::compact_pill(&mut badge_ui, label, color, Color32::from_white_alpha(248));
            }
            egui::Frame::new().inner_margin(18).show(ui, |ui| {
                ui.set_width(width - 36.0);
                ui.horizontal(|ui| {
                    self.icon(ui, app, 40.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        limited_label(
                            ui,
                            &app.definition.name,
                            width - 134.0,
                            1,
                            15.0,
                            theme::TEXT,
                            true,
                        );
                        version_badge(ui, app);
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        self.star(ui, app);
                    });
                });
                ui.add_space(12.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(width - 36.0, 40.0),
                    egui::Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_min_height(40.0);
                        limited_label(
                            ui,
                            &app.definition.description,
                            width - 36.0,
                            2,
                            13.0,
                            MUTED,
                            false,
                        );
                    },
                );
                ui.add_space(10.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(width - 36.0, 28.0),
                    egui::Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_height(28.0);
                        if badge.is_none() {
                            self.status(ui, app);
                        }
                        if app.definition.is_private {
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                theme::pill(ui, "개인", MUTED, BACKGROUND)
                                    .on_hover_text("비공개 · 개인 카탈로그");
                                theme::icon(ui, Icon::Lock, 12.0, MUTED);
                            });
                        }
                    },
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    self.action_button(ui, app, busy, width - 82.0);
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if theme::icon_button(ui, Icon::ChevronRight, false, "프로그램 상세 보기")
                            .clicked()
                        {
                            self.selected = Some(app.definition.id.clone());
                        }
                    });
                });
            });
        });
    }

    fn list_row(&mut self, ui: &mut egui::Ui, app: &AppView, busy: bool) {
        panel().inner_margin(18).show(ui, |ui| {
            let width = ui.available_width();
            ui.set_width(width);
            let narrow = width < 730.0;
            let trailing_width = if narrow { 204.0 } else { 302.0 };
            let height = if narrow { 72.0 } else { 60.0 };
            let (row, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(row.left() + 22.0, row.center().y),
                Vec2::splat(44.0),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(icon_rect), |ui| {
                self.icon(ui, app, 44.0);
            });
            let actions = egui::Rect::from_min_max(
                egui::pos2(row.right() - trailing_width, row.top()),
                row.right_bottom(),
            );
            let content = egui::Rect::from_min_max(
                egui::pos2(row.left() + 60.0, row.top()),
                egui::pos2(actions.left() - 16.0, row.bottom()),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(content)
                    .layout(egui::Layout::top_down(Align::Min)),
                |ui| {
                    ui.set_width(content.width());
                    ui.horizontal(|ui| {
                        limited_label(
                            ui,
                            &app.definition.name,
                            (content.width() - 78.0).max(70.0),
                            1,
                            15.0,
                            theme::TEXT,
                            true,
                        );
                        version_badge(ui, app);
                    });
                    limited_label(
                        ui,
                        &app.definition.description,
                        content.width(),
                        1,
                        13.0,
                        MUTED,
                        false,
                    );
                    if narrow {
                        ui.horizontal(|ui| {
                            self.status(ui, app);
                            if app.definition.is_private {
                                theme::icon(ui, Icon::Lock, 12.0, MUTED)
                                    .on_hover_text("비공개 · 개인 카탈로그");
                            }
                        });
                    } else if app.definition.is_private {
                        ui.label(
                            RichText::new("비공개 · 개인 카탈로그")
                                .size(12.0)
                                .color(MUTED),
                        );
                    }
                },
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(actions)
                    .layout(egui::Layout::right_to_left(Align::Center)),
                |ui| {
                    if theme::icon_button(ui, Icon::ChevronRight, false, "프로그램 상세 보기")
                        .clicked()
                    {
                        self.selected = Some(app.definition.id.clone());
                    }
                    self.star(ui, app);
                    self.action_button(ui, app, busy, if narrow { 100.0 } else { 118.0 });
                    if !narrow {
                        self.status(ui, app);
                    }
                },
            );
        });
    }

    fn star(&mut self, ui: &mut egui::Ui, app: &AppView) {
        let selected = self
            .snapshot
            .settings
            .favorite_app_ids
            .contains(&app.definition.id);
        if theme::icon_button(
            ui,
            Icon::Star,
            selected,
            if selected {
                "즐겨찾기 해제"
            } else {
                "즐겨찾기 추가"
            },
        )
        .clicked()
        {
            self.favorite(&app.definition.id);
        }
    }

    fn status(&self, ui: &mut egui::Ui, app: &AppView) {
        if let Some((label, color, background)) = installation_badge(app) {
            theme::compact_pill(ui, label, color, background);
            return;
        }
        let (message, color, bg) = if app.busy {
            (
                app.progress
                    .as_ref()
                    .map(|p| p.stage.as_str())
                    .unwrap_or("확인 중"),
                PURPLE,
                theme::ACCENT_SOFT,
            )
        } else if app.error.is_some() {
            (
                "확인 필요",
                Color32::from_rgb(166, 57, 71),
                Color32::from_rgb(255, 239, 241),
            )
        } else if app.definition.repository.is_empty() {
            ("저장소 연결 필요", MUTED, BACKGROUND)
        } else if app.release.is_some() {
            ("설치 가능", MUTED, BACKGROUND)
        } else {
            ("확인 전", MUTED, BACKGROUND)
        };
        theme::pill(ui, message, color, bg);
    }

    fn action_button(&mut self, ui: &mut egui::Ui, app: &AppView, busy: bool, width: f32) {
        ui.allocate_ui_with_layout(
            Vec2::new(width, 36.0),
            egui::Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(width);
                ui.set_min_width(width);
                if let Some(progress) = &app.progress
                    && let Some(total) = progress.total_bytes.filter(|total| *total > 0)
                {
                    ui.add(
                        egui::ProgressBar::new(
                            (progress.bytes_received as f64 / total as f64).clamp(0.0, 1.0) as f32,
                        )
                        .desired_width(width)
                        .desired_height(4.0),
                    );
                }
                let cancel = app.busy && app.can_cancel;
                let current = app.is_launcher && app.release.is_some() && !has_update(app);
                let primary = has_update(app) || app.installed.is_none();
                let full_label = if cancel {
                    "다운로드 취소"
                } else {
                    button_text(app)
                };
                let label = if width < 130.0 && app.is_launcher && !app.busy {
                    if has_update(app) {
                        "업데이트"
                    } else if app.release.is_none() {
                        "버전 확인"
                    } else {
                        "최신 버전"
                    }
                } else {
                    full_label
                };
                let button = egui::Button::new(RichText::new(label).size(13.0).color(if current {
                    MUTED
                } else if primary {
                    Color32::WHITE
                } else {
                    PURPLE
                }))
                .fill(if current {
                    BACKGROUND
                } else if primary {
                    PURPLE
                } else {
                    theme::ACCENT_SOFT
                })
                .stroke(egui::Stroke::NONE)
                .corner_radius(8)
                .min_size(Vec2::new(width, 36.0));
                if ui
                    .add_enabled(cancel || (!busy && !current), button)
                    .on_hover_text(full_label)
                    .clicked()
                {
                    if cancel {
                        self.engine.cancel(Some(app.definition.id.clone()));
                    } else {
                        self.primary(app);
                    }
                }
            },
        );
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.set_max_width(ui.available_width().min(800.0));
        ui.label(theme::strong("설정", 27.0));
        ui.add_space(3.0);
        ui.label(RichText::new("모아 런처를 나의 작업 방식에 맞춰보세요.").color(MUTED));
        ui.add_space(24.0);
        panel().show(ui, |ui| {
            setting_heading(ui, Icon::Power, "시작 및 종료");
            let mut startup = self.snapshot.startup_enabled;
            if setting_switch(
                ui,
                "Windows 시작 시 자동 실행",
                "로그인하면 창 없이 트레이에서 시작합니다.",
                &mut startup,
            ) {
                let result = self.engine.set_startup(startup);
                self.report(result);
                self.snapshot = self.engine.snapshot();
            }
            setting_separator(ui);
            ui.label(
                RichText::new(
                    "창을 닫아도 트레이에서 실행됩니다. 완전히 끝내려면 ‘런처 종료’를 선택하세요.",
                )
                .size(12.0)
                .color(MUTED),
            );
        });
        ui.add_space(18.0);
        panel().show(ui, |ui| {
            setting_heading(ui, Icon::Refresh, "업데이트");
            let mut startup = self.snapshot.settings.check_on_startup;
            if setting_switch(
                ui,
                "시작할 때 업데이트 확인",
                "런처를 열면 프로그램의 새 버전을 확인합니다.",
                &mut startup,
            ) {
                self.save_patch(serde_json::json!({"checkOnStartup":startup}));
            }
            setting_separator(ui);
            let mut periodic = self.snapshot.settings.periodic_checks;
            if setting_switch(
                ui,
                "주기적으로 업데이트 확인",
                "런처가 실행 중일 때 자동으로 확인합니다.",
                &mut periodic,
            ) {
                self.save_patch(serde_json::json!({"periodicChecks":periodic}));
            }
            let mut interval = self.snapshot.settings.check_interval_minutes;
            ui.add_space(10.0);
            ui.add_enabled_ui(periodic, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("확인 간격").size(13.0).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        egui::ComboBox::from_id_salt("check-interval")
                            .width(116.0)
                            .selected_text(interval_text(interval))
                            .show_ui(ui, |ui| {
                                for value in [30, 60, 180, 360, 720, 1440] {
                                    ui.selectable_value(&mut interval, value, interval_text(value));
                                }
                            });
                    });
                });
            });
            if interval != self.snapshot.settings.check_interval_minutes {
                self.save_patch(serde_json::json!({"checkIntervalMinutes":interval}));
            }
            setting_separator(ui);
            let mut notifications = self.snapshot.settings.update_notifications;
            if setting_switch(
                ui,
                "업데이트 알림",
                "새 버전이 나오면 트레이 알림으로 알려줍니다.",
                &mut notifications,
            ) {
                self.save_patch(serde_json::json!({"updateNotifications":notifications}));
            }
        });
        ui.add_space(18.0);
        panel().show(ui, |ui| {
            setting_heading(ui, Icon::Github, "GitHub 연결");
            ui.horizontal(|ui| {
                theme::pill(ui, if self.snapshot.github_connected {"연결됨"} else {"연결 안 됨"}, if self.snapshot.github_connected {theme::SUCCESS} else {MUTED}, if self.snapshot.github_connected {Color32::from_rgb(235,246,239)} else {BACKGROUND});
                ui.label(RichText::new("비공개 저장소의 도구도 사용할 수 있습니다.").size(13.0).color(MUTED));
            });
            ui.add_space(14.0);
            ui.add(egui::TextEdit::singleline(&mut self.password).password(true).hint_text("GitHub 개인 액세스 토큰").margin(egui::Margin::symmetric(12,10)).desired_width(f32::INFINITY));
            ui.add_space(4.0);
            ui.label(RichText::new("선택한 저장소의 Contents: Read-only 권한이 필요합니다. 인증 정보는 Windows 자격 증명 관리자에 안전하게 보관합니다.").size(12.0).color(MUTED));
            ui.add_space(10.0);
            let busy = self.busy();
            ui.horizontal_wrapped(|ui| {
                if theme::button(ui, if self.snapshot.github_connected {"연결 교체"} else {"GitHub 연결"}, Some(Icon::Github), true, !busy && !self.password.is_empty()).clicked() { let token=std::mem::take(&mut self.password); let result=self.engine.connect_github(token); self.report(result); }
                if self.snapshot.github_connected && theme::button(ui, "연결 해제", None, false, !busy).clicked() {let result=self.engine.disconnect_github();self.report(result);}
                if theme::button(ui, "토큰 발급 페이지", Some(Icon::External), false, true).clicked() {let result=self.engine.open_token_page();self.report(result);}
            });
        });
        ui.add_space(18.0);
        panel().show(ui, |ui| {
            setting_heading(ui, Icon::Folder, "개인 카탈로그와 데이터");
            ui.label(RichText::new("비공개 앱과 즐겨찾기는 이 PC에 저장됩니다. 내보내기에는 공개 여부를 확인한 앱만 포함합니다.").size(13.0).color(MUTED));
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if theme::button(ui,"데이터 폴더",Some(Icon::Folder),false,true).clicked() {let result=self.engine.open_location(None);self.report(result);}
                if theme::button(ui,"가져오기",Some(Icon::Download),false,!self.busy()).clicked() && let Some(path)=rfd::FileDialog::new().add_filter("앱 카탈로그", &["json"]).pick_file() {self.error.clear();self.catalog_pending=true;let result=self.engine.preview_import(path);self.report(result);}
                if theme::button(ui,"내보내기",Some(Icon::External),false,!self.busy()).clicked() && let Some(path)=rfd::FileDialog::new().add_filter("앱 카탈로그", &["json"]).set_file_name("moa-catalog.json").save_file() {let result=self.engine.export_catalog(path);self.report(result);}
            });
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new(&self.snapshot.data_directory).size(12.0).color(MUTED)).truncate()).on_hover_text(&self.snapshot.data_directory);
        });
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Moa Launcher {}", self.snapshot.version))
                    .size(12.0)
                    .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if theme::button(ui, "런처 종료", Some(Icon::Power), false, true).clicked() {
                    self.quit(ui.ctx());
                }
            });
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(app) = self
            .selected
            .as_ref()
            .and_then(|id| {
                self.snapshot
                    .apps
                    .iter()
                    .find(|app| &app.definition.id == id)
            })
            .cloned()
        {
            let mut close = false;
            let response = egui::Modal::new(egui::Id::unique("program-details")).frame(panel()).show(ctx, |ui| {
                ui.set_width(620.0_f32.min(ctx.content_rect().width() - 70.0));
                ui.horizontal(|ui| {
                    self.icon(ui, &app, 48.0);
                    ui.vertical(|ui| {
                        ui.label(theme::strong(&app.definition.name, 21.0));
                        ui.label(RichText::new(&app.definition.category).size(12.0).color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if theme::icon_button(ui, Icon::Close, false, "상세 닫기").clicked() { close = true; }
                    });
                });
                ui.add_space(16.0);
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 250.0).max(170.0)).show(ui, |ui| {
                    if let Some(image) = self.image(&app, true) {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 172.0), egui::Sense::hover());
                        paint_cover(ui, image, rect);
                        ui.add_space(16.0);
                    }
                    ui.label(RichText::new(&app.definition.description).size(14.0).color(MUTED));
                    ui.add_space(16.0);
                    egui::Frame::new().fill(BACKGROUND).corner_radius(10).inner_margin(16).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.columns(2, |columns| {
                            columns[0].label(RichText::new("설치 버전").size(12.0).color(MUTED));
                            columns[0].label(theme::strong(app.installed.as_ref().map(|installed| installed.version.as_str()).unwrap_or("미설치"),16.0));
                            columns[1].label(RichText::new("최신 버전").size(12.0).color(MUTED));
                            columns[1].label(theme::strong(app.release.as_ref().map(|release| release.tag.as_str()).unwrap_or("확인 전"),16.0));
                        });
                    });
                    ui.add_space(16.0);
                    if let Some(error) = &app.error { ui.colored_label(Color32::from_rgb(170,55,66),error);ui.add_space(10.0); }
                    ui.horizontal(|ui| {
                        ui.label(theme::strong("릴리스 노트",16.0));
                        if app.definition.is_private { theme::pill(ui,"비공개 · 개인 카탈로그",PURPLE,theme::ACCENT_SOFT); }
                    });
                    if app.is_cached { ui.label(RichText::new("오프라인 캐시 · 설치 전에 최신 배포 정보를 다시 확인합니다.").size(12.0).color(MUTED)); }
                    ui.add_space(8.0);
                    ui.label(app.release.as_ref().map(|release| release.notes.as_str()).filter(|text|!text.is_empty()).unwrap_or("업데이트 확인을 누르면 배포 정보를 가져옵니다."));
                    ui.add_space(18.0);
                    ui.horizontal_wrapped(|ui| {
                        if theme::button(ui,"GitHub 릴리스",Some(Icon::External),false,true).clicked() {let result=self.engine.open_github(&app.definition.id);self.report(result);}
                        if theme::button(ui,"설치 폴더",Some(Icon::Folder),false,app.installed.is_some()).clicked() {let result=self.engine.open_location(Some(app.definition.id.clone()));self.report(result);}
                        if !app.is_launcher && theme::button(ui,"프로그램 설정",Some(Icon::Settings),false,!self.busy()).clicked() {self.edit(Some(&app.definition));}
                    });
                });
                ui.add_space(18.0);
                ui.separator();
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if theme::button(ui,"닫기",None,false,true).clicked() {close=true;}
                    ui.with_layout(egui::Layout::right_to_left(Align::Center),|ui| {self.action_button(ui,&app,self.busy(),180.0);});
                });
            });
            if close || response.should_close() {
                self.selected = None;
            }
        }
        if let Some(mut editor) = self.editor.take() {
            let mut close = false;
            let mut save = false;
            let response = egui::Modal::new(egui::Id::unique("program-editor"))
                .frame(panel())
                .show(ctx, |ui| {
                    ui.set_width(560.0_f32.min(ctx.content_rect().width() - 70.0));
                    ui.heading(if editor.existing {
                        "프로그램 설정"
                    } else {
                        "프로그램 추가"
                    });
                    if !self.error.is_empty() {
                        ui.colored_label(Color32::from_rgb(170, 55, 66), &self.error);
                    }
                    egui::ScrollArea::vertical()
                        .max_height((ctx.content_rect().height() - 230.0).max(220.0))
                        .show(ui, |ui| {
                            form_field(ui, "이름", &mut editor.definition.name);
                            ui.add_enabled_ui(!editor.existing, |ui| {
                                form_field(ui, "앱 ID", &mut editor.definition.id);
                            });
                            form_field(ui, "설명", &mut editor.definition.description);
                            form_field(
                                ui,
                                "GitHub 저장소 (owner/repository 또는 저장소 URL)",
                                &mut editor.definition.repository,
                            );
                            ui.checkbox(
                                &mut editor.definition.is_private,
                                "비공개 앱 · 개인 카탈로그에만 저장",
                            );
                            ui.label(
                                RichText::new("저장할 때 실제 공개 여부와 접근 권한을 확인합니다.")
                                    .small()
                                    .color(MUTED),
                            );
                            for (index, package) in
                                editor.definition.packages.iter_mut().enumerate()
                            {
                                ui.push_id(index, |ui| {
                                    ui.add_space(10.0);
                                    ui.label(RichText::new("설치 패키지").strong());
                                    ui.horizontal(|ui| {
                                        egui::ComboBox::from_id_salt("architecture")
                                            .selected_text(&package.architecture)
                                            .show_ui(ui, |ui| {
                                                for arch in ["x64", "arm64", "x86"] {
                                                    ui.selectable_value(
                                                        &mut package.architecture,
                                                        arch.to_string(),
                                                        arch,
                                                    );
                                                }
                                            });
                                        egui::ComboBox::from_id_salt("package-kind")
                                            .selected_text(&package.kind)
                                            .show_ui(ui, |ui| {
                                                for kind in ["zip", "inno", "nsis"] {
                                                    ui.selectable_value(
                                                        &mut package.kind,
                                                        kind.to_string(),
                                                        kind,
                                                    );
                                                }
                                            });
                                    });
                                    form_field(ui, "릴리스 파일 패턴", &mut package.asset_pattern);
                                    form_field(
                                        ui,
                                        "패키지 안 실행 파일 경로",
                                        &mut package.executable_path,
                                    );
                                    if package.kind != "zip" {
                                        let directory = package
                                            .install_directory
                                            .get_or_insert_with(String::new);
                                        form_field(ui, "LocalAppData 기준 설치 폴더", directory);
                                    }
                                });
                            }
                            ui.collapsing("이미지와 표시 설정", |ui| {
                                optional_field(ui, "아이콘 URL", &mut editor.definition.icon_url);
                                optional_field(ui, "커버 URL", &mut editor.definition.cover_url);
                                form_field(ui, "분류", &mut editor.definition.category);
                                form_field(ui, "문자 아이콘", &mut editor.definition.monogram);
                                form_field(
                                    ui,
                                    "강조 색상 (#RRGGBB)",
                                    &mut editor.definition.accent_color,
                                );
                            });
                        });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.busy(), egui::Button::new("취소"))
                            .clicked()
                        {
                            close = true;
                        }
                        if ui
                            .add_enabled(
                                !self.busy(),
                                primary_button(if self.catalog_pending {
                                    "저장 중…"
                                } else {
                                    "개인 카탈로그에 저장"
                                }),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                    });
                });
            if save {
                editor.definition.repository = normalize_repository(&editor.definition.repository);
                self.error.clear();
                self.catalog_pending = true;
                let result = self.engine.save_program(editor.definition.clone());
                if result.is_err() {
                    self.catalog_pending = false;
                }
                self.report(result);
            }
            if !close && !(response.should_close() && !self.catalog_pending) {
                self.editor = Some(editor);
            }
        }
        if let Some(catalog) = self.imported.clone() {
            let mut close = false;
            let mut confirm = false;
            let response = egui::Modal::new(egui::Id::unique("catalog-preview"))
                .frame(panel())
                .show(ctx, |ui| {
                    ui.set_width(560.0_f32.min(ctx.content_rect().width() - 70.0));
                    ui.heading(format!("프로그램 {}개 가져오기", catalog.apps.len()));
                    ui.label("같은 ID의 도구는 프로그램 설정과 배포 출처가 변경됩니다.");
                    if !self.error.is_empty() {
                        ui.colored_label(Color32::from_rgb(170, 55, 66), &self.error);
                    }
                    egui::ScrollArea::vertical()
                        .max_height((ctx.content_rect().height() - 250.0).max(180.0))
                        .show(ui, |ui| {
                            for definition in &catalog.apps {
                                ui.separator();
                                ui.label(RichText::new(&definition.name).strong());
                                if let Some(old) = self
                                    .snapshot
                                    .apps
                                    .iter()
                                    .find(|app| app.definition.id == definition.id)
                                {
                                    if old.definition.repository != definition.repository {
                                        ui.label(format!(
                                            "{} → {}",
                                            old.definition.repository, definition.repository
                                        ));
                                    } else {
                                        ui.label(&definition.repository);
                                    }
                                } else {
                                    ui.label(&definition.repository);
                                }
                                if definition.is_private {
                                    ui.label(RichText::new("비공개 · 개인 카탈로그").color(PURPLE));
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.busy(), egui::Button::new("취소"))
                            .clicked()
                        {
                            close = true;
                        }
                        if ui
                            .add_enabled(!self.busy(), primary_button("확인 후 가져오기"))
                            .clicked()
                        {
                            confirm = true;
                        }
                    });
                });
            if confirm {
                self.error.clear();
                self.catalog_pending = true;
                let result = self.engine.confirm_import(catalog);
                if result.is_err() {
                    self.catalog_pending = false;
                }
                self.report(result);
            }
            if close || (response.should_close() && !self.catalog_pending) {
                self.imported = None;
            }
        }
        // Keep process termination confirmation above every other modal dialog.
        if let Some(prompt) = self.snapshot.force_close_prompt.clone() {
            let mut deny = false;
            let mut force_close = false;
            let response = egui::Modal::new(egui::Id::unique("force-close-before-update"))
                .frame(panel())
                .show(ctx, |ui| {
                    ui.set_width(480.0_f32.min(ctx.content_rect().width() - 70.0));
                    ui.heading("프로그램을 종료하고 업데이트할까요?");
                    ui.add_space(10.0);
                    ui.label(theme::strong(&prompt.app_name, 17.0));
                    ui.label(
                        RichText::new(format!("실행 중인 프로세스 {}개", prompt.process_count))
                            .size(13.0)
                            .color(MUTED),
                    );
                    ui.add_space(14.0);
                    ui.label("일반 종료 요청에 응답하지 않았습니다.");
                    ui.colored_label(
                        Color32::from_rgb(162, 95, 34),
                        "강제 종료하면 저장하지 않은 작업이 사라질 수 있습니다.",
                    );
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("취소")).clicked() {
                            deny = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(primary_button("강제 종료 후 업데이트")).clicked() {
                                force_close = true;
                            }
                        });
                    });
                });
            if force_close {
                self.engine.respond_force_close(&prompt.request_id, true);
                self.snapshot = self.engine.snapshot();
            } else if deny || response.should_close() {
                self.engine.respond_force_close(&prompt.request_id, false);
                self.snapshot = self.engine.snapshot();
            }
        }
    }
}

impl eframe::App for NativeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.initial_repaints > 0 {
            // Present again after the initially hidden native window becomes visible.
            self.initial_repaints -= 1;
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }
        self.sidebar(ui);
        self.header(ui);
        self.footer(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(28))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(if self.page == Page::Library {
                        "library-scroll"
                    } else {
                        "settings-scroll"
                    })
                    .show(ui, |ui| {
                        self.notices(ui);
                        if self.page == Page::Library {
                            self.library(ui);
                        } else {
                            self.settings(ui);
                        }
                    });
            });
        self.dialogs(ui.ctx());
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        *self.wake_context.lock().unwrap() = None;
        let _ = self.engine.request_exit();
    }
}

fn installation_badge(app: &AppView) -> Option<(&'static str, Color32, Color32)> {
    if app.busy || app.error.is_some() {
        None
    } else if has_update(app) {
        Some(("업데이트", PURPLE, theme::ACCENT_SOFT))
    } else if app.installed.is_some() {
        Some(("설치됨", theme::SUCCESS, Color32::from_rgb(235, 246, 239)))
    } else {
        None
    }
}

fn configure(ctx: &egui::Context) {
    theme::configure(ctx);
}

fn limited_label(
    ui: &mut egui::Ui,
    text: &str,
    width: f32,
    rows: usize,
    size: f32,
    color: Color32,
    bold: bool,
) -> egui::Response {
    let font = if bold {
        FontId::new(size, egui::FontFamily::Name("Moa Malgun Bold".into()))
    } else {
        FontId::proportional(size)
    };
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width.max(1.0));
    job.wrap.max_rows = rows;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.add(egui::Label::new(galley)).on_hover_text(text)
}

fn setting_heading(ui: &mut egui::Ui, icon: Icon, title: &str) {
    ui.set_width(ui.available_width());
    ui.horizontal(|ui| {
        theme::icon(ui, icon, 20.0, PURPLE);
        ui.label(theme::strong(title, 16.0));
    });
    ui.add_space(16.0);
}

fn setting_switch(ui: &mut egui::Ui, title: &str, description: &str, value: &mut bool) -> bool {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 62.0).max(80.0);
        ui.allocate_ui_with_layout(
            Vec2::new(width, 48.0),
            egui::Layout::top_down(Align::Min),
            |ui| {
                ui.label(theme::strong(title, 14.0));
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(RichText::new(description).size(12.0).color(MUTED));
            },
        );
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            theme::toggle(ui, value, title).changed()
        })
        .inner
    })
    .inner
}

fn setting_separator(ui: &mut egui::Ui) {
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(12.0);
}

fn panel() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(12)
        .inner_margin(20)
}
fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(Color32::WHITE))
        .fill(PURPLE)
        .stroke(egui::Stroke::NONE)
        .min_size(Vec2::new(100.0, 38.0))
}
fn texture(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Result<TextureHandle, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|_| "이미지를 읽지 못했습니다.")?
        .into_rgba8();
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    );
    Ok(ctx.load_texture(name, color, egui::TextureOptions::LINEAR))
}
fn data_texture(ctx: &egui::Context, name: &str, value: &str) -> Option<TextureHandle> {
    let (_, bytes) = value.split_once(',')?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes)
        .ok()?;
    texture(ctx, name, &bytes).ok()
}
fn paint_cover(ui: &egui::Ui, image: &TextureHandle, rect: egui::Rect) {
    let ratio = image.size_vec2().x / image.size_vec2().y;
    let target = rect.width() / rect.height();
    let uv = if ratio > target {
        let width = target / ratio;
        egui::Rect::from_min_max(
            egui::pos2((1.0 - width) * 0.5, 0.0),
            egui::pos2((1.0 + width) * 0.5, 1.0),
        )
    } else {
        let height = ratio / target;
        egui::Rect::from_min_max(
            egui::pos2(0.0, (1.0 - height) * 0.5),
            egui::pos2(1.0, (1.0 + height) * 0.5),
        )
    };
    egui::Image::new(image)
        .uv(uv)
        .corner_radius(egui::CornerRadius {
            nw: 12,
            ne: 12,
            sw: 0,
            se: 0,
        })
        .paint_at(ui, rect);
}
fn accent(value: &str) -> Color32 {
    if value.len() == 7
        && value.starts_with('#')
        && let Ok(bytes) = hex::decode(&value[1..])
    {
        return Color32::from_rgb(bytes[0], bytes[1], bytes[2]);
    }
    PURPLE
}
fn interval_text(minutes: u32) -> String {
    if minutes < 60 {
        format!("{minutes}분")
    } else {
        format!("{}시간", minutes / 60)
    }
}
fn version_badge(ui: &mut egui::Ui, app: &AppView) {
    if let Some(installed) = &app.installed {
        ui.label(
            RichText::new(format!("v{}", installed.version))
                .small()
                .color(PURPLE),
        )
        .on_hover_text(format!("설치 버전 {}", installed.version));
    }
}
fn button_text(app: &AppView) -> &str {
    if app.busy {
        "작업 중…"
    } else if app.definition.repository.is_empty() {
        "저장소 연결"
    } else if app.is_launcher {
        if has_update(app) {
            "업데이트 후 재시작"
        } else if app.release.is_some() {
            "최신 버전"
        } else {
            "업데이트 확인"
        }
    } else if app.installed.is_some() && !has_update(app) {
        "실행하기"
    } else if has_update(app) {
        "업데이트"
    } else if app.release.is_some() {
        "설치하기"
    } else {
        "릴리스 확인"
    }
}
fn form_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
}
fn optional_field(ui: &mut egui::Ui, label: &str, value: &mut Option<String>) {
    let mut text = value.clone().unwrap_or_default();
    ui.label(label);
    if ui
        .add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY))
        .changed()
    {
        *value = Some(text);
    }
}
fn normalize_repository(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("https://github.com/")
        .trim_start_matches("http://github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string()
}
fn empty_definition() -> AppDefinition {
    AppDefinition {
        id: String::new(),
        name: String::new(),
        description: String::new(),
        category: "유틸리티".into(),
        accent_color: "#6964E8".into(),
        monogram: "M".into(),
        repository: String::new(),
        is_private: false,
        icon_url: None,
        cover_url: None,
        packages: vec![PackageDefinition {
            architecture: "x64".into(),
            kind: "zip".into(),
            asset_pattern: String::new(),
            executable_path: String::new(),
            install_directory: None,
        }],
    }
}
