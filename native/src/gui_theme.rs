//! Shared visual language and small, accessible egui components.

use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Response, RichText,
    Role, Sense, Stroke, StrokeKind, TextStyle, Ui, Vec2, WidgetInfo,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub(super) const ACCENT: Color32 = Color32::from_rgb(98, 89, 217);
pub(super) const BACKGROUND: Color32 = Color32::from_rgb(246, 247, 251);
pub(super) const SURFACE: Color32 = Color32::WHITE;
pub(super) const TEXT: Color32 = Color32::from_rgb(32, 38, 54);
pub(super) const MUTED: Color32 = Color32::from_rgb(104, 113, 132);
pub(super) const BORDER: Color32 = Color32::from_rgb(231, 234, 240);
pub(super) const ACCENT_SOFT: Color32 = Color32::from_rgb(238, 236, 252);
pub(super) const SUCCESS: Color32 = Color32::from_rgb(34, 126, 91);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Icon {
    Library,
    Star,
    Installed,
    Refresh,
    Settings,
    Search,
    Grid,
    List,
    Plus,
    ChevronRight,
    Download,
    Check,
    Lock,
    External,
    Close,
    Folder,
    Github,
    Power,
}

/// Install Malgun Gothic regular and bold as named families when Windows provides them.
pub(super) fn configure(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let Some(windows) = std::env::var_os("WINDIR") else {
        return configure_style(ctx, fonts);
    };
    let directory = PathBuf::from(windows).join("Fonts");
    let regular = directory.join("malgun.ttf");
    let bold = directory.join("malgunbd.ttf");
    let mut proportional_families = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut regular_name = None;
    if let Ok(bytes) = std::fs::read(regular) {
        let family: Arc<str> = "Moa Malgun".into();
        regular_name = Some(family.to_string());
        fonts
            .font_data
            .insert(family.to_string(), FontData::from_owned(bytes).into());
        fonts
            .families
            .insert(FontFamily::Name(family.clone()), vec![family.to_string()]);
        proportional_families.insert(0, family.to_string());
    }
    let mut bold_name = "Moa Malgun Bold".to_owned();
    if let Ok(bytes) = std::fs::read(bold) {
        let family: Arc<str> = "Moa Malgun Bold".into();
        bold_name = family.to_string();
        fonts
            .font_data
            .insert(family.to_string(), FontData::from_owned(bytes).into());
    }
    let bold_fallback = regular_name
        .map(|name| vec![name])
        .unwrap_or_else(|| proportional_families.clone());
    fonts.families.insert(
        FontFamily::Name(Arc::from(bold_name.clone())),
        if fonts.font_data.contains_key(&bold_name) {
            vec![bold_name]
        } else {
            bold_fallback
        },
    );
    fonts
        .families
        .insert(FontFamily::Proportional, proportional_families);
    configure_style(ctx, fonts);
}

fn configure_style(ctx: &egui::Context, fonts: FontDefinitions) {
    ctx.set_visuals(egui::Visuals::light());
    ctx.set_theme(egui::Theme::Light);
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.spacing.item_spacing = Vec2::new(12.0, 10.0);
    style.spacing.button_padding = Vec2::new(14.0, 9.0);
    style.spacing.interact_size = Vec2::new(36.0, 38.0);
    style.spacing.icon_width = 18.0;
    style.spacing.icon_width_inner = 12.0;
    style.spacing.icon_spacing = 9.0;
    style.visuals.dark_mode = false;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.weak_text_color = Some(MUTED);
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = SURFACE;
    style.visuals.extreme_bg_color = SURFACE;
    style.visuals.faint_bg_color = BACKGROUND;
    style.visuals.text_edit_bg_color = Some(SURFACE);
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.selection.bg_fill = ACCENT_SOFT;
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    style.visuals.window_corner_radius = 12.into();
    style.visuals.menu_corner_radius = 10.into();
    style.visuals.window_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.noninteractive.bg_fill = SURFACE;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.inactive.bg_fill = SURFACE;
    style.visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.hovered.bg_fill = BACKGROUND;
    style.visuals.widgets.hovered.weak_bg_fill = ACCENT_SOFT;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.active.bg_fill = ACCENT_SOFT;
    style.visuals.widgets.active.weak_bg_fill = ACCENT_SOFT;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.open.bg_fill = ACCENT_SOFT;
    style.visuals.widgets.open.weak_bg_fill = ACCENT_SOFT;
    style.visuals.widgets.open.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.open.fg_stroke = Stroke::new(1.0, ACCENT);
    for widgets in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widgets.corner_radius = 9.into();
    }

    let proportional = FontFamily::Proportional;
    style.text_styles = BTreeMap::from([
        (TextStyle::Small, FontId::new(12.0, proportional.clone())),
        (TextStyle::Body, FontId::new(14.0, proportional.clone())),
        (TextStyle::Button, FontId::new(14.0, proportional.clone())),
        (TextStyle::Heading, FontId::new(20.0, proportional.clone())),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]);
    ctx.set_style_of(egui::Theme::Light, style);
    ctx.set_fonts(fonts);
}

