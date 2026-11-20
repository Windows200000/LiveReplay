//! Look & feel: a clean dark theme with card-style panels, plus small vector icons for stats.

use egui::{Color32, Pos2, Rect, Rounding, Shape, Stroke, Vec2};

pub const BG: Color32 = Color32::from_rgb(14, 17, 22);
pub const PANEL: Color32 = Color32::from_rgb(20, 24, 31);
pub const CARD: Color32 = Color32::from_rgb(27, 32, 41);
pub const CARD_HOVER: Color32 = Color32::from_rgb(35, 41, 52);
pub const BORDER: Color32 = Color32::from_rgb(40, 47, 59);
pub const TEXT: Color32 = Color32::from_rgb(222, 226, 233);
pub const MUTED: Color32 = Color32::from_rgb(132, 141, 156);
pub const ACCENT: Color32 = Color32::from_rgb(255, 122, 89);

/// Module / crew kinds that can show as damaged / destroyed, in display order (the game's names).
pub const CRIT_KINDS: [&str; 13] =
    ["ammoBay", "engine", "fuelTank", "gun", "turretRotator", "track", "radio", "surveyingDevice", "commander", "driver", "gunner", "loader", "radioman"];

/// The game's crit icon for a kind and state (1 = damaged, 2 = destroyed); crew only have "destroyed".
pub fn crit_icon_path(kind: &str, state: u8) -> String {
    let crew = matches!(kind, "commander" | "driver" | "gunner" | "loader" | "radioman");
    let suffix = if state >= 2 || crew { "destroyed" } else { "critical" };
    format!("gui/maps/icons/library/crits/{}{suffix}small.png", kind.to_lowercase())
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.extreme_bg_color = BG;
    v.faint_bg_color = Color32::from_rgb(23, 27, 35);
    v.window_rounding = Rounding::same(10.0);
    v.menu_rounding = Rounding::same(8.0);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    for (w, fill) in [
        (&mut v.widgets.noninteractive, PANEL),
        (&mut v.widgets.inactive, CARD),
        (&mut v.widgets.hovered, CARD_HOVER),
        (&mut v.widgets.active, CARD_HOVER),
        (&mut v.widgets.open, CARD_HOVER),
    ] {
        w.rounding = Rounding::same(6.0);
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, BORDER);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    // Slider rails / checkbox boxes use this; lighter than the cards so they stay visible.
    v.widgets.inactive.bg_fill = Color32::from_rgb(56, 64, 78);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.window_margin = egui::Margin::same(12.0);
    ctx.set_style(style);
}

/// A card frame for grouped content.
pub fn card() -> egui::Frame {
    egui::Frame::none().fill(CARD).rounding(Rounding::same(10.0)).stroke(Stroke::new(1.0, BORDER)).inner_margin(egui::Margin::same(10.0))
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Damage,
    Kills,
    Shots,
    Received,
    Health,
    Assist,
    Blocked,
    Spotted,
    Xp,
    Clock,
    /// A shell (shots seen).
    Shell,
}

impl Icon {
    pub const ALL: [Icon; 10] = [
        Icon::Damage,
        Icon::Kills,
        Icon::Shots,
        Icon::Received,
        Icon::Health,
        Icon::Assist,
        Icon::Blocked,
        Icon::Spotted,
        Icon::Xp,
        Icon::Clock,
    ];

    /// The client's battle-result icon for this stat (loaded at runtime, never shipped).
    pub fn game_path(&self) -> Option<&'static str> {
        Some(match self {
            // The client's glowing battle-efficiency icons (recoloured per stat when drawn).
            Icon::Damage => "gui/maps/icons/library/efficiency/48x48/damage.png",
            Icon::Kills => "gui/maps/icons/library/efficiency/48x48/destruction.png",
            Icon::Spotted => "gui/maps/icons/library/efficiency/48x48/detection.png",
            Icon::Assist => "gui/maps/icons/library/efficiency/48x48/help.png",
            Icon::Blocked => "gui/maps/icons/library/efficiency/48x48/armor.png",
            // Shots, XP, lifetime: no icon in that set, drawn as matching glowing vector icons.
            _ => return None,
        })
    }
}

/// Icon textures loaded from the client.
pub type IconTextures = std::collections::HashMap<Icon, egui::TextureHandle>;

