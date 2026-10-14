//! Look & feel: dark palette, adaptive accent, soft shadows, painted icons,
//! and the small custom widgets (scrubber, transport buttons, pills).

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke, Ui,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(18, 18, 20);
pub const SIDEBAR: Color32 = Color32::from_rgb(25, 25, 28);
pub const BAR: Color32 = Color32::from_rgb(30, 30, 34);
pub const ROW_ALT: Color32 = Color32::from_rgb(23, 23, 26);
pub const HOVER: Color32 = Color32::from_rgb(42, 42, 47);
pub const SEL: Color32 = Color32::from_rgb(52, 52, 58);
pub const TEXT: Color32 = Color32::from_rgb(240, 240, 245);
pub const DIM: Color32 = Color32::from_rgb(150, 150, 160);
pub const FAINT: Color32 = Color32::from_rgb(86, 86, 94);
pub const LINE: Color32 = Color32::from_rgb(40, 40, 45);

/// Default accent when nothing (or something greyscale) is playing: soft violet.
pub const DEFAULT_ACCENT: Color32 = Color32::from_rgb(150, 132, 255);

static ACCENT: AtomicU32 = AtomicU32::new(0x9684FF);

/// The live accent colour. Follows the cover art of whatever is playing.
pub fn accent() -> Color32 {
    let v = ACCENT.load(Ordering::Relaxed);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Text colour that reads on top of the accent (dark on light accents).
pub fn on_accent() -> Color32 {
    let a = accent();
    let lum = (0.299 * a.r() as f32 + 0.587 * a.g() as f32 + 0.114 * a.b() as f32) / 255.0;
    if lum > 0.62 {
        Color32::from_rgb(20, 20, 24)
    } else {
        Color32::WHITE
    }
}

pub fn set_accent(c: Color32) {
    ACCENT.store(((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32, Ordering::Relaxed);
}

/// Turn an album's average colour into a usable accent: saturated and bright
/// enough to read on dark backgrounds. Greys fall back to the default.
pub fn vivid(c: Color32) -> Color32 {
    let hsva = egui::ecolor::Hsva::from(c);
    if hsva.s < 0.12 {
        return DEFAULT_ACCENT;
    }
    egui::ecolor::Hsva::new(hsva.h, hsva.s.clamp(0.5, 0.72), 0.95, 1.0).into()
}

pub fn cr(r: u8) -> CornerRadius {
    CornerRadius::same(r)
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()), f(a.a(), b.a()))
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Ease-out cubic.
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Eased 0..1 that follows a boolean (hover, active, …).
pub fn anim(ui: &Ui, id: Id, on: bool, secs: f32) -> f32 {
    ease(ui.ctx().animate_bool_with_time(id, on, secs))
}

/// Real blurred drop shadow.
pub fn shadow(ui: &Ui, rect: Rect, radius: u8, blur: u8, offset_y: i8, strength: f32) {
    let s = egui::epaint::Shadow {
        offset: [0, offset_y],
        blur,
        spread: 0,
        color: Color32::from_black_alpha((strength.clamp(0.0, 1.0) * 255.0) as u8),
    };
    ui.painter().add(s.as_shape(rect, cr(radius)));
}

pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let load = |path: &str, index: u32| -> Option<egui::FontData> {
        let bytes = std::fs::read(path).ok()?;
        let mut fd = egui::FontData::from_owned(bytes);
        fd.index = index;
        Some(fd)
    };
    #[cfg(windows)]
    let (regular, bold, cjk) = (("C:\\Windows\\Fonts\\segoeui.ttf", 0), ("C:\\Windows\\Fonts\\segoeuib.ttf", 0), ("C:\\Windows\\Fonts\\YuGothM.ttc", 0));
    #[cfg(not(windows))]
    let (regular, bold, cjk) = (("/System/Library/Fonts/SFNS.ttf", 0), ("/System/Library/Fonts/HelveticaNeue.ttc", 1), ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0));
    if let Some(fd) = load(regular.0, regular.1) {
        fonts.font_data.insert("sf".into(), Arc::new(fd));
        fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "sf".into());
    }
    let mut bold_family = vec![];
    if let Some(fd) = load(bold.0, bold.1) {
        fonts.font_data.insert("sf-bold".into(), Arc::new(fd));
        bold_family.push("sf-bold".to_string());
    }
    // Japanese/Chinese fallback (kana + kanji), e.g. NieR composer names.
    if let Some(fd) = load(cjk.0, cjk.1) {
        fonts.font_data.insert("cjk".into(), Arc::new(fd));
        fonts.families.get_mut(&FontFamily::Proportional).unwrap().push("cjk".into());
    }
    bold_family.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    ctx.set_fonts(fonts);
}

