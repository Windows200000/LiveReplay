//! 2D minimap renderer. Implements [`BattleView`] so the 3D view can share playback,
//! selection and the stats popups.

use std::collections::HashMap;

use egui::{Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use super::scene::{ShotView, Unit, Visibility};
use super::settings::{HpText, ViewSettings};
use crate::gamedata::dds::Image;
use crate::gamedata::{ArenaDef, GameplayDef, VehicleClass};

/// A map-event object state that exists at the current time (3D only).
/// A random event's state at the current time.
#[derive(Debug, Clone)]
pub struct EventView {
    /// Scenario name, e.g. `airshipCrash01`.
    pub name: String,
    /// Announced, not happened yet: show the danger zone.
    pub warning: bool,
    /// Happened: show the aftermath.
    pub done: bool,
    pub pos: Option<[f32; 2]>,
}

struct EventLayer {
    /// Lower-case file name, e.g. `mmap_crash01_airship_zone.dds`.
    name: String,
    zone: bool,
    /// World-space centre of what the layer marks.
    centre: Option<[f32; 2]>,
    image: Option<egui::ColorImage>,
    texture: Option<egui::TextureHandle>,
}

pub struct ScenarioView {
    pub prefab: String,
    pub pos: [f32; 3],
    pub yaw: f32,
}

pub struct ViewInput<'a> {
    pub units: &'a [Unit],
    pub scenario: &'a [ScenarioView],
    /// Random events at this moment (danger zone or aftermath).
    pub events: &'a [EventView],
    pub shots: &'a [ShotView],
    /// Bases with their capture state.
    pub captures: &'a [super::scene::CaptureView],
    /// 3D camera to mark on the minimap: (eye x, eye z, view yaw).
    pub camera: Option<[f32; 3]>,
    pub settings: &'a ViewSettings,
    /// Pinned unit (yellow ring).
    pub selected: Option<i32>,
    /// Unit hovered elsewhere, e.g. in the team lists (white ring).
    pub hovered: Option<i32>,
    pub recorder_team: u8,
}

pub struct ViewOutput {
    pub hovered: Option<i32>,
    pub clicked_unit: Option<i32>,
    pub clicked_empty: bool,
    /// World position (x, z) of a click on the map.
    pub clicked_world: Option<[f32; 2]>,
    /// Right-click on the map (not on a tank in 3D): where to open the map's menu.
    pub context_at: Option<Pos2>,
    /// Right-click on a tank (the 3D view's corner minimap follows it).
    pub follow_unit: Option<i32>,
    /// Screen position of each drawn unit, used to attach popups.
    pub anchors: HashMap<i32, Pos2>,
    pub rect: Rect,
}

/// Anything that can draw a battle snapshot and report hover/click per unit.
pub trait BattleView {
    /// Tank the camera follows (3D only).
    fn follow(&self) -> Option<i32> {
        None
    }
    fn set_follow(&mut self, _id: Option<i32>) {}
    /// Move the camera to look at a world position (3D only).
    fn look_at(&mut self, _x: f32, _z: f32) {}
    /// Camera for the minimap mark: (eye x, eye z, yaw) (3D only).
    fn camera_mark(&self) -> Option<[f32; 3]> {
        None
    }
    /// Camera for share links: (target x, target z, yaw, pitch, distance) (3D only).
    fn camera(&self) -> Option<[f32; 5]> {
        None
    }
    fn show(&mut self, ui: &mut egui::Ui, input: &ViewInput) -> ViewOutput;
}

// Marker and label colours sampled from the in-game minimap.
pub const ALLY: Color32 = Color32::from_rgb(128, 200, 36);
pub const ENEMY: Color32 = Color32::from_rgb(222, 50, 24);
const ALLY_LABEL: Color32 = Color32::from_rgb(170, 196, 146);
const ENEMY_LABEL: Color32 = Color32::from_rgb(214, 92, 76);
const DEAD: Color32 = Color32::from_rgb(78, 78, 78);
pub const HOVER: Color32 = Color32::from_rgb(255, 255, 255);
pub const SELECT: Color32 = Color32::from_rgb(255, 210, 60);

pub fn team_color(is_ally: bool) -> Color32 {
    if is_ally {
        ALLY
    } else {
        ENEMY
    }
}

pub struct Minimap2D {
    arena: ArenaDef,
    gameplay: GameplayDef,
    texture: Option<egui::TextureHandle>,
    image: Option<egui::ColorImage>,
    /// Random-event overlays (danger zones and aftermath).
    layers: Vec<EventLayer>,
}