/// Draw a small stat icon inside `r`.
pub fn icon(painter: &egui::Painter, r: Rect, icon: Icon, color: Color32) {
    let c = r.center();
    let s = r.width().min(r.height()) * 0.5;
    let st = Stroke::new((s * 0.18).max(1.2), color);
    match icon {
        Icon::Damage => {
            // crosshair
            painter.circle_stroke(c, s * 0.62, st);
            for d in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
                painter.line_segment([c + d * s * 0.3, c + d * s], st);
            }
        }
        Icon::Kills => {
            // skull-ish: round head + jaw + eyes
            painter.circle_filled(c - Vec2::new(0.0, s * 0.15), s * 0.62, color);
            painter.rect_filled(Rect::from_center_size(c + Vec2::new(0.0, s * 0.5), Vec2::new(s * 0.8, s * 0.4)), 2.0, color);
            for dx in [-0.25, 0.25] {
                painter.circle_filled(c + Vec2::new(dx * s, -s * 0.15), s * 0.17, CARD);
            }
        }
        Icon::Shell => {
            // shell: body + pointed nose
            let w = s * 0.36;
            painter.add(Shape::convex_polygon(
                vec![c + Vec2::new(-w, s * 0.9), c + Vec2::new(w, s * 0.9), c + Vec2::new(w, -s * 0.25), c + Vec2::new(0.0, -s * 0.95), c + Vec2::new(-w, -s * 0.25)],
                color,
                Stroke::NONE,
            ));
        }
        // Received uses the crosshair too (hits taken).
        Icon::Shots | Icon::Received => {
            // crosshair (accuracy)
            let st = Stroke::new(s * 0.16, color);
            painter.circle_stroke(c, s * 0.62, st);
            for d in [Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(0.0, -1.0)] {
                painter.line_segment([c + d * s * 0.35, c + d * s * 1.0], st);
            }
            painter.circle_filled(c, s * 0.12, color);
        }
        Icon::Health => {
            // plus
            painter.rect_filled(Rect::from_center_size(c, Vec2::new(s * 1.6, s * 0.55)), 2.0, color);
            painter.rect_filled(Rect::from_center_size(c, Vec2::new(s * 0.55, s * 1.6)), 2.0, color);
        }
        Icon::Assist => {
            // two linked rings
            painter.circle_stroke(c - Vec2::new(s * 0.32, 0.0), s * 0.5, st);
            painter.circle_stroke(c + Vec2::new(s * 0.32, 0.0), s * 0.5, st);
        }
        Icon::Blocked => {
            // shield
            painter.add(Shape::convex_polygon(
                vec![c + Vec2::new(-s * 0.75, -s * 0.8), c + Vec2::new(s * 0.75, -s * 0.8), c + Vec2::new(s * 0.75, s * 0.05), c + Vec2::new(0.0, s * 0.95), c + Vec2::new(-s * 0.75, s * 0.05)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Spotted => {
            // eye
            let pts: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 24.0;
                    c + Vec2::new(a.cos() * s, a.sin() * s * 0.5)
                })
                .collect();
            painter.add(Shape::closed_line(pts, st));
            painter.circle_filled(c, s * 0.32, color);
        }
        Icon::Xp => {
            // star
            let pts: Vec<Pos2> = (0..10)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    let rr = if i % 2 == 0 { s } else { s * 0.42 };
                    c + Vec2::new(a.cos(), a.sin()) * rr
                })
                .collect();
            painter.add(Shape::closed_line(pts, st));
        }
        Icon::Clock => {
            painter.circle_stroke(c, s * 0.85, st);
            painter.line_segment([c, c - Vec2::new(0.0, s * 0.55)], st);
            painter.line_segment([c, c + Vec2::new(s * 0.4, 0.0)], st);
        }
    }
}

/// A compact stat tile: icon, big value, small label underneath.
pub fn stat_tile(ui: &mut egui::Ui, icons: &IconTextures, ic: Icon, color: Color32, value: &str, label: &str, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 44.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 8.0, BG.gamma_multiply(0.9));
    // Glowing icons in the stat's colour (client icons are greyscale with their glow, tinted here).
    let c = Pos2::new(rect.left() + 21.0, rect.center().y);
    match icons.get(&ic) {
        Some(tex) => {
            p.image(tex.id(), Rect::from_center_size(c, Vec2::splat(32.0)), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), color);
        }
        None => {
            // Soft glow: the glyph a few times slightly larger and faint, then the glyph itself.
            for (grow, a) in [(8.0, 0.10), (5.0, 0.16), (2.5, 0.24)] {
                icon(p, Rect::from_center_size(c, Vec2::splat(18.0 + grow)), ic, color.gamma_multiply(a));
            }
            icon(p, Rect::from_center_size(c, Vec2::splat(18.0)), ic, color);
        }
    }
    p.text(Pos2::new(rect.left() + 42.0, rect.top() + 14.0), egui::Align2::LEFT_CENTER, value, egui::FontId::proportional(16.0), TEXT);
    p.text(Pos2::new(rect.left() + 42.0, rect.top() + 32.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(11.0), MUTED);
}

pub fn roman(tier: u8) -> &'static str {
    ["", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI"].get(tier as usize).copied().unwrap_or("")
}