/// Visuals depend on the accent, so this is re-applied when it changes.
pub fn apply_visuals(ctx: &egui::Context) {
    let a = accent();
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = Color32::from_rgb(34, 34, 38);
    v.extreme_bg_color = Color32::from_rgb(38, 38, 43);
    v.faint_bg_color = ROW_ALT;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = alpha(a, 0.45);
    v.selection.stroke = Stroke::new(1.0f32, a);
    v.hyperlink_color = a;
    v.window_corner_radius = cr(12);
    v.menu_corner_radius = cr(10);
    v.window_stroke = Stroke::new(1.0f32, Color32::from_gray(58));
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(140) };
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(120) };
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.corner_radius = cr(7);
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(44, 44, 50);
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 58, 66);
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.weak_bg_fill = Color32::from_rgb(70, 70, 78);
    v.widgets.open.weak_bg_fill = Color32::from_rgb(58, 58, 66);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0f32, LINE);
    ctx.set_visuals(v);
}

pub fn setup_style(ctx: &egui::Context) {
    setup_fonts(ctx);
    apply_visuals(ctx);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.scroll.bar_width = 7.0;
        s.spacing.scroll.floating = true;
        s.text_styles.insert(egui::TextStyle::Body, font(13.5));
        s.text_styles.insert(egui::TextStyle::Button, font(13.5));
        s.text_styles.insert(egui::TextStyle::Small, font(11.5));
        s.text_styles.insert(egui::TextStyle::Heading, bold(26.0));
        s.animation_time = 0.16;
    });
}

/// Single-line text, truncated with an ellipsis to `max_w`.
pub fn text(ui: &Ui, pos: Pos2, s: &str, f: FontId, color: Color32, max_w: f32) -> Rect {
    let mut job = LayoutJob::simple_singleline(s.to_owned(), f, color);
    job.wrap = TextWrapping {
        max_width: max_w.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let rect = Rect::from_min_size(pos, galley.size());
    ui.painter().galley(pos, galley, color);
    rect
}

pub fn text_right(ui: &Ui, right: Pos2, s: &str, f: FontId, color: Color32) {
    let galley = ui.painter().layout_no_wrap(s.to_owned(), f, color);
    ui.painter().galley(pos2(right.x - galley.size().x, right.y), galley, color);
}

#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Pause,
    Next,
    Prev,
}

pub fn paint_icon(ui: &Ui, icon: Icon, c: Pos2, s: f32, color: Color32) {
    let p = ui.painter();
    let tri = |x0: f32, x1: f32, h: f32| {
        Shape::convex_polygon(vec![pos2(x0, c.y - h), pos2(x1, c.y), pos2(x0, c.y + h)], color, Stroke::NONE)
    };
    match icon {
        Icon::Play => {
            p.add(tri(c.x - s * 0.36, c.x + s * 0.46, s * 0.48));
        }
        Icon::Pause => {
            for dx in [-s * 0.21, s * 0.21] {
                p.rect_filled(Rect::from_center_size(pos2(c.x + dx, c.y), vec2(s * 0.24, s * 0.9)), cr(2), color);
            }
        }
        Icon::Next => {
            let h = s * 0.34;
            p.add(tri(c.x - s * 0.5, c.x, h));
            p.add(tri(c.x - s * 0.02, c.x + s * 0.48, h));
        }
        Icon::Prev => {
            let h = s * 0.34;
            p.add(tri(c.x + s * 0.5, c.x, h));
            p.add(tri(c.x + s * 0.02, c.x - s * 0.48, h));
        }
    }
}

