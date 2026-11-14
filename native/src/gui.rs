use crate::{
    engine::{AppView, EngineEvent, SharedEngine, Snapshot, has_update},
    library::{self, Filter},
    models::{AppDefinition, Catalog, PackageDefinition},
    platform::InstanceGuard,
    storage::Paths,
    windows::{NativeEvent, Tray},
};
use base64::Engine;
use eframe::egui::{self, Align, Color32, FontId, RichText, TextureHandle, Vec2};
use std::{
    collections::HashMap,
    sync::{Arc, mpsc::Receiver},
};

const PURPLE: Color32 = Color32::from_rgb(102, 89, 224);
const BACKGROUND: Color32 = Color32::from_rgb(245, 246, 251);
const MUTED: Color32 = Color32::from_rgb(116, 122, 140);
const BORDER: Color32 = Color32::from_rgb(229, 231, 240);

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
}

impl NativeApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: Paths,
        runtime: Arc<tokio::runtime::Runtime>,
        guard: InstanceGuard,
        background: bool,
    ) -> Result<Self, String> {
        configure(&cc.egui_ctx);
        let context = cc.egui_ctx.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || context.request_repaint());
        let engine = SharedEngine::new(paths, runtime.handle().clone(), wake.clone())?;
        engine.set_background(background);
        let (tray, native_events) = Tray::new(guard.event_handle(), wake)?;
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
        engine.start();
        Ok(Self {
            tray,
            _guard: guard,
            engine,
            _runtime: runtime,
            native_events,
            snapshot,
            artwork: HashMap::new(),
            generation: u64::MAX,
            launcher_icon,
            launcher_cover,
            page: Page::Library,
            filter: Filter::All,
            query: String::new(),
            selected: None,
            editor: None,
            imported: None,
            notified: vec![],
            password: String::new(),
            error: String::new(),
            visible: !background,
            quitting: false,
            catalog_pending: false,
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

    fn header(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(28, 20)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(&self.launcher_icon)
                            .fit_to_exact_size(Vec2::splat(34.0))
                            .corner_radius(8),
                    );
                    if ui
                        .add(
                            egui::Button::new(RichText::new("moa").size(24.0).strong())
                                .frame(false),
                        )
                        .clicked()
                    {
                        self.page = Page::Library;
                        self.filter = Filter::All;
                    }
                    ui.label(RichText::new("Launcher").small().color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .selectable_label(self.page == Page::Settings, "설정")
                            .clicked()
                        {
                            self.page = Page::Settings;
                        }
                        if ui
                            .selectable_label(self.page == Page::Library, "프로그램")
                            .clicked()
                        {
                            self.page = Page::Library;
                            self.filter = Filter::All;
                        }
                    });
                });
            });
    }
    fn footer(&self, ui: &mut egui::Ui) {
        egui::Panel::bottom("footer")
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(25, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&self.snapshot.status).size(11.0).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(
                                self.snapshot
                                    .last_checked
                                    .as_ref()
                                    .map(|time| format!("마지막 확인 {time}"))
                                    .unwrap_or_else(|| "아직 확인하지 않음".into()),
                            )
                            .size(11.0)
                            .color(MUTED),
                        );
                    });
                });
            });
    }
    fn notices(&mut self, ui: &mut egui::Ui) {
        if let Some(result) = self.snapshot.update_result.clone() {
            egui::Frame::new()
                .fill(Color32::from_rgb(236, 235, 255))
                .inner_margin(12)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&result.message);
                        if ui.small_button("닫기").clicked() {
                            let result = self.engine.dismiss_update_result();
                            self.report(result);
                        }
                    });
                });
            ui.add_space(12.0);
        }
        if !self.error.is_empty() {
            egui::Frame::new()
                .fill(Color32::from_rgb(255, 237, 239))
                .inner_margin(12)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(Color32::from_rgb(160, 48, 65), &self.error);
                        if ui.small_button("닫기").clicked() {
                            self.error.clear();
                        }
                    });
                });
            ui.add_space(12.0);
        }
    }
    fn library(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        let updates = library::update_count(&self.snapshot.apps);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("MY WORKSPACE").size(10.0).color(MUTED));
                ui.label(
                    RichText::new(match self.filter {
                        Filter::Favorites => "즐겨찾기",
                        Filter::Updates => "업데이트",
                        _ => "내 프로그램",
                    })
                    .size(26.0)
                    .strong(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("+ 프로그램 추가"))
                    .clicked()
                {
                    self.edit(None);
                }
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(if self.snapshot.checking {
                            "확인 중…"
                        } else {
                            "업데이트 확인"
                        }),
                    )
                    .clicked()
                {
                    let result = self.engine.check_updates();
                    self.report(result);
                }
                if ui
                    .add_enabled(
                        !busy && updates > 0,
                        primary_button(format!(
                            "모두 업데이트{}",
                            if updates > 0 {
                                format!(" ({updates})")
                            } else {
                                String::new()
                            }
                        )),
                    )
                    .clicked()
                {
                    let result = self.engine.update_all();
                    self.report(result);
                }
            });
        });
        ui.add_space(18.0);
        ui.horizontal_wrapped(|ui| {
            for (filter, label) in [
                (Filter::All, "전체"),
                (Filter::Favorites, "즐겨찾기"),
                (Filter::Installed, "설치됨"),
                (Filter::Updates, "업데이트"),
            ] {
                if ui.selectable_label(self.filter == filter, label).clicked() {
                    self.filter = filter;
                }
            }
            ui.separator();
            if ui
                .selectable_label(!self.snapshot.settings.use_grid_view, "리스트")
                .clicked()
            {
                self.save_patch(serde_json::json!({"useGridView":false}));
            }
            if ui
                .selectable_label(self.snapshot.settings.use_grid_view, "그리드")
                .clicked()
            {
                self.save_patch(serde_json::json!({"useGridView":true}));
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("프로그램 검색")
                    .desired_width(180.0),
            );
        });
        ui.add_space(12.0);
        if self.snapshot.batch_updating {
            ui.horizontal_wrapped(|ui| {
                ui.label("일반 도구부터 업데이트하며 런처는 마지막에 진행합니다.");
                if ui.button("남은 업데이트 취소").clicked() {
                    self.engine.cancel(None);
                }
            });
            ui.add_space(8.0);
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
        ui.label(
            RichText::new(format!("{}개 도구", apps.len()))
                .small()
                .color(MUTED),
        );
        ui.add_space(12.0);
        if apps.is_empty() {
            ui.add_space(55.0);
            ui.vertical_centered(|ui| {
                ui.heading(if self.filter == Filter::Favorites {
                    "자주 쓰는 도구를 모아 보세요"
                } else {
                    "표시할 프로그램이 없습니다"
                });
                ui.label(if self.filter == Filter::Favorites {
                    "도구의 별 버튼을 누르면 여기에 표시합니다."
                } else {
                    "검색어나 필터를 바꿔 보세요."
                });
            });
        } else if self.snapshot.settings.use_grid_view {
            let columns = if ui.available_width() >= 910.0 { 3 } else { 2 };
            let width = (ui.available_width() - 18.0 * (columns - 1) as f32) / columns as f32;
            for row in apps.chunks(columns) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    for app in row {
                        ui.allocate_ui_with_layout(
                            Vec2::new(width, 342.0),
                            egui::Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_width(width);
                                self.card(ui, app, busy);
                            },
                        );
                    }
                });
                ui.add_space(16.0);
            }
        } else {
            for app in apps {
                self.list_row(ui, &app, busy);
                ui.add_space(10.0);
            }
        }
    }
    fn card(&mut self, ui: &mut egui::Ui, app: &AppView, busy: bool) {
        let width = ui.available_width();
        panel().inner_margin(0).show(ui, |ui| {
            ui.set_width(width);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 112.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 12, accent(&app.definition.accent_color));
            if let Some(image) = self.image(app, true) {
                paint_cover(ui, image, rect);
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &app.definition.monogram,
                    FontId::proportional(40.0),
                    Color32::from_white_alpha(100),
                );
            }
            let star_rect = egui::Rect::from_min_size(
                rect.right_top() + Vec2::new(-42.0, 10.0),
                Vec2::splat(30.0),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(star_rect), |ui| {
                self.star(ui, app);
            });
            ui.advance_cursor_after_rect(rect);
            egui::Frame::new().inner_margin(16).show(ui, |ui| {
                ui.set_width(width - 32.0);
                ui.horizontal(|ui| {
                    self.icon(ui, app, 38.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&app.definition.name).strong().size(14.0));
                        version_badge(ui, app);
                    });
                });
                ui.add_space(8.0);
                ui.add_sized(
                    [width - 32.0, 43.0],
                    egui::Label::new(
                        RichText::new(&app.definition.description)
                            .size(11.5)
                            .color(MUTED),
                    )
                    .wrap(),
                );
                if app.definition.is_private {
                    ui.label(
                        RichText::new("비공개 · 개인 카탈로그")
                            .size(10.0)
                            .color(PURPLE),
                    );
                } else {
                    ui.add_space(16.0);
                }
                self.status(ui, app);
                ui.add_space(8.0);
                self.action_button(ui, app, busy, width - 32.0);
                if ui
                    .add_sized(
                        [width - 32.0, 24.0],
                        egui::Button::new(RichText::new("상세 보기").small().color(MUTED))
                            .frame(false),
                    )
                    .clicked()
                {
                    self.selected = Some(app.definition.id.clone());
                }
            });
        });
    }
    fn list_row(&mut self, ui: &mut egui::Ui, app: &AppView, busy: bool) {
        panel().show(ui, |ui| {
            ui.horizontal(|ui| {
                self.star(ui, app);
                self.icon(ui, app, 40.0);
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - 260.0).max(180.0));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&app.definition.name).strong());
                        version_badge(ui, app);
                    });
                    ui.label(
                        RichText::new(&app.definition.description)
                            .size(11.0)
                            .color(MUTED),
                    );
                    self.status(ui, app);
                });
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("상세").clicked() {
                        self.selected = Some(app.definition.id.clone());
                    }
                    self.action_button(ui, app, busy, 122.0);
                });
            });
        });
    }
    fn star(&mut self, ui: &mut egui::Ui, app: &AppView) {
        let selected = self
            .snapshot
            .settings
            .favorite_app_ids
            .contains(&app.definition.id);
        if ui
            .add(
                egui::Button::new(
                    RichText::new(if selected { "★" } else { "☆" })
                        .size(21.0)
                        .color(if selected {
                            Color32::from_rgb(191, 150, 32)
                        } else {
                            MUTED
                        }),
                )
                .fill(Color32::from_white_alpha(235))
                .min_size(Vec2::splat(28.0)),
            )
            .on_hover_text(if selected {
                "즐겨찾기 해제"
            } else {
                "즐겨찾기 추가"
            })
            .clicked()
        {
            self.favorite(&app.definition.id);
        }
    }
    fn status(&self, ui: &mut egui::Ui, app: &AppView) {
        let message = if app.busy {
            app.progress
                .as_ref()
                .map(|progress| progress.stage.as_str())
                .unwrap_or("새 버전 확인 중…")
        } else if app.error.is_some() {
            "확인 필요 · 상세 보기에서 확인하세요"
        } else if has_update(app) {
            "업데이트 가능"
        } else if app.installed.is_some() {
            "사용 준비 완료"
        } else if app.definition.repository.is_empty() {
            "저장소 연결 필요"
        } else if app.release.is_some() {
            "설치 가능"
        } else {
            "릴리스 확인 전"
        };
        ui.label(
            RichText::new(message)
                .size(10.5)
                .color(if app.error.is_some() {
                    Color32::from_rgb(170, 67, 75)
                } else {
                    MUTED
                }),
        );
        if let Some(progress) = &app.progress
            && let Some(total) = progress.total_bytes.filter(|total| *total > 0)
        {
            ui.add(
                egui::ProgressBar::new(
                    (progress.bytes_received as f64 / total as f64).clamp(0.0, 1.0) as f32,
                )
                .desired_height(4.0),
            );
        }
    }
    fn action_button(&mut self, ui: &mut egui::Ui, app: &AppView, busy: bool, width: f32) {
        if app.busy && app.can_cancel {
            if ui
                .add_sized([width, 32.0], egui::Button::new("다운로드 취소"))
                .clicked()
            {
                self.engine.cancel(Some(app.definition.id.clone()));
            }
        } else if ui
            .add_enabled(
                !busy,
                primary_button(button_text(app)).min_size(Vec2::new(width, 32.0)),
            )
            .clicked()
        {
            self.primary(app);
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.set_max_width(800.0);
        ui.label(RichText::new("MAKE IT YOURS").size(10.0).color(MUTED));
        ui.label(RichText::new("설정").size(26.0).strong());
        ui.label(RichText::new("모아 런처를 내 작업 방식에 맞춰 보세요.").color(MUTED));
        ui.add_space(24.0);
        panel().show(ui, |ui| {
            ui.heading("시작 및 종료"); ui.add_space(12.0);
            let mut startup = self.snapshot.startup_enabled;
            if ui.checkbox(&mut startup, "Windows 시작 시 자동 실행").changed() { let result = self.engine.set_startup(startup); self.report(result); self.snapshot = self.engine.snapshot(); }
            ui.label(RichText::new("로그인하면 창 없이 트레이에서 시작합니다.").small().color(MUTED));
            ui.add_space(12.0); ui.label(RichText::new("창을 닫아도 트레이에서 실행됩니다. 완전히 끝내려면 트레이 메뉴의 ‘종료’를 선택하세요.").small().color(MUTED));
        });
        ui.add_space(16.0);
        panel().show(ui, |ui| {
            ui.heading("업데이트 확인");
            ui.add_space(12.0);
            let mut startup = self.snapshot.settings.check_on_startup;
            if ui
                .checkbox(&mut startup, "런처를 시작할 때 새 버전 확인")
                .changed()
            {
                self.save_patch(serde_json::json!({"checkOnStartup":startup}));
            }
            let mut periodic = self.snapshot.settings.periodic_checks;
            if ui
                .checkbox(&mut periodic, "실행 중 주기적으로 새 버전 확인")
                .changed()
            {
                self.save_patch(serde_json::json!({"periodicChecks":periodic}));
            }
            let mut interval = self.snapshot.settings.check_interval_minutes;
            ui.add_enabled_ui(periodic, |ui| {
                ui.horizontal(|ui| {
                    ui.label("확인 간격");
                    egui::ComboBox::from_id_salt("check-interval")
                        .selected_text(interval_text(interval))
                        .show_ui(ui, |ui| {
                            for value in [30, 60, 180, 360, 720, 1440] {
                                ui.selectable_value(&mut interval, value, interval_text(value));
                            }
                        });
                });
            });
            if interval != self.snapshot.settings.check_interval_minutes {
                self.save_patch(serde_json::json!({"checkIntervalMinutes":interval}));
            }
            ui.label(
                RichText::new("기본 1시간 · 런처가 실행 중일 때 확인합니다.")
                    .small()
                    .color(MUTED),
            );
            ui.add_space(8.0);
            let mut notifications = self.snapshot.settings.update_notifications;
            if ui.checkbox(&mut notifications, "업데이트 알림").changed() {
                self.save_patch(serde_json::json!({"updateNotifications":notifications}));
            }
            ui.label(
                RichText::new("새 버전을 찾으면 트레이 알림으로 알려줍니다.")
                    .small()
                    .color(MUTED),
            );
        });
        ui.add_space(16.0);
        panel().show(ui, |ui| {
            ui.horizontal(|ui| { ui.heading("GitHub 연결"); ui.colored_label(PURPLE, if self.snapshot.github_connected { "연결됨" } else { "연결 안 됨" }); });
            ui.label(RichText::new("인증 정보는 이 PC의 Windows 자격 증명 관리자에 보관합니다.").small().color(MUTED));
            ui.add_space(8.0); ui.add(egui::TextEdit::singleline(&mut self.password).password(true).hint_text("GitHub 개인 액세스 토큰").desired_width(f32::INFINITY));
            ui.label(RichText::new("Fine-grained 토큰에서 사용할 저장소를 선택하고 Contents: Read-only 권한을 지정하세요.").small().color(MUTED));
            let busy = self.busy();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!busy && !self.password.is_empty(), primary_button(if self.snapshot.github_connected { "연결 교체" } else { "연결" })).clicked() { let token = std::mem::take(&mut self.password); let result = self.engine.connect_github(token); self.report(result); }
                if ui.add_enabled(!busy && self.snapshot.github_connected, egui::Button::new("연결 해제")).clicked() { let result = self.engine.disconnect_github(); self.report(result); }
                if ui.button("토큰 발급 페이지").clicked() { let result = self.engine.open_token_page(); self.report(result); }
            });
        });
        ui.add_space(16.0);
        panel().show(ui, |ui| {
            ui.heading("프로그램과 데이터"); ui.label(RichText::new(&self.snapshot.data_directory).small().color(MUTED));
            ui.horizontal_wrapped(|ui| {
                if ui.button("데이터 폴더").clicked() { let result = self.engine.open_location(None); self.report(result); }
                if ui.add_enabled(!self.busy(), egui::Button::new("카탈로그 가져오기")).clicked() && let Some(path) = rfd::FileDialog::new().add_filter("앱 카탈로그", &["json"]).pick_file() { self.error.clear(); self.catalog_pending = true; let result = self.engine.preview_import(path); self.report(result); }
                if ui.add_enabled(!self.busy(), egui::Button::new("카탈로그 내보내기")).clicked() && let Some(path) = rfd::FileDialog::new().add_filter("앱 카탈로그", &["json"]).set_file_name("moa-catalog.json").save_file() { let result = self.engine.export_catalog(path); self.report(result); }
            });
            ui.label(RichText::new("비공개 앱과 즐겨찾기는 이 PC에만 저장됩니다. 내보내기에는 공개 여부를 확인한 앱만 포함합니다.").small().color(MUTED));
        });
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Moa Launcher {}", self.snapshot.version))
                    .small()
                    .color(MUTED),
            );
            if ui.button("런처 종료").clicked() {
                self.quit(ui.ctx());
            }
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
                ui.set_width(560.0_f32.min(ctx.content_rect().width() - 70.0));
                ui.horizontal(|ui| { self.icon(ui, &app, 40.0); ui.heading(&app.definition.name); });
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 230.0).max(190.0)).show(ui, |ui| {
                    ui.label(&app.definition.description); ui.add_space(10.0);
                    if let Some(image) = self.image(&app, true) { let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 160.0), egui::Sense::hover()); paint_cover(ui, image, rect); }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| { ui.label(format!("설치 버전  {}", app.installed.as_ref().map(|installed| installed.version.as_str()).unwrap_or("미설치"))); ui.separator(); ui.label(format!("최신 버전  {}", app.release.as_ref().map(|release| release.tag.as_str()).unwrap_or("확인 전"))); });
                    if let Some(error) = &app.error { ui.colored_label(Color32::from_rgb(170, 55, 66), error); }
                    ui.add_space(12.0); ui.heading("릴리스 노트");
                    if app.is_cached { ui.label(RichText::new("오프라인 캐시 · 설치 전에 최신 배포 정보를 다시 확인합니다.").small().color(MUTED)); }
                    if app.definition.is_private { ui.label(RichText::new("비공개 · 개인 카탈로그").small().color(PURPLE)); }
                    ui.label(app.release.as_ref().map(|release| release.notes.as_str()).filter(|text| !text.is_empty()).unwrap_or("업데이트 확인을 누르면 배포 정보를 가져옵니다."));
                    ui.add_space(12.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("GitHub 릴리스").clicked() { let result = self.engine.open_github(&app.definition.id); self.report(result); }
                        if ui.add_enabled(app.installed.is_some(), egui::Button::new("설치 폴더")).clicked() { let result = self.engine.open_location(Some(app.definition.id.clone())); self.report(result); }
                        if !app.is_launcher && ui.add_enabled(!self.busy(), egui::Button::new("프로그램 설정")).clicked() { self.edit(Some(&app.definition)); }
                    });
                });
                ui.add_space(14.0);
                ui.horizontal(|ui| { if ui.button("닫기").clicked() { close = true; } self.action_button(ui, &app, self.busy(), 170.0); });
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
    }
}

impl eframe::App for NativeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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
        let _ = self.engine.request_exit();
    }
}

fn configure(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::light());
    ctx.set_theme(egui::Theme::Light);
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.spacing.item_spacing = Vec2::new(10.0, 9.0);
    style.spacing.button_padding = Vec2::new(13.0, 8.0);
    style.visuals.selection.bg_fill = Color32::from_rgb(233, 230, 255);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, PURPLE);
    style.visuals.widgets.inactive.corner_radius = 7.into();
    style.visuals.widgets.hovered.corner_radius = 7.into();
    style.visuals.widgets.active.corner_radius = 7.into();
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(18.0));
    ctx.set_style_of(egui::Theme::Light, style);
    if let Some(windows) = std::env::var_os("WINDIR")
        && let Ok(bytes) = std::fs::read(std::path::PathBuf::from(windows).join("Fonts/malgun.ttf"))
    {
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("korean".into(), egui::FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "korean".into());
        ctx.set_fonts(fonts);
    }
}
fn panel() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(12)
        .inner_margin(20)
}
fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(Color32::WHITE)).fill(PURPLE)
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