pub(super) fn strong(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text)
        .font(FontId::new(
            size,
            FontFamily::Name("Moa Malgun Bold".into()),
        ))
        .color(TEXT)
}

fn draw_arc(
    painter: &egui::Painter,
    point: impl Fn(f32, f32) -> Pos2,
    center: (f32, f32),
    radius: f32,
    start: f32,
    end: f32,
    stroke: Stroke,
) {
    let segments = (((end - start).abs() * radius) / 2.0).ceil().max(6.0) as usize;
    let points = (0..=segments)
        .map(|index| {
            let angle = start + (end - start) * index as f32 / segments as f32;
            point(
                center.0 + radius * angle.cos(),
                center.1 + radius * angle.sin(),
            )
        })
        .collect::<Vec<_>>();
    painter.add(egui::Shape::line(points, stroke));
}

/// Paint a consistent 1.7 px outline icon, scaled to the supplied rectangle.
pub(super) fn draw_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let side = rect.width().min(rect.height());
    if side <= 0.0 {
        return;
    }
    let scale = side / 24.0;
    let origin = rect.center() - Vec2::splat(12.0 * scale);
    let p = |x: f32, y: f32| origin + Vec2::new(x * scale, y * scale);
    let stroke = Stroke::new((1.7 * scale).max(1.0), color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([p(a.0, a.1), p(b.0, b.1)], stroke);
    };
    let poly = |points: &[(f32, f32)]| {
        for pair in points.windows(2) {
            line(pair[0], pair[1]);
        }
    };
    let circle = |x: f32, y: f32, radius: f32| {
        painter.circle_stroke(p(x, y), radius * scale, stroke);
    };
    let rect_at = |left: f32, top: f32, right: f32, bottom: f32, radius: f32| {
        painter.rect_stroke(
            Rect::from_min_max(p(left, top), p(right, bottom)),
            radius * scale,
            stroke,
            StrokeKind::Inside,
        );
    };
    match icon {
        Icon::Library => {
            rect_at(3.5, 4.0, 20.5, 20.0, 3.0);
            line((8.0, 9.0), (16.0, 9.0));
            line((8.0, 12.0), (16.0, 12.0));
            line((8.0, 15.0), (13.0, 15.0));
        }
        Icon::Star => {
            let points = (0..=10)
                .map(|index| {
                    let angle =
                        -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.0;
                    let radius = if index % 2 == 0 { 8.5 } else { 3.8 };
                    (12.0 + angle.cos() * radius, 12.0 + angle.sin() * radius)
                })
                .collect::<Vec<_>>();
            poly(&points);
        }
        Icon::Installed => {
            circle(12.0, 12.0, 8.0);
            poly(&[(8.0, 12.0), (10.5, 14.5), (16.0, 9.0)]);
        }
        Icon::Refresh => {
            poly(&[(19.0, 8.0), (19.0, 4.0), (15.0, 4.0)]);
            poly(&[(5.0, 16.0), (5.0, 20.0), (9.0, 20.0)]);
            draw_arc(painter, p, (12.0, 12.0), 7.0, -0.6, 2.6, stroke);
            draw_arc(painter, p, (12.0, 12.0), 7.0, 2.55, 5.7, stroke);
        }
        Icon::Settings => {
            circle(12.0, 12.0, 7.0);
            circle(12.0, 12.0, 2.6);
            for angle in 0..8 {
                let a = angle as f32 * std::f32::consts::PI / 4.0;
                let (sin, cos) = a.sin_cos();
                line(
                    (12.0 + cos * 8.0, 12.0 + sin * 8.0),
                    (12.0 + cos * 10.0, 12.0 + sin * 10.0),
                );
            }
        }
        Icon::Search => {
            circle(10.5, 10.5, 6.2);
            line((15.0, 15.0), (20.0, 20.0));
        }
        Icon::Grid => {
            for (x, y) in [(4.0, 4.0), (13.0, 4.0), (4.0, 13.0), (13.0, 13.0)] {
                rect_at(x, y, x + 7.0, y + 7.0, 1.5);
            }
        }
        Icon::List => {
            for y in [6.0, 12.0, 18.0] {
                circle(5.0, y, 0.8);
                line((9.0, y), (20.0, y));
            }
        }
        Icon::Plus => {
            line((12.0, 4.0), (12.0, 20.0));
            line((4.0, 12.0), (20.0, 12.0));
        }
        Icon::ChevronRight => poly(&[(9.0, 5.0), (16.0, 12.0), (9.0, 19.0)]),
        Icon::Download => {
            line((12.0, 3.0), (12.0, 15.0));
            poly(&[(7.0, 10.0), (12.0, 15.0), (17.0, 10.0)]);
            poly(&[(5.0, 16.0), (5.0, 20.0), (19.0, 20.0), (19.0, 16.0)]);
        }
        Icon::Check => poly(&[(4.0, 12.0), (9.0, 17.0), (20.0, 6.0)]),
        Icon::Lock => {
            rect_at(5.0, 10.0, 19.0, 21.0, 2.0);
            draw_arc(
                painter,
                p,
                (12.0, 10.0),
                5.0,
                std::f32::consts::PI,
                std::f32::consts::TAU,
                stroke,
            );
            circle(12.0, 15.0, 0.8);
        }
        Icon::External => {
            rect_at(4.0, 8.0, 16.0, 20.0, 2.0);
            poly(&[(11.0, 4.0), (20.0, 4.0), (20.0, 13.0)]);
            line((20.0, 4.0), (11.0, 13.0));
        }
        Icon::Close => {
            line((6.0, 6.0), (18.0, 18.0));
            line((18.0, 6.0), (6.0, 18.0));
        }
        Icon::Folder => {
            poly(&[
                (3.0, 7.0),
                (10.0, 7.0),
                (12.0, 10.0),
                (21.0, 10.0),
                (19.0, 19.0),
                (4.0, 19.0),
                (3.0, 7.0),
            ]);
        }
        Icon::Github => {
            circle(12.0, 12.0, 8.5);
            circle(9.0, 10.0, 0.7);
            circle(15.0, 10.0, 0.7);
            poly(&[(8.0, 17.0), (10.0, 15.0), (14.0, 15.0), (16.0, 17.0)]);
        }
        Icon::Power => {
            draw_arc(painter, p, (12.0, 13.0), 8.0, 0.35, 5.95, stroke);
            line((12.0, 3.0), (12.0, 13.0));
        }
    }
}