/// Slightly richer colours than the raw texture (the game draws the minimap over the 3D scene).
fn grade(img: &Image) -> egui::ColorImage {
    let mut px = img.rgba.clone();
    for p in px.chunks_exact_mut(4) {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        let l = 0.299 * r + 0.587 * g + 0.114 * b;
        for c in p.iter_mut().take(3) {
            let v = *c as f32;
            let sat = l + (v - l) * 1.3;
            let con = (sat - 110.0) * 1.12 + 110.0;
            *c = con.clamp(0.0, 255.0) as u8;
        }
        p[3] = 255;
    }
    egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &px)
}

impl Minimap2D {
    pub fn new(arena: ArenaDef, gameplay: GameplayDef, image: Option<Image>) -> Self {
        Self { arena, gameplay, texture: None, image: image.as_ref().map(grade), layers: Vec::new() }
    }

    /// Add random-event aftermath overlays (same size/placement as the minimap).
    pub fn with_event_layers(mut self, layers: Vec<(String, Image)>) -> Self {
        let [x0, z0] = self.arena.bbox_min;
        let [x1, z1] = self.arena.bbox_max;
        for (path, img) in layers {
            let name = path.rsplit('/').next().unwrap_or(&path).to_lowercase();
            // Centre of the marked (opaque) pixels, in world coordinates.
            let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
            for y in (0..img.height).step_by(2) {
                for x in (0..img.width).step_by(2) {
                    if img.rgba[(y * img.width + x) * 4 + 3] > 40 {
                        sx += x as f64;
                        sy += y as f64;
                        n += 1.0;
                    }
                }
            }
            let centre = (n > 0.0).then(|| {
                let (u, v) = ((sx / n) as f32 / img.width as f32, (sy / n) as f32 / img.height as f32);
                [x0 + u * (x1 - x0), z1 - v * (z1 - z0)]
            });
            self.layers.push(EventLayer {
                zone: name.contains("zone"),
                name,
                centre,
                image: Some(egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba)),
                texture: None,
            });
        }
        self
    }

    /// Layers belonging to an event (by name), the one nearest its position if there are variants.
    fn event_layers(&self, ev: &EventView, zone: bool) -> Vec<usize> {
        let full: String = ev.name.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_lowercase();
        let first: String = ev.name.chars().take_while(|c| c.is_ascii_lowercase()).collect();
        let key = if self.layers.iter().any(|l| l.name.contains(&full)) { full } else { first };
        if key.is_empty() {
            return Vec::new();
        }
        let cand: Vec<usize> = (0..self.layers.len()).filter(|&i| self.layers[i].zone == zone && self.layers[i].name.contains(&key)).collect();
        match ev.pos {
            Some(p) if cand.len() > 1 => {
                let d = |i: usize| self.layers[i].centre.map(|c| (c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2)).unwrap_or(f32::MAX);
                cand.iter().copied().min_by(|&a, &b| d(a).total_cmp(&d(b))).into_iter().collect()
            }
            _ => cand,
        }
    }

    fn to_screen(&self, rect: Rect, x: f32, z: f32) -> Pos2 {
        let [x0, z0] = self.arena.bbox_min;
        let [x1, z1] = self.arena.bbox_max;
        Pos2::new(
            rect.left() + (x - x0) / (x1 - x0) * rect.width(),
            rect.top() + (z1 - z) / (z1 - z0) * rect.height(),
        )
    }
}