/// Transport button: soft circle on hover, shrinks a touch when pressed.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, hit: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(hit, hit), Sense::click());
    let h = anim(ui, resp.id, resp.hovered(), 0.15);
    let press = anim(ui, resp.id.with("p"), resp.is_pointer_button_down_on(), 0.06);
    if h > 0.0 {
        ui.painter().circle_filled(rect.center(), hit * 0.5, Color32::from_white_alpha((14.0 * h) as u8));
    }
    paint_icon(ui, icon, rect.center(), size * (1.0 - 0.1 * press), lerp(TEXT, Color32::WHITE, h));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Big round white play/pause.
pub fn play_button(ui: &mut Ui, playing: bool, d: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(d, d), Sense::click());
    let h = anim(ui, resp.id, resp.hovered(), 0.15);
    let press = anim(ui, resp.id.with("p"), resp.is_pointer_button_down_on(), 0.06);
    let r = d * 0.5 * (1.0 + 0.06 * h - 0.07 * press);
    shadow(ui, Rect::from_center_size(rect.center(), vec2(r * 2.0, r * 2.0)), r as u8, 14, 3, 0.2 + 0.3 * h);
    ui.painter().circle_filled(rect.center(), r, Color32::WHITE);
    let icon = if playing { Icon::Pause } else { Icon::Play };
    let off = if playing { 0.0 } else { d * 0.04 };
    paint_icon(ui, icon, rect.center() + vec2(off, 0.0), d * 0.38, BG);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Shuffle / repeat toggle: monochrome glyph, accent + dot when on.
pub fn glyph_toggle(ui: &mut Ui, glyph: &str, active: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    let on = anim(ui, resp.id.with("on"), active, 0.2);
    let h = anim(ui, resp.id, resp.hovered(), 0.15);
    let color = lerp(lerp(DIM, TEXT, h), accent(), on);
    ui.painter().text(rect.center() - vec2(0.0, 2.0), Align2::CENTER_CENTER, glyph, font(15.0), color);
    if on > 0.0 {
        ui.painter().circle_filled(pos2(rect.center().x, rect.bottom() - 3.0), 2.0 * on, alpha(accent(), on));
    }
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Thin scrubber that thickens on hover and grows a knob.
pub fn scrubber(ui: &mut Ui, width: f32, frac: f32, enabled: bool, fill: Color32) -> (Response, Option<f32>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 16.0), Sense::click_and_drag());
    let hot = anim(ui, resp.id, enabled && (resp.hovered() || resp.dragged()), 0.14);
    let h = 4.0 + 3.0 * hot;
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), h));
    let mut f = frac.clamp(0.0, 1.0);
    let mut out = None;
    if enabled {
        if let Some(p) = resp.interact_pointer_pos() {
            if resp.dragged() || resp.clicked() || resp.drag_stopped() {
                f = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
                out = Some(f);
            }
        }
    }
    ui.painter().rect_filled(track, cr(4), Color32::from_white_alpha(34));
    let mut filled = track;
    filled.set_right(track.left() + track.width() * f);
    ui.painter().rect_filled(filled, cr(4), lerp(alpha(fill, 0.85), fill, hot));
    if hot > 0.01 {
        ui.painter().circle_filled(pos2(filled.right(), track.center().y), 6.5 * hot, Color32::WHITE);
    }
    (resp.on_hover_cursor(egui::CursorIcon::PointingHand), out)
}