pub(super) fn icon(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    draw_icon(ui.painter(), rect, icon, color);
    response
}

pub(super) fn button(
    ui: &mut Ui,
    label: &str,
    leading: Option<Icon>,
    primary: bool,
    enabled: bool,
) -> Response {
    let enabled = enabled && ui.is_enabled();
    let font = FontId::new(14.0, FontFamily::Proportional);
    let text = ui.painter().layout_no_wrap(label.to_owned(), font, TEXT);
    let leading_space = if leading.is_some() { 20.0 } else { 0.0 };
    let width = (text.size().x + leading_space + 30.0)
        .max(76.0)
        .min(ui.available_width());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 38.0), Sense::hover());
    let response = ui.interact(
        rect,
        ui.auto_id_with(("moa-button", label)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let fill = if !enabled {
        BACKGROUND
    } else if primary {
        if response.is_pointer_button_down_on() {
            ACCENT.linear_multiply(0.82)
        } else if response.hovered() || response.has_focus() {
            Color32::from_rgb(83, 74, 202)
        } else {
            ACCENT
        }
    } else if response.is_pointer_button_down_on() || response.hovered() || response.has_focus() {
        ACCENT_SOFT
    } else {
        SURFACE
    };
    let color = if !enabled {
        MUTED
    } else if primary {
        Color32::WHITE
    } else {
        TEXT
    };
    ui.painter().rect_filled(rect, 9, fill);
    if !primary {
        ui.painter()
            .rect_stroke(rect, 9, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    }
    let content_width = text.size().x + leading_space;
    let start = rect.center().x - content_width * 0.5;
    if let Some(icon) = leading {
        draw_icon(
            ui.painter(),
            Rect::from_center_size(Pos2::new(start + 8.0, rect.center().y), Vec2::splat(16.0)),
            icon,
            color,
        );
    }
    ui.painter().galley_with_override_text_color(
        Pos2::new(start + leading_space, rect.center().y - text.size().y * 0.5),
        text,
        color,
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    response
}

pub(super) fn icon_button(ui: &mut Ui, icon: Icon, selected: bool, label: &str) -> Response {
    let enabled = ui.is_enabled();
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
    let response = ui.interact(
        rect,
        ui.auto_id_with(("moa-icon-button", label)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let background = if selected {
        ACCENT_SOFT
    } else if response.hovered() {
        BACKGROUND
    } else {
        SURFACE
    };
    ui.painter().rect_filled(rect, 8, background);
    if selected || response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            8,
            Stroke::new(1.0, if selected { ACCENT } else { BORDER }),
            StrokeKind::Inside,
        );
    }
    let color = if !enabled {
        MUTED
    } else if selected {
        ACCENT
    } else {
        MUTED
    };
    draw_icon(ui.painter(), rect.shrink(8.0), icon, color);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    response.on_hover_text(label)
}

pub(super) fn nav_item(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    count: Option<usize>,
    selected: bool,
) -> Response {
    let enabled = ui.is_enabled();
    let width = ui.available_width().max(0.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 42.0), Sense::hover());
    let response = ui.interact(
        rect,
        ui.auto_id_with(("moa-nav", label)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if enabled && (selected || response.hovered()) {
        ui.painter()
            .rect_filled(rect, 8, if selected { ACCENT_SOFT } else { BACKGROUND });
    }
    let foreground = if !enabled {
        MUTED
    } else if selected {
        ACCENT
    } else {
        MUTED
    };
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            8,
            Stroke::new(1.0, if selected { ACCENT } else { BORDER }),
            StrokeKind::Inside,
        );
    }
    draw_icon(
        ui.painter(),
        Rect::from_center_size(
            Pos2::new(rect.left() + 22.0, rect.center().y),
            Vec2::splat(18.0),
        ),
        icon,
        foreground,
    );
    let text = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(14.0, FontFamily::Proportional),
        if !enabled {
            MUTED
        } else if selected {
            ACCENT
        } else {
            TEXT
        },
    );
    ui.painter().galley(
        Pos2::new(rect.left() + 42.0, rect.center().y - text.size().y * 0.5),
        text,
        if !enabled {
            MUTED
        } else if selected {
            ACCENT
        } else {
            TEXT
        },
    );
    if let Some(count) = count.filter(|count| *count > 0) {
        let badge = count.to_string();
        let badge_galley = ui.painter().layout_no_wrap(
            badge,
            FontId::new(12.0, FontFamily::Proportional),
            foreground,
        );
        let badge_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 18.0, rect.center().y),
            Vec2::new((badge_galley.size().x + 14.0).max(26.0), 22.0),
        );
        ui.painter().rect_filled(
            badge_rect,
            11,
            if selected { Color32::WHITE } else { BACKGROUND },
        );
        ui.painter().galley(
            badge_rect.center() - badge_galley.size() * 0.5,
            badge_galley,
            foreground,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    response
}

pub(super) fn pill(ui: &mut Ui, text: &str, color: Color32, bg: Color32) -> Response {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(color))
        })
        .inner
}

pub(super) fn compact_pill(ui: &mut Ui, text: &str, color: Color32, bg: Color32) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(11.0), color);
    let size = Vec2::new(galley.size().x + 12.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 6, bg);
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, color);
    response.widget_info(|| WidgetInfo::labeled(Role::Label, ui.is_enabled(), text));
    response
}

pub(super) fn toggle(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let enabled = ui.is_enabled();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(42.0, 24.0), Sense::hover());
    let mut response = ui.interact(
        rect,
        ui.auto_id_with(("moa-toggle", label)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let fill = if !enabled {
        Color32::from_rgb(220, 223, 231)
    } else if *value {
        ACCENT
    } else {
        Color32::from_rgb(202, 207, 217)
    };
    ui.painter().rect_filled(rect, 12, fill);
    let radius = 8.0;
    let x = if *value {
        rect.right() - 12.0
    } else {
        rect.left() + 12.0
    };
    ui.painter()
        .circle_filled(Pos2::new(x, rect.center().y), radius, Color32::WHITE);
    response.widget_info(|| WidgetInfo::selected(Role::CheckBox, enabled, *value, label));
    response.on_hover_text(label)
}