/// In-game class markers, recreated as vectors from the client's icons:
/// LT = diamond, MT/HT = the same diamond cut into 2/3 diagonal stripes,
/// TD = wide downward triangle, SPG = square. `r` is the diamond's half-diagonal.
pub fn class_icon(painter: &egui::Painter, c: Pos2, r: f32, class: VehicleClass, fill: Color32) {
    let none = Stroke::NONE;
    match class {
        VehicleClass::TankDestroyer => {
            painter.add(Shape::convex_polygon(
                vec![c + Vec2::new(-1.15 * r, -0.85 * r), c + Vec2::new(1.15 * r, -0.85 * r), c + Vec2::new(0.0, 0.95 * r)],
                fill,
                none,
            ));
        }
        VehicleClass::Artillery => {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(1.5 * r)), 0.0, fill);
        }
        VehicleClass::Medium | VehicleClass::Heavy => {
            // Rotated frame: U runs "/" (up-right), V runs "\" (down-right). The diamond is |u|,|v| <= a.
            let a = r / std::f32::consts::SQRT_2;
            let u = Vec2::new(1.0, -1.0) / std::f32::consts::SQRT_2;
            let v = Vec2::new(1.0, 1.0) / std::f32::consts::SQRT_2;
            let n = if class == VehicleClass::Heavy { 3 } else { 2 };
            let gap = r * 0.16;
            let band = (2.0 * a - gap * (n - 1) as f32) / n as f32;
            for i in 0..n {
                let v0 = -a + i as f32 * (band + gap);
                let v1 = v0 + band;
                painter.add(Shape::convex_polygon(
                    vec![c + u * -a + v * v0, c + u * a + v * v0, c + u * a + v * v1, c + u * -a + v * v1],
                    fill,
                    none,
                ));
            }
        }
        _ => {
            painter.add(Shape::convex_polygon(
                vec![c + Vec2::new(0.0, -r), c + Vec2::new(r, 0.0), c + Vec2::new(0.0, r), c + Vec2::new(-r, 0.0)],
                fill,
                none,
            ));
        }
    }
}

/// Text with a dark outline, like the game's minimap labels.
pub fn outlined_text(painter: &egui::Painter, pos: Pos2, align: egui::Align2, text: &str, font: FontId, color: Color32) {
    let shadow = Color32::from_black_alpha((color.a() as f32 * 0.85) as u8);
    for d in [Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(0.0, -1.0), Vec2::new(1.0, 1.0)] {
        painter.text(pos + d, align, text, font.clone(), shadow);
    }
    painter.text(pos, align, text, font, color);
}

fn mix(a: Color32, b: Color32, k: f32) -> Color32 {
    let k = k.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k) as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