/// Rounded pill button (Play / Shuffle). Filled = accent background.
pub fn pill(ui: &mut Ui, icon: Option<&str>, label: &str, filled: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), bold(14.0), Color32::WHITE);
    let icon_w = if icon.is_some() { 22.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 44.0, 36.0).max(vec2(124.0, 36.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let h = anim(ui, resp.id, resp.hovered(), 0.15);
    let press = anim(ui, resp.id.with("p"), resp.is_pointer_button_down_on(), 0.06);
    let r = rect.shrink(2.0 * press);
    let a = accent();
    let bg = if filled {
        lerp(a, Color32::WHITE, 0.14 * h)
    } else {
        lerp(Color32::from_rgb(40, 40, 46), Color32::from_rgb(56, 56, 64), h)
    };
    if filled {
        shadow(ui, r, 10, 16, 4, 0.2 + 0.25 * h);
    }
    ui.painter().rect_filled(r, cr(10), bg);
    let fg = if filled { on_accent() } else { a };
    let total = galley.size().x + icon_w;
    let x0 = r.center().x - total / 2.0;
    match icon {
        Some("play") => paint_icon(ui, Icon::Play, pos2(x0 + 6.0, r.center().y), 12.0, fg),
        Some(g) => {
            ui.painter().text(pos2(x0 + 6.0, r.center().y), Align2::CENTER_CENTER, g, font(14.0), fg);
        }
        None => {}
    }
    ui.painter().text(pos2(x0 + icon_w, r.center().y), Align2::LEFT_CENTER, label, bold(14.0), fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn fmt_long(secs: f64) -> String {
    let m = (secs / 60.0).round() as u64;
    if m >= 60 {
        format!("{} hr {} min", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

// ------------------------------------------------------------ sidebar icons

#[derive(Clone, Copy)]
pub enum NavIcon {
    NowPlaying,
    Recent,
    Artists,
    Albums,
    Songs,
    Loved,
    Stats,
    Store,
    Playlist,
}

/// Small monochrome line icons for the sidebar, drawn at ~16px.
pub fn nav_icon(ui: &Ui, icon: NavIcon, c: Pos2, color: Color32) {
    let p = ui.painter();
    let st = Stroke::new(1.6f32, color);
    match icon {
        NavIcon::NowPlaying => {
            for (i, h) in [6.0, 11.0, 8.0, 13.0].iter().enumerate() {
                let x = c.x - 6.0 + i as f32 * 4.0;
                p.rect_filled(Rect::from_min_max(pos2(x - 1.0, c.y + 6.5 - h), pos2(x + 1.2, c.y + 6.5)), cr(1), color);
            }
        }
        NavIcon::Recent => {
            p.circle_stroke(c, 7.0, st);
            p.line_segment([c, c + vec2(0.0, -4.5)], st);
            p.line_segment([c, c + vec2(3.5, 2.0)], st);
        }
        NavIcon::Artists => {
            p.circle_stroke(c + vec2(0.0, -3.0), 3.4, st);
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
                    c + vec2(0.0, 7.5) + vec2(a.cos() * 6.5, a.sin() * 5.5)
                })
                .collect();
            p.add(Shape::line(pts, st));
        }
        NavIcon::Albums => {
            p.rect_stroke(Rect::from_center_size(c + vec2(1.5, 1.5), vec2(11.0, 11.0)), cr(2), st, egui::StrokeKind::Middle);
            p.line_segment([c + vec2(-6.5, 3.5), c + vec2(-6.5, -6.5)], st);
            p.line_segment([c + vec2(-6.5, -6.5), c + vec2(3.5, -6.5)], st);
        }
        NavIcon::Songs => {
            p.text(c, Align2::CENTER_CENTER, "♪", font(15.0), color);
        }
        NavIcon::Loved => {
            p.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, "♥", font(14.0), color);
        }
        NavIcon::Store => {
            // Shopping bag.
            p.rect_stroke(Rect::from_min_max(c + vec2(-6.5, -3.0), c + vec2(6.5, 7.0)), cr(2), st, egui::StrokeKind::Middle);
            let handle: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 10.0);
                    c + vec2(0.0, -3.0) + vec2(a.cos() * 3.5, a.sin() * 4.0)
                })
                .collect();
            p.add(Shape::line(handle, st));
        }
        NavIcon::Stats => {
            for (i, h) in [5.0, 9.0, 13.0].iter().enumerate() {
                let x = c.x - 5.0 + i as f32 * 5.0;
                p.rect_stroke(Rect::from_min_max(pos2(x - 1.6, c.y + 6.5 - h), pos2(x + 1.6, c.y + 6.5)), cr(1), Stroke::new(1.4f32, color), egui::StrokeKind::Middle);
            }
        }
        NavIcon::Playlist => {
            for i in 0..3 {
                let y = c.y - 4.0 + i as f32 * 4.0;
                p.line_segment([pos2(c.x - 6.0, y), pos2(c.x + (if i == 2 { 1.0 } else { 6.0 }), y)], st);
            }
        }
    }
}