impl BattleView for Minimap2D {
    fn show(&mut self, ui: &mut egui::Ui, input: &ViewInput) -> ViewOutput {
        let st = input.settings;
        let avail = ui.available_size();
        // Grid labels only when there's room (not in the small corner minimap).
        let show_grid_labels = st.show_grid && avail.x.min(avail.y) > 420.0;
        let margin = if show_grid_labels { 16.0 } else { 0.0 };
        let side = (avail.x.min(avail.y) - margin).max(60.0);
        let (outer, response) = ui.allocate_exact_size(Vec2::splat(side + margin), Sense::click_and_drag());
        let rect = Rect::from_min_size(outer.min + Vec2::splat(margin), Vec2::splat(side));
        let painter = ui.painter_at(outer);
        let map_painter = painter.with_clip_rect(rect);

        if self.texture.is_none() {
            if let Some(img) = self.image.take() {
                self.texture = Some(ui.ctx().load_texture(format!("minimap-{}", self.arena.map), img, egui::TextureOptions::LINEAR));
            }
        }
        let map = self.arena.map.clone();
        for (i, l) in self.layers.iter_mut().enumerate() {
            if let Some(img) = l.image.take() {
                l.texture = Some(ui.ctx().load_texture(format!("minimap-layer-{map}-{i}"), img, egui::TextureOptions::LINEAR));
            }
        }
        painter.rect_filled(rect, 0.0, Color32::from_rgb(20, 22, 18));
        if let Some(tex) = &self.texture {
            let b = (st.map_brightness.clamp(0.2, 1.5) * 255.0).min(255.0) as u8;
            painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::from_gray(b));
            for ev in input.events {
                let mut show = Vec::new();
                if ev.done {
                    show.extend(self.event_layers(ev, false));
                }
                if ev.warning {
                    show.extend(self.event_layers(ev, true));
                }
                for i in show {
                    if let Some(tex) = &self.layers[i].texture {
                        painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::from_gray(b));
                    }
                }
            }
        }

        // Grid: rows A-K (no I) top to bottom, columns 1-0 left to right.
        if st.show_grid {
            let grid = Stroke::new(1.0_f32, Color32::from_white_alpha(14));
            let label_font = FontId::proportional(11.0);
            let rows = ["A", "B", "C", "D", "E", "F", "G", "H", "J", "K"];
            for i in 0..=10 {
                let f = i as f32 / 10.0;
                let x = rect.left() + f * rect.width();
                let y = rect.top() + f * rect.height();
                painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], grid);
                painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], grid);
                if i < 10 && show_grid_labels {
                    let cell = rect.width() / 10.0;
                    painter.text(Pos2::new(x + cell / 2.0, outer.top() + margin / 2.0), egui::Align2::CENTER_CENTER, ((i + 1) % 10).to_string(), label_font.clone(), Color32::from_gray(170));
                    painter.text(Pos2::new(outer.left() + margin / 2.0, y + cell / 2.0), egui::Align2::CENTER_CENTER, rows[i], label_font.clone(), Color32::from_gray(170));
                }
            }
        }

        // Bases / spawns / control points for this game mode, styled like the in-game minimap.
        let k = side / 600.0; // the in-game minimap is ~600 px at default size
        if st.show_bases {
            // Capture circles: white; tinted with the capturing team's colour while being capped.
            for c in input.captures {
                let p = self.to_screen(rect, c.pos[0], c.pos[1]);
                let r = (self.to_screen(rect, c.pos[0] + c.radius, c.pos[1]).x - p.x).abs();
                map_painter.circle_stroke(p, r, Stroke::new(1.6_f32 * k, Color32::from_white_alpha(170)));
                if let Some(team) = c.capturing_team {
                    let col = if c.blocked { Color32::from_gray(150) } else { team_color(team == input.recorder_team) };
                    map_painter.circle_filled(p, r, col.gamma_multiply(0.18));
                    // progress arc
                    let n = (c.points as usize * 64 / 100).max(1);
                    let pts: Vec<Pos2> = (0..=n).map(|i| {
                        let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 64.0 * std::f32::consts::TAU;
                        p + Vec2::new(a.cos(), a.sin()) * r
                    }).collect();
                    map_painter.add(Shape::line(pts, Stroke::new(3.0_f32 * k, col)));
                }
            }
            let own = (input.recorder_team as usize).saturating_sub(1).min(1);
            let flag = |p: Pos2, flag_color: Color32, rim: Color32| {
                let r = 16.0 * k;
                map_painter.circle(p, r, Color32::from_rgba_unmultiplied(40, 40, 40, 150), Stroke::new(2.2_f32 * k, rim));
                let pole = Stroke::new(1.8_f32 * k, Color32::from_gray(235));
                map_painter.line_segment([p + Vec2::new(-5.0, 8.0) * k, p + Vec2::new(-5.0, -8.0) * k], pole);
                let a = p + Vec2::new(-5.0, -8.0) * k;
                let b = p + Vec2::new(1.5, -6.5) * k;
                let c = p + Vec2::new(8.0, -8.0) * k;
                let d = p + Vec2::new(8.0, -1.0) * k;
                let e = p + Vec2::new(1.5, 0.5) * k;
                let f = p + Vec2::new(-5.0, -1.0) * k;
                map_painter.add(Shape::convex_polygon(vec![a, b, e, f], flag_color, Stroke::NONE));
                map_painter.add(Shape::convex_polygon(vec![b, c, d, e], flag_color, Stroke::NONE));
            };
            for (team, bases) in self.gameplay.bases.iter().enumerate() {
                let color = team_color(team == own);
                for b in bases {
                    flag(self.to_screen(rect, b[0], b[1]), color, color);
                }
            }
            for (team, spawns) in self.gameplay.spawns.iter().enumerate() {
                let color = team_color(team == own);
                for (n, sp) in spawns.iter().enumerate() {
                    let p = self.to_screen(rect, sp[0], sp[1]);
                    let r = 16.0 * k;
                    let pts = vec![p + Vec2::new(0.0, -r), p + Vec2::new(r, 0.0), p + Vec2::new(0.0, r), p + Vec2::new(-r, 0.0)];
                    map_painter.add(Shape::convex_polygon(pts.clone(), color.gamma_multiply(0.22), Stroke::NONE));
                    map_painter.add(Shape::closed_line(pts, Stroke::new(2.2_f32 * k, color)));
                    let font = FontId::proportional(15.0 * k);
                    map_painter.text(p + Vec2::new(1.0, 1.0), egui::Align2::CENTER_CENTER, (n + 1).to_string(), font.clone(), Color32::from_black_alpha(200));
                    map_painter.text(p, egui::Align2::CENTER_CENTER, (n + 1).to_string(), font, color);
                }
            }
            for cp in &self.gameplay.control_points {
                flag(self.to_screen(rect, cp[0], cp[1]), Color32::from_gray(235), Color32::from_gray(205));
            }
        }

        // 3D camera: translucent white dot with a view cone.
        if let Some([cx, cz, yaw]) = input.camera {
            let p = self.to_screen(rect, cx, cz);
            let len = side * 0.16;
            let half = 25f32.to_radians();
            // Screen: x = east, y = south; yaw 0 = north (+z).
            let dir = |a: f32| Vec2::new(a.sin(), -a.cos()) * len;
            let mut cone = vec![p];
            for i in 0..=12 {
                cone.push(p + dir(yaw - half + 2.0 * half * i as f32 / 12.0));
            }
            map_painter.add(Shape::convex_polygon(cone, Color32::from_white_alpha(38), Stroke::new(1.0, Color32::from_white_alpha(90))));
            map_painter.circle_filled(p, (4.0 * k).max(3.0), Color32::from_white_alpha(170));
        }

        // Your 564 m draw-distance circle (the in-game yellow one).
        if st.range_circle {
            // One per uploader when several recordings are merged.
            for me in input.units.iter().filter(|u| (u.is_recorder || u.is_viewpoint) && u.alive) {
                let p = self.to_screen(rect, me.pos[0], me.pos[2]);
                let px_per_m = rect.width() / (self.arena.bbox_max[0] - self.arena.bbox_min[0]);
                map_painter.circle_stroke(p, 564.0 * px_per_m, Stroke::new(1.3_f32, Color32::from_rgba_unmultiplied(230, 215, 70, 200)));
            }
        }

        // Shots: a line from the muzzle to the shell, fading after it lands.
        if st.show_shots {
            for s in input.shots {
                // Top-down the arc is a straight line: muzzle -> shell.
                let path = s.path(0.0);
                let (Some(first), Some(last)) = (path.first(), path.last()) else { continue };
                let a = self.to_screen(rect, first[0], first[2]);
                let b = self.to_screen(rect, last[0], last[2]);
                let base = if s.is_ally { Color32::from_rgb(255, 236, 150) } else { Color32::from_rgb(255, 170, 120) };
                map_painter.line_segment([a, b], Stroke::new(2.2_f32, base.gamma_multiply(s.alpha)));
                map_painter.circle_filled(b, 2.6, base.gamma_multiply(s.alpha));
            }
        }

        // Units
        let icon_r = (side * 0.0135).max(5.5) * st.marker_scale; // diamond half-diagonal
        let mut out = ViewOutput { hovered: None, clicked_unit: None, clicked_empty: false, clicked_world: None, context_at: None, follow_unit: None, anchors: HashMap::new(), rect };
        let pointer = response.hover_pos();
        let mut best: Option<(f32, i32)> = None;
        let label_font = FontId::proportional((side * 0.0145).clamp(10.0, 18.0) * st.marker_scale.sqrt());
        let small = side < 300.0;

        // Hit-test first, then draw, so the hovered unit is known while drawing.
        let points: Vec<Pos2> = input.units.iter().map(|u| self.to_screen(rect, u.pos[0], u.pos[2])).collect();
        for (u, &p) in input.units.iter().zip(&points) {
            out.anchors.insert(u.id, p);
            if let Some(ptr) = pointer {
                let d = ptr.distance(p);
                if d <= icon_r + 7.0 && best.map(|b| d < b.0).unwrap_or(true) {
                    best = Some((d, u.id));
                }
            }
        }
        let hovered_here = best.map(|b| b.1);
        for (u, &p) in input.units.iter().zip(&points) {
            let highlight = if input.selected == Some(u.id) {
                Highlight::Selected
            } else if input.hovered == Some(u.id) || hovered_here == Some(u.id) {
                Highlight::Hovered
            } else {
                Highlight::None
            };
            draw_marker(&map_painter, p, u, st, icon_r, &label_font, highlight, !small);
        }

        out.hovered = best.map(|b| b.1);
        // Click or drag: the world position under the pointer (moves the 3D camera).
        if response.clicked() || response.dragged_by(egui::PointerButton::Primary) {
            if let Some(pt) = response.interact_pointer_pos() {
                let [x0, z0] = self.arena.bbox_min;
                let [x1, z1] = self.arena.bbox_max;
                let u = ((pt.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let v = ((pt.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                out.clicked_world = Some([x0 + u * (x1 - x0), z1 - v * (z1 - z0)]);
            }
        }
        if response.clicked() {
            match out.hovered {
                Some(id) => out.clicked_unit = Some(id),
                None => out.clicked_empty = true,
            }
        }
        if response.secondary_clicked() {
            out.context_at = response.interact_pointer_pos();
            out.follow_unit = out.hovered;
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Highlight {
    None,
    Hovered,
    Selected,
}

/// One unit marker with everything the display settings ask for: selection/hover ring,
/// HP ring, class icon (with hit flash / despotted fade / dead grey), HP text and damage
/// number above, name labels to the right. Shared by the 2D minimap and the 3D view.
pub fn draw_marker(
    painter: &egui::Painter,
    p: Pos2,
    u: &Unit,
    st: &ViewSettings,
    icon_r: f32,
    label_font: &FontId,
    highlight: Highlight,
    with_text: bool,
) {
    let last_known = matches!(u.visibility, Visibility::LastKnown { .. });
    let fade = if last_known { 0.42 } else { 1.0 };
    let base = team_color(u.is_ally);
    let mut color = if !u.alive { DEAD } else { base.gamma_multiply(fade) };
    if u.flash > 0.0 {
        color = mix(color, Color32::WHITE, u.flash);
    }
    match highlight {
        Highlight::Selected => {
            painter.circle_stroke(p, icon_r + 8.0, Stroke::new(2.0_f32, SELECT));
        }
        Highlight::Hovered => {
            painter.circle_stroke(p, icon_r + 8.0, Stroke::new(1.6_f32, HOVER));
        }
        Highlight::None => {}
    }
    // Own tank: extra white ring so it's easy to find.
    if u.is_recorder && u.alive {
        painter.circle_stroke(p, icon_r + 11.0, Stroke::new(1.6_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 220)));
    }
    if st.hp_ring && u.alive {
        if let Some(hp) = u.hp {
            let base = base.gamma_multiply(fade);
            let r = icon_r + 4.0;
            painter.circle_stroke(p, r, Stroke::new(2.5_f32, Color32::from_black_alpha(150)));
            let frac = if u.max_hp > 0 { (hp as f32 / u.max_hp as f32).clamp(0.0, 1.0) } else { 0.0 };
            if frac > 0.0 {
                let n = (48.0 * frac).ceil().max(2.0) as usize;
                let pts: Vec<Pos2> = (0..=n)
                    .map(|i| {
                        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * frac * i as f32 / n as f32;
                        p + Vec2::new(a.cos(), a.sin()) * r
                    })
                    .collect();
                painter.add(Shape::line(pts, Stroke::new(2.5_f32, base)));
            }
        }
    }
    class_icon(painter, p, icon_r, u.class, color);
    if !with_text {
        return;
    }
    let ring_top = if st.hp_ring { icon_r + 7.0 } else { icon_r + 3.0 };
    let mut lift = ring_top;
    if u.alive {
        if let Some(hp) = u.hp {
            let text = match st.hp_text {
                HpText::Off => None,
                HpText::Current => Some(hp.to_string()),
                HpText::Max => Some(u.max_hp.to_string()),
                HpText::CurrentOfMax => Some(format!("{hp}/{}", u.max_hp)),
            };
            if let Some(text) = text {
                let hp_font = FontId::proportional(label_font.size * 0.85);
                outlined_text(painter, p - Vec2::new(0.0, lift), egui::Align2::CENTER_BOTTOM, &text, hp_font.clone(), Color32::from_gray(235).gamma_multiply(fade));
                lift += hp_font.size + 1.0;
            }
        }
    }
    // Damage taken: white "-123", stacking while hits keep coming, then fading.
    if let Some((dmg, a)) = u.damage_popup {
        outlined_text(painter, p - Vec2::new(0.0, lift), egui::Align2::CENTER_BOTTOM, &format!("-{dmg}"), FontId::proportional(label_font.size * 1.05), Color32::WHITE.gamma_multiply(a));
    }
    let mut lines: Vec<&str> = Vec::new();
    if st.label_player {
        lines.push(&u.player);
    }
    if st.label_tank {
        lines.push(&u.tank);
    }
    let text_color = if !u.alive { Color32::from_gray(120) } else { (if u.is_ally { ALLY_LABEL } else { ENEMY_LABEL }).gamma_multiply(fade) };
    let line_h = label_font.size + 1.0;
    let x = p.x + icon_r + 7.0;
    let y0 = p.y - (lines.len() as f32 - 1.0) * line_h / 2.0;
    for (i, l) in lines.iter().enumerate() {
        outlined_text(painter, Pos2::new(x, y0 + i as f32 * line_h), egui::Align2::LEFT_CENTER, l, label_font.clone(), text_color);
    }
}
