//! Replay player: playback controls, team panels, hover/pinned stats popups.
//! Rendering of the battlefield itself is delegated to a [`BattleView`]. Where the data comes
//! from (local game files on the desktop, the server on the web) is behind [`Assets`].

use std::sync::{mpsc, Arc};

use egui::{Color32, RichText};

use super::minimap2d::{team_color, BattleView, ViewInput};
use super::scene::{self, PlayerStats};
use super::settings::{HpText, ViewerSettings};
use super::view3d::{Scene3D, View3D};
use crate::replay::battle::{Battle, Period};

/// Source of the 3D data for a battle. `request_3d` must deliver its result on `tx`
/// (from a thread on the desktop, from an async fetch on the web).
pub trait Assets: Send + Sync {
    fn request_3d(&self, req: Request3D, tx: mpsc::Sender<Result<Scene3D, String>>);
    /// A stat icon image (from the game client), if available.
    fn icon(&self, _icon: super::theme::Icon) -> Option<crate::gamedata::dds::Image> {
        None
    }
}

/// What a 3D scene needs: map + game mode (objects differ per mode), tanks, map-event prefabs.
#[derive(Clone)]
pub struct Request3D {
    pub map: String,
    pub gameplay_id: i32,
    pub vehicle_types: Vec<String>,
    pub prefabs: Vec<String>,
}

pub struct Loaded {
    battle: Battle,
    view: Box<dyn BattleView + Send>,
    /// 3D view: only exists while the 3D toggle is on.
    view3d: Option<Box<dyn BattleView + Send>>,
    rx3d: Option<mpsc::Receiver<Result<Scene3D, String>>>,
    status3d: Option<String>,
}

impl Loaded {
    pub fn new(battle: Battle, view: Box<dyn BattleView + Send>) -> Self {
        Self { battle, view, view3d: None, rx3d: None, status3d: None }
    }
}

pub enum Msg {
    /// Fresh battle (new map): replaces everything.
    Loaded(Loaded, Arc<dyn Assets>),
    /// Same battle, more packets (live mode).
    Update(Battle),
    /// Live battle ended (file renamed by the client / upload finished).
    LiveEnded,
    /// The live battle is now a complete replay (web): normal replay controls from here on.
    Finished,
    Error(String),
    #[cfg(not(target_arch = "wasm32"))]
    Engine(Arc<native::Engine>),
}

pub struct ReplayViewer {
    pub title: String,
    rx: mpsc::Receiver<Msg>,
    /// Dropping this tells a live watcher to stop.
    _alive: Arc<()>,
    loaded: Option<Loaded>,
    assets: Option<Arc<dyn Assets>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub engine: Option<Arc<native::Engine>>,
    error: Option<String>,
    t: f32,
    playing: bool,
    pinned: Option<i32>,
    settings: ViewerSettings,
    /// Live mode: `Some(true)` while following the newest moment.
    live_follow: Option<bool>,
    live_ended: bool,
    /// Open at this many seconds after battle start instead of the beginning.
    pub start_at: Option<f32>,
    /// Start playing as soon as the replay is loaded (`--play`).
    pub autoplay: bool,
    /// Test hook (`--pin`): open with your own tank's stats panel pinned.
    pub pin_own: bool,
    /// Unit hovered on the map last frame (highlighted in the team lists).
    map_hover: Option<i32>,
    /// Initial 3D camera target (world x, z).
    pub look_at: Option<[f32; 2]>,
    /// Test hook: camera orbit (yaw, pitch, distance) applied with `look_at`.
    pub orbit: Option<[f32; 3]>,
    /// Web address of this battle (`.../b/<id>`), if it is on the server: enables "Copy link".
    pub share_base: Option<String>,
    /// Map menu (right-click) open at this point.
    share_menu: Option<egui::Pos2>,
    /// Open with this tank selected / followed in 3D (share links).
    pub pin: Option<i32>,
    pub follow_at_start: Option<i32>,
    icons: super::theme::IconTextures,
}

/// In-game style "Base capture (37)" bar under the top bar, one per base being captured.
fn capture_bar(painter: &egui::Painter, top: egui::Rect, captures: &[scene::CaptureView], recorder_team: u8) {
    // Like the in-game bar: team-coloured progress, tank + number of cappers on the left,
    // "Base capture (points)" in the middle, time left on the right.
    let mut y = top.bottom() + 4.0;
    for c in captures.iter().filter(|c| c.capturing_team.is_some() && (c.points > 0 || c.invaders > 0)) {
        let ours = c.capturing_team == Some(recorder_team);
        // Blocked: frozen, greyed out, like the game.
        let col = if c.blocked { Color32::from_gray(120) } else { super::minimap2d::team_color(ours) };
        let w = (top.width() * 0.42).clamp(260.0, 440.0);
        let r = egui::Rect::from_min_size(egui::pos2(top.center().x - w / 2.0, y), egui::vec2(w, 26.0));
        painter.rect_filled(r, 3.0, Color32::from_black_alpha(150));
        painter.rect_filled(r, 3.0, col.gamma_multiply(0.28));
        let mut f = r;
        f.set_width(r.width() * c.points as f32 / 100.0);
        painter.rect_filled(f, 3.0, col.gamma_multiply(0.85));
        // left: tank silhouette + count
        let tc = egui::pos2(r.left() + 16.0, r.center().y);
        let body = egui::Rect::from_center_size(tc + egui::vec2(0.0, 2.0), egui::vec2(16.0, 6.0));
        painter.rect_filled(body, 2.0, Color32::WHITE);
        painter.rect_filled(egui::Rect::from_center_size(tc + egui::vec2(-1.0, -3.0), egui::vec2(8.0, 4.0)), 1.5, Color32::WHITE);
        painter.line_segment([tc + egui::vec2(2.0, -3.0), tc + egui::vec2(10.0, -4.0)], egui::Stroke::new(1.5, Color32::WHITE));
        let font = egui::FontId::proportional(13.5);
        painter.text(egui::pos2(r.left() + 30.0, r.center().y), egui::Align2::LEFT_CENTER, c.invaders.to_string(), font.clone(), Color32::WHITE);
        // middle
        let label = if c.blocked { format!("Capture blocked ({})", c.points) } else { format!("Base capture ({})", c.points) };
        painter.text(r.center(), egui::Align2::CENTER_CENTER, label, font.clone(), Color32::WHITE);
        if c.blocked && c.blockers > 0 {
            // defenders inside, in their team colour
            let dcol = super::minimap2d::team_color(!ours);
            painter.text(egui::pos2(r.left() + 48.0, r.center().y), egui::Align2::LEFT_CENTER, format!("vs {}", c.blockers), font.clone(), dcol);
        }
        // right: clock + time left
        if c.time_left > 0 && !c.blocked {
            let cc = egui::pos2(r.right() - 58.0, r.center().y);
            painter.circle_stroke(cc, 6.0, egui::Stroke::new(1.4, Color32::WHITE));
            painter.line_segment([cc, cc + egui::vec2(0.0, -4.0)], egui::Stroke::new(1.3, Color32::WHITE));
            painter.line_segment([cc, cc + egui::vec2(3.0, 0.0)], egui::Stroke::new(1.3, Color32::WHITE));
            painter.text(
                egui::pos2(r.right() - 8.0, r.center().y),
                egui::Align2::RIGHT_CENTER,
                format!("{:02}:{:02}", c.time_left / 60, c.time_left % 60),
                font,
                Color32::WHITE,
            );
        }
        y += 30.0;
    }
}

/// On/off chip: highlighted when on (no tick boxes).
fn chip(ui: &mut egui::Ui, value: &mut bool) -> bool {
    let r = ui.add_sized([64.0, 22.0], egui::Button::new(if *value { "On" } else { "Off" }).selected(*value));
    if r.clicked() {
        *value = !*value;
    }
    r.clicked()
}

/// The Display menu: one row per setting, a column each for the 2D map and the 3D view.
fn display_settings_ui(ui: &mut egui::Ui, s: &mut ViewerSettings) -> bool {
    let mut changed = false;
    let muted = Color32::from_gray(150);
    let section = |ui: &mut egui::Ui, name: &str| {
        ui.label(RichText::new(name).small().strong().color(muted));
        ui.end_row();
    };
    egui::Grid::new("display_settings").num_columns(3).spacing([10.0, 6.0]).min_col_width(64.0).show(ui, |ui| {
        ui.label("");
        ui.label(RichText::new("2D").strong());
        ui.label(RichText::new("3D").strong());
        ui.end_row();

        type Flag = fn(&mut super::settings::ViewSettings) -> &mut bool;
        let mut row = |ui: &mut egui::Ui, name: &str, get: Flag, in2d: bool, in3d: bool| {
            ui.label(name);
            for (vs, on) in [(&mut s.view2d, in2d), (&mut s.view3d, in3d)] {
                if on {
                    changed |= chip(ui, get(vs));
                } else {
                    ui.label(RichText::new("–").color(muted));
                }
            }
            ui.end_row();
        };
        section(ui, "LABELS");
        row(ui, "Player name", |v| &mut v.label_player, true, true);
        row(ui, "Tank name", |v| &mut v.label_tank, true, true);
        section(ui, "HEALTH");
        row(ui, "HP ring", |v| &mut v.hp_ring, true, true);
        section(ui, "MAP");
        row(ui, "Last known positions", |v| &mut v.show_last_known, true, true);
        row(ui, "View range (564 m)", |v| &mut v.range_circle, true, true);
        row(ui, "Shots", |v| &mut v.show_shots, true, true);
        row(ui, "Grid", |v| &mut v.show_grid, true, false);
        row(ui, "Bases / spawns", |v| &mut v.show_bases, true, false);
        row(ui, "Textures", |v| &mut v.textures, false, true);

        ui.label("HP text");
        for (i, vs) in [&mut s.view2d, &mut s.view3d].into_iter().enumerate() {
            let name = |m: HpText| match m {
                HpText::Off => "Off",
                HpText::Current => "Current",
                HpText::Max => "Max",
                HpText::CurrentOfMax => "Cur / max",
            };
            egui::ComboBox::from_id_source(("hp_text", i)).width(64.0).selected_text(name(vs.hp_text)).show_ui(ui, |ui| {
                for m in [HpText::Off, HpText::Current, HpText::Max, HpText::CurrentOfMax] {
                    changed |= ui.selectable_value(&mut vs.hp_text, m, name(m)).changed();
                }
            });
        }
        ui.end_row();

        section(ui, "SIZE / LIGHT");
        ui.spacing_mut().slider_width = 64.0;
        ui.label("Marker size");
        for vs in [&mut s.view2d, &mut s.view3d] {
            changed |= ui.add(egui::Slider::new(&mut vs.marker_scale, 0.5..=2.0).show_value(false)).changed();
        }
        ui.end_row();
        ui.label("Brightness");
        for vs in [&mut s.view2d, &mut s.view3d] {
            changed |= ui.add(egui::Slider::new(&mut vs.map_brightness, 0.4..=1.2).show_value(false)).changed();
        }
        ui.end_row();
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("UI scale");
        // Presets instead of a slider: a slider would rescale itself while being dragged.
        egui::ComboBox::from_id_source("ui_scale").width(70.0).selected_text(format!("{:.0}%", s.ui_scale * 100.0)).show_ui(ui, |ui| {
            for v in [0.8_f32, 0.9, 1.0, 1.1, 1.25, 1.4, 1.5, 1.75, 2.0] {
                changed |= ui.selectable_value(&mut s.ui_scale, v, format!("{:.0}%", v * 100.0)).changed();
            }
        });
    });
    changed
}

fn class_order(c: Option<crate::gamedata::VehicleClass>) -> u8 {
    use crate::gamedata::VehicleClass::*;
    match c {
        Some(Heavy) => 0,
        Some(Medium) => 1,
        Some(TankDestroyer) => 2,
        Some(Light) => 3,
        Some(Artillery) => 4,
        _ => 5,
    }
}

/// Design size of the top bar; everything scales with the width it gets.
const TOP_BAR_W: f32 = 900.0;
const TOP_BAR_H: f32 = 46.0;

/// Top-bar rectangle for a given available width (keeps the design aspect ratio).
pub fn top_bar_size(avail_w: f32) -> egui::Vec2 {
    let w = avail_w.min(1300.0);
    egui::vec2(w, w * TOP_BAR_H / TOP_BAR_W)
}

/// In-game style top bar, laid out like the game's: score boxes in the middle; per side a thin HP
/// bar with the team HP next to the score (HP difference at the far left), and below it the class
/// icons running outward from the score - alive first (by class), destroyed ones small and dim.
/// Scaled to the rectangle so every icon always fits.
pub fn top_bar(painter: &egui::Painter, rect: egui::Rect, b: &Battle, t: f32) {
    use super::minimap2d::{class_icon, ALLY, ENEMY};
    let s = rect.width() / TOP_BAR_W;
    let is_dead = |v: &crate::replay::battle::VehicleRec| v.death_time.map(|d| d <= t).unwrap_or(false);
    let team = |ally: bool| {
        let mut v: Vec<_> = b.vehicles.values().filter(|v| (v.team == b.recorder_team) == ally).collect();
        v.sort_by_key(|v| (is_dead(v), class_order(v.class), std::cmp::Reverse(v.tier)));
        v
    };
    let (allies, enemies) = (team(true), team(false));
    let hp = |v: &&crate::replay::battle::VehicleRec| -> (i32, i32) {
        let cur = if is_dead(v) { 0 } else { scene::health_at(v, t).unwrap_or(v.max_health) };
        (cur.max(0), v.max_health)
    };
    let sum = |list: &[&crate::replay::battle::VehicleRec]| list.iter().map(hp).fold((0, 0), |a, h| (a.0 + h.0, a.1 + h.1));
    let (ahp, amax) = sum(&allies);
    let (ehp, emax) = sum(&enemies);
    let dead = |list: &[&crate::replay::battle::VehicleRec]| list.iter().filter(|v| is_dead(v)).count();
    let (ally_score, enemy_score) = (dead(&enemies), dead(&allies));

    painter.rect_filled(rect, 6.0 * s, Color32::from_black_alpha(150));
    let c = rect.center().x;
    let top = rect.top();
    let num_font = egui::FontId::proportional((13.0 * s).max(8.0));
    let score_font = egui::FontId::proportional((24.0 * s).max(11.0));

    // Score boxes
    let box_w = 42.0 * s;
    let box_h = rect.height() - 8.0 * s;
    for (ally, score) in [(true, ally_score), (false, enemy_score)] {
        let x0 = if ally { c - 2.0 * s - box_w } else { c + 2.0 * s };
        let r = egui::Rect::from_min_size(egui::pos2(x0, top + 4.0 * s), egui::vec2(box_w, box_h));
        let col = if ally { ALLY } else { ENEMY };
        painter.rect_filled(r, 4.0 * s, col.gamma_multiply(0.45));
        painter.text(r.center(), egui::Align2::CENTER_CENTER, score.to_string(), score_font.clone(), Color32::WHITE);
    }

    let inner = box_w + 10.0 * s; // distance from the centre to where each side starts
    let side_w = rect.width() / 2.0 - inner - 8.0 * s;
    let row1 = top + 13.0 * s;
    let row2 = top + 32.0 * s;
    for (ally, cur, max, list) in [(true, ahp, amax, &allies), (false, ehp, emax, &enemies)] {
        let dir = if ally { -1.0 } else { 1.0 }; // outward
        let start = c + dir * inner;
        let col = if ally { ALLY } else { ENEMY };
        // HP number at the inner end, then the bar outward from it.
        let num = format_thousands(cur);
        let align = if ally { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER };
        let galley = painter.layout_no_wrap(num.clone(), num_font.clone(), Color32::WHITE);
        painter.text(egui::pos2(start, row1), align, num, num_font.clone(), Color32::from_gray(235));
        let bar_start = start + dir * (galley.size().x + 8.0 * s);
        let bar_len = (side_w * 0.42).max(10.0);
        let bar_h = (5.0 * s).max(2.0);
        let track = egui::Rect::from_two_pos(egui::pos2(bar_start, row1 - bar_h / 2.0), egui::pos2(bar_start + dir * bar_len, row1 + bar_h / 2.0));
        painter.rect_filled(track, 1.5 * s, Color32::from_white_alpha(25));
        let frac = if max > 0 { cur as f32 / max as f32 } else { 0.0 };
        let fill = egui::Rect::from_two_pos(egui::pos2(bar_start, track.top()), egui::pos2(bar_start + dir * bar_len * frac, track.bottom()));
        painter.rect_filled(fill, 1.5 * s, col);

        // Class icons below, from the score outward; spacing shrinks so all of them fit.
        let n = list.len().max(1) as f32;
        let step = (side_w / n).min(19.0 * s);
        let r = step * 0.34;
        for (i, v) in list.iter().enumerate() {
            let x = start + dir * step * (i as f32 + 0.5);
            let class = v.class.unwrap_or(crate::gamedata::VehicleClass::Unknown);
            if is_dead(v) {
                class_icon(painter, egui::pos2(x, row2), r * 0.75, class, Color32::from_gray(90));
            } else {
                class_icon(painter, egui::pos2(x, row2), r, class, col);
            }
        }
    }
    // HP difference at the far left
    let diff = ahp - ehp;
    painter.text(
        egui::pos2(rect.left() + 8.0 * s, row1),
        egui::Align2::LEFT_CENTER,
        format!("{}{}", if diff > 0 { "+" } else { "" }, format_thousands(diff)),
        num_font,
        if diff >= 0 { ALLY } else { ENEMY },
    );
}

fn format_thousands(v: i32) -> String {
    let s = v.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if v < 0 {
        format!("-{out}")
    } else {
        out
    }
}

fn fmt_clock(secs: f32) -> String {
    let s = secs.max(0.0).ceil() as i32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Desktop: everything comes from the local World of Tanks installation.
#[cfg(not(target_arch = "wasm32"))]
pub mod native {
    use super::*;
    use crate::gamedata::cache3d::Cache3D;
    use crate::gamedata::GameData;
    use crate::replay::defs::Definitions;
    use crate::viewer::minimap2d::Minimap2D;

    pub struct Engine {
        pub game: GameData,
        pub defs: Definitions,
        /// Simplified 3D data cache (None if there's no cache directory).
        pub cache3d: Option<Cache3D>,
    }

    impl Engine {
        pub fn scene3d(&self, req: &Request3D) -> Result<Scene3D, String> {
            let map = req.map.as_str();
            let cache = self.cache3d.as_ref().ok_or("No cache directory available for 3D data")?;
            let arena = self.game.arena(map).ok_or_else(|| format!("Map '{map}' not found"))?;
            let terrain = cache.terrain(&self.game, map).ok_or_else(|| format!("Could not read terrain for '{map}'"))?;
            let minimap = self.game.minimap(&arena);
            let map_scene = cache.scene(&self.game, map, req.gameplay_id).unwrap_or_default();
            let mut tanks = std::collections::HashMap::new();
            for vt in &req.vehicle_types {
                if let Some(m) = cache.tank(&self.game, vt) {
                    tanks.insert(vt.clone(), m);
                }
            }
            let mut prefabs = std::collections::HashMap::new();
            for name in &req.prefabs {
                if let Some(m) = cache.prefab(&self.game, name) {
                    prefabs.insert(name.clone(), m);
                }
            }
            Ok(Scene3D { arena, terrain, minimap, map: map_scene, tanks, prefabs })
        }
    }

    /// Engine wrapper that builds 3D scenes on a background thread.
    pub struct ThreadAssets(pub Arc<Engine>);
    impl Assets for ThreadAssets {
        fn icon(&self, icon: super::super::theme::Icon) -> Option<crate::gamedata::dds::Image> {
            self.0.game.gui_icon(icon.game_path()?)
        }

        fn request_3d(&self, req: Request3D, tx: mpsc::Sender<Result<Scene3D, String>>) {
            let engine = self.0.clone();
            std::thread::spawn(move || {
                let _ = tx.send(engine.scene3d(&req));
            });
        }
    }

    /// The game data, loaded once and shared by every viewer and the uploader.
    pub fn get_engine(engine: Option<Arc<Engine>>, exe_hint: &str) -> Result<Arc<Engine>, String> {
        static SHARED: std::sync::Mutex<Option<Arc<Engine>>> = std::sync::Mutex::new(None);
        if let Some(e) = engine {
            return Ok(e);
        }
        let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = shared.as_ref() {
            return Ok(e.clone());
        }
        let dir = crate::gamedata::find_game_dir(exe_hint)
            .ok_or("World of Tanks installation not found. Set the WoT executable path in Settings.")?;
        let game = GameData::new(dir);
        let defs = Definitions::load(&game).map_err(|e| e.to_string())?;
        let cache3d = Cache3D::new(&game);
        let e = Arc::new(Engine { game, defs, cache3d });
        *shared = Some(e.clone());
        Ok(e)
    }

    pub fn build_view(engine: &Engine, battle: &Battle) -> Result<Box<dyn BattleView + Send>, String> {
        let arena = engine
            .game
            .arena(&battle.map)
            .ok_or_else(|| format!("Map definition for '{}' not found in game files", battle.map))?;
        let gameplay = arena.gameplay(&battle.gameplay);
        let image = engine.game.minimap(&arena);
        let layers = engine.game.minimap_event_layers(&arena);
        Ok(Box::new(Minimap2D::new(arena, gameplay, image).with_event_layers(layers)))
    }

    fn loaded_msg(engine: &Arc<Engine>, battle: Battle) -> Msg {
        match build_view(engine, &battle) {
            Ok(view) => Msg::Loaded(Loaded::new(battle, view), Arc::new(ThreadAssets(engine.clone()))),
            Err(e) => Msg::Error(e),
        }
    }

    impl ReplayViewer {
        /// Load a saved replay in the background.
        pub fn open(path: std::path::PathBuf, engine: Option<Arc<Engine>>, exe_hint: String) -> Self {
            let (tx, rx) = mpsc::channel();
            let title = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            std::thread::spawn(move || {
                let engine = match get_engine(engine, &exe_hint) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = tx.send(Msg::Error(e));
                        return;
                    }
                };
                let _ = tx.send(Msg::Engine(engine.clone()));
                let msg = match Battle::load(&path, &engine.game, &engine.defs) {
                    Ok(battle) => loaded_msg(&engine, battle),
                    Err(e) => Msg::Error(e.to_string()),
                };
                let _ = tx.send(msg);
            });
            Self::new(title, rx, Arc::new(()), false)
        }

        /// Follow the battle in progress: re-read `temp.wotreplay` (written by the running client) every second.
        pub fn open_live(replays_dir: std::path::PathBuf, engine: Option<Arc<Engine>>, exe_hint: String) -> Self {
            let (tx, rx) = mpsc::channel();
            let alive = Arc::new(());
            let watch = Arc::downgrade(&alive);
            let path = replays_dir.join("temp.wotreplay");
            std::thread::spawn(move || {
                let engine = match get_engine(engine, &exe_hint) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = tx.send(Msg::Error(e));
                        return;
                    }
                };
                let _ = tx.send(Msg::Engine(engine.clone()));
                let mut last_len = 0u64;
                let mut current: Option<(String, i32)> = None;
                let mut was_present = false;
                while watch.upgrade().is_some() {
                    match std::fs::metadata(&path) {
                        Ok(meta) if meta.len() != last_len => {
                            was_present = true;
                            let shrunk = meta.len() < last_len;
                            last_len = meta.len();
                            if let Ok(battle) = Battle::load(&path, &engine.game, &engine.defs) {
                                let key = (battle.map.clone(), battle.recorder_vehicle.unwrap_or(0));
                                let msg = if shrunk || current.as_ref() != Some(&key) {
                                    current = Some(key);
                                    loaded_msg(&engine, battle)
                                } else {
                                    Msg::Update(battle)
                                };
                                if tx.send(msg).is_err() {
                                    return;
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(_) => {
                            if was_present {
                                was_present = false;
                                last_len = 0;
                                current = None;
                                if tx.send(Msg::LiveEnded).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                }
            });
            Self::new("Live battle".into(), rx, alive, true)
        }
    }
}

impl ReplayViewer {
    fn new(title: String, rx: mpsc::Receiver<Msg>, alive: Arc<()>, live: bool) -> Self {
        Self {
            title,
            rx,
            _alive: alive,
            loaded: None,
            assets: None,
            #[cfg(not(target_arch = "wasm32"))]
            engine: None,
            error: None,
            t: 0.0,
            playing: false,
            pinned: None,
            settings: ViewerSettings::load(),
            live_follow: live.then_some(true),
            live_ended: false,
            start_at: None,
            autoplay: false,
            pin_own: false,
            map_hover: None,
            look_at: None,
            orbit: None,
            share_base: None,
            share_menu: None,
            pin: None,
            follow_at_start: None,
            icons: Default::default(),
        }
    }

    /// Viewer fed by an external producer (the web frontend). Keep `alive` to keep a poller running.
    pub fn remote(title: String, rx: mpsc::Receiver<Msg>, alive: Arc<()>, live: bool) -> Self {
        Self::new(title, rx, alive, live)
    }

    /// Current moment in seconds after battle start (once loaded).
    pub fn moment(&self) -> Option<f32> {
        self.loaded.as_ref().map(|l| self.t - l.battle.battle_start())
    }

    /// Link to this battle at the current moment, with the view (2D/3D), selected tank and 3D camera.
    pub fn share_link(&self) -> Option<String> {
        let base = self.share_base.as_ref()?;
        let mut q = vec![format!("t={:.1}", self.moment()?.max(0.0))];
        if self.settings.view_3d {
            q.push("v=3d".into());
        }
        if let Some(id) = self.pinned {
            q.push(format!("sel={id}"));
        }
        if let Some(v) = self.loaded.as_ref().and_then(|l| l.view3d.as_ref()).filter(|_| self.settings.view_3d) {
            if let Some([x, z, yaw, pitch, dist]) = v.camera() {
                q.push(format!("cam={x:.1},{z:.1},{yaw:.3},{pitch:.3},{dist:.0}"));
            }
            if let Some(id) = v.follow() {
                q.push(format!("follow={id}"));
            }
        }
        let sep = if base.contains('?') { '&' } else { '?' };
        Some(format!("{base}{sep}{}", q.join("&")))
    }

    /// Open as a share link says (`t`, `v`, `sel`, `cam`, `follow`; see [`Self::share_link`]).
    pub fn apply_link(&mut self, query: &str) {
        for (k, v) in query.trim_start_matches('?').split('&').filter_map(|kv| kv.split_once('=')) {
            let nums: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
            match (k, nums.as_slice()) {
                ("t", [t]) => self.start_at = Some(*t),
                ("v", _) => self.settings.view_3d = v == "3d",
                ("sel", [id]) => self.pin = Some(*id as i32),
                ("follow", [id]) => self.follow_at_start = Some(*id as i32),
                ("cam", [x, z, yaw, pitch, dist]) => {
                    self.look_at = Some([*x, *z]);
                    self.orbit = Some([*yaw, *pitch, *dist]);
                }
                _ => {}
            }
        }
    }

    /// Live mode (follow the newest moment) on or off before loading.
    pub fn set_live(&mut self, live: bool) {
        self.live_follow = live.then_some(true);
    }

    /// Switch the 3D view on/off for this session (not saved).
    pub fn set_3d(&mut self, on: bool) {
        self.settings.view_3d = on;
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Loaded(loaded, assets) => {
                    let start_at = self.start_at.take();
                    self.t = if let Some(at) = start_at {
                        (loaded.battle.battle_start() + at).clamp(0.0, loaded.battle.duration)
                    } else if self.live_follow.is_some() {
                        (loaded.battle.duration - 1.0).max(0.0)
                    } else {
                        // Start at the end of the pre-battle countdown.
                        (loaded.battle.battle_start() - 3.0).max(0.0)
                    };
                    if self.live_follow.is_some() {
                        self.live_follow = Some(true);
                    }
                    if self.icons.is_empty() {
                        for ic in super::theme::Icon::ALL {
                            if let Some(mut img) = assets.icon(ic) {
                                // Greyscale (brightest = white) so each stat can tint it; alpha/glow kept.
                                let lum = |p: &[u8]| 0.3 * p[0] as f32 + 0.59 * p[1] as f32 + 0.11 * p[2] as f32;
                                let max = img.rgba.chunks_exact(4).filter(|p| p[3] > 128).map(lum).fold(1.0f32, f32::max);
                                for px in img.rgba.chunks_exact_mut(4) {
                                    let v = (lum(px) / max * 255.0).min(255.0) as u8;
                                    px[0] = v;
                                    px[1] = v;
                                    px[2] = v;
                                }
                                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba);
                                self.icons.insert(ic, ctx.load_texture(format!("icon-{ic:?}"), ci, egui::TextureOptions::LINEAR));
                            }
                        }
                    }
                    self.loaded = Some(loaded);
                    self.assets = Some(assets);
                    // Opening at a specific moment starts paused.
                    self.playing = start_at.is_none() || self.autoplay;
                    self.pinned = if self.pin_own { self.loaded.as_ref().and_then(|l| l.battle.recorder_vehicle) } else { self.pin.take() };
                    self.live_ended = false;
                    self.error = None;
                }
                Msg::Update(battle) => {
                    if let Some(l) = self.loaded.as_mut() {
                        l.battle = battle;
                        if self.live_follow == Some(true) {
                            // Stay ~1s behind the newest packet so movement stays smooth.
                            let newest = l.battle.duration;
                            if (newest - self.t).abs() > 3.0 {
                                self.t = (newest - 1.0).max(0.0);
                            }
                            self.playing = true;
                        }
                    }
                }
                Msg::LiveEnded => self.live_ended = true,
                Msg::Finished => {
                    self.live_follow = None;
                    self.live_ended = false;
                }
                Msg::Error(e) => self.error = Some(e),
                #[cfg(not(target_arch = "wasm32"))]
                Msg::Engine(e) => self.engine = Some(e),
            }
        }
        if self.live_follow.is_some() {
            // Keep checking the watcher even when nothing moves.
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        if let Some(e) = &self.error {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.colored_label(Color32::LIGHT_RED, format!("Could not open replay: {e}"));
            });
            return;
        }
        let Some(loaded) = self.loaded.as_mut() else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    if self.live_follow.is_some() {
                        ui.label("Waiting for a battle... start one in World of Tanks (replay recording must be enabled).");
                    } else {
                        ui.spinner();
                    }
                });
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        };

        apply_ui_scale(ctx, self.settings.ui_scale);

        // Advance playback.
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.playing = !self.playing;
        }
        if self.playing {
            let speed = if self.live_follow == Some(true) { 1.0 } else { self.settings.speed };
            self.t += dt * speed;
            if self.t >= loaded.battle.duration {
                self.t = loaded.battle.duration;
                if self.live_follow.is_none() {
                    self.playing = false;
                }
            }
            ctx.request_repaint();
        }
        let battle = &loaded.battle;
        let t = self.t;
        let mut settings_changed = false;

        // Top bar: map, clock, playback controls, display settings.
        egui::TopBottomPanel::top("viewer_top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("{} — {}", battle.map_display, battle.gameplay));
                ui.separator();
                if ui.selectable_label(!self.settings.view_3d, "2D").clicked() && self.settings.view_3d {
                    self.settings.view_3d = false;
                    settings_changed = true;
                }
                if ui.selectable_label(self.settings.view_3d, "3D").clicked() && !self.settings.view_3d {
                    self.settings.view_3d = true;
                    settings_changed = true;
                }
                if self.settings.view_3d {
                    if ui.add(egui::Button::new("Minimap").selected(self.settings.corner_minimap)).clicked() {
                        self.settings.corner_minimap = !self.settings.corner_minimap;
                        settings_changed = true;
                    }
                }
                ui.separator();
                let clock = battle.clock(t);
                let text = match (clock.period, clock.remaining) {
                    (Period::Waiting, _) => "Waiting for players".to_string(),
                    (Period::Prebattle, Some(r)) => format!("Battle starts in {}", fmt_clock(r)),
                    (Period::Battle, Some(r)) => fmt_clock(r),
                    (Period::AfterBattle, _) => match battle.winner_team {
                        Some(w) if w == battle.recorder_team => "Victory".into(),
                        Some(0) | None => "Battle over".into(),
                        Some(_) => "Defeat".into(),
                    },
                    _ => String::new(),
                };
                ui.label(RichText::new(text).size(20.0).strong().color(Color32::from_rgb(240, 230, 200)));
                if let Some(follow) = self.live_follow {
                    ui.separator();
                    if self.live_ended {
                        ui.label(RichText::new("Battle ended - waiting for the next one").color(Color32::from_gray(170)));
                    } else if follow {
                        ui.label(RichText::new("LIVE").strong().color(Color32::from_rgb(255, 70, 60)));
                    } else if ui.button("Go live").clicked() {
                        self.live_follow = Some(true);
                        self.t = (battle.duration - 1.0).max(0.0);
                        self.playing = true;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button("Display ⚙", |ui| {
                        ui.set_width(360.0);
                        settings_changed |= display_settings_ui(ui, &mut self.settings);
                    });
                });
            });
        });

        egui::TopBottomPanel::bottom("viewer_bottom").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button(if self.playing { "⏸ Pause" } else { "▶ Play" }).clicked() {
                    if !self.playing && self.t >= battle.duration {
                        self.t = 0.0;
                    }
                    self.playing = !self.playing;
                }
                egui::ComboBox::from_id_source("speed")
                    .selected_text(format!("{}x", self.settings.speed))
                    .width(60.0)
                    .show_ui(ui, |ui| {
                        for s in [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0] {
                            settings_changed |= ui.selectable_value(&mut self.settings.speed, s, format!("{s}x")).changed();
                        }
                    });
                if ui.button("⏮ Battle start").clicked() {
                    self.t = battle.battle_start();
                    if self.live_follow.is_some() {
                        self.live_follow = Some(false);
                    }
                }
                let elapsed = (t - battle.battle_start()).max(0.0);
                ui.label(format!("{} / {}", fmt_clock(elapsed), fmt_clock(battle.duration - battle.battle_start())));
                ui.spacing_mut().slider_width = (ui.available_width() - 10.0).max(100.0);
                let scrub = ui.add(egui::Slider::new(&mut self.t, 0.0..=battle.duration).show_value(false));
                if scrub.changed() && self.live_follow.is_some() {
                    self.live_follow = Some(false);
                }
            });
            ui.add_space(4.0);
        });

        // Team panels (allies left, enemies right), like the in-game ear panels.
        let mut list_click: Option<i32> = None;
        let mut list_hover: Option<i32> = None;
        let map_hover = self.map_hover;
        for (side, allies) in [(egui::panel::Side::Left, true), (egui::panel::Side::Right, false)] {
            egui::SidePanel::new(side, if allies { "allies" } else { "enemies" })
                .resizable(true)
                .default_width(210.0)
                // Small windows: the lists give way to the map (long names get cut with "…").
                .max_width((ctx.screen_rect().width() * 0.25).max(120.0))
                .show(ctx, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    ui.label(RichText::new(if allies { "Allies" } else { "Enemies" }).strong().color(team_color(allies)));
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let mut rows: Vec<_> = battle.vehicles.values().filter(|v| (v.team == battle.recorder_team) == allies).collect();
                        // Like the in-game panels: alive first, then by class, then tier.
                        rows.sort_by_key(|v| (v.death_time.map(|d| d <= t).unwrap_or(false), class_order(v.class), std::cmp::Reverse(v.tier), v.name.to_lowercase()));
                        for v in rows {
                            let dead = v.death_time.map(|d| d <= t).unwrap_or(false);
                            let hp = scene::health_at(v, t);
                            let color = if dead { Color32::GRAY } else { team_color(allies) };
                            let selected = self.pinned == Some(v.id);
                            let label = format!(
                                "{} {}  {}",
                                v.class.map(|c| c.short()).unwrap_or("?"),
                                v.tank_short,
                                v.name
                            );
                            let r = ui.selectable_label(selected, RichText::new(label).color(color).size(12.0));
                            if r.clicked() {
                                list_click = Some(v.id);
                            }
                            if r.hovered() {
                                list_hover = Some(v.id);
                            }
                            // Highlight the row of the tank hovered on the map (and the hovered row itself).
                            if !selected && (map_hover == Some(v.id) || r.hovered()) {
                                ui.painter().rect_stroke(r.rect.expand(1.0), 3.0, egui::Stroke::new(1.5_f32, super::minimap2d::HOVER));
                            }
                            if v.max_health > 0 {
                                let frac = if dead { 0.0 } else { hp.map(|h| h as f32 / v.max_health as f32).unwrap_or(1.0).clamp(0.0, 1.0) };
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
                                ui.painter().rect_filled(rect, 1.0, Color32::from_black_alpha(120));
                                let mut fill = rect;
                                fill.set_width(rect.width() * frac);
                                ui.painter().rect_filled(fill, 1.0, if hp.is_some() { color } else { color.gamma_multiply(0.35) });
                            }
                        }
                    });
                });
        }
        if let Some(id) = list_click {
            self.pinned = if self.pinned == Some(id) { None } else { Some(id) };
        }

        let units = scene::units_at(battle, t, self.settings.active().show_last_known);
        // Each view filters shots by its own setting.
        let shots = scene::shots_at(battle, t);
        // 3D lifecycle: built in the background on first use, dropped entirely when switched off.
        if self.settings.view_3d {
            if let Some(rx) = &loaded.rx3d {
                if let Ok(r) = rx.try_recv() {
                    match r {
                        Ok(scene) => {
                            let mut v = View3D::new(scene);
                            if let Some([x, z]) = self.look_at {
                                v.set_target(x, z);
                                if let Some([yaw, pitch, dist]) = self.orbit {
                                    v.set_orbit(yaw, pitch, dist);
                                }
                            }
                            if let Some(id) = self.follow_at_start.take() {
                                v.set_follow(Some(id));
                            }
                            loaded.view3d = Some(Box::new(v));
                        }
                        Err(e) => loaded.status3d = Some(e),
                    }
                    loaded.rx3d = None;
                }
            }
            if loaded.view3d.is_none() && loaded.rx3d.is_none() && loaded.status3d.is_none() {
                if let Some(assets) = self.assets.clone() {
                    let (tx, rx) = mpsc::channel();
                    let mut types: Vec<String> = loaded.battle.vehicles.values().map(|v| v.vehicle_type.clone()).collect();
                    types.sort();
                    types.dedup();
                    let mut prefabs: Vec<String> = loaded.battle.scenario.iter().map(|s| s.prefab.clone()).collect();
                    prefabs.sort();
                    prefabs.dedup();
                    assets.request_3d(
                        Request3D { map: loaded.battle.map.clone(), gameplay_id: loaded.battle.gameplay_id, vehicle_types: types, prefabs },
                        tx,
                    );
                    loaded.rx3d = Some(rx);
                }
            }
            if loaded.rx3d.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        } else if loaded.view3d.is_some() || loaded.rx3d.is_some() || loaded.status3d.is_some() {
            loaded.view3d = None;
            loaded.rx3d = None;
            loaded.status3d = None;
        }
        let battle = &loaded.battle;

        let scenario: Vec<super::minimap2d::ScenarioView> = battle
            .scenario
            .iter()
            .filter(|s| s.created <= t && s.removed.map_or(true, |r| t < r))
            .map(|s| super::minimap2d::ScenarioView { prefab: s.prefab.clone(), pos: s.pos, yaw: s.yaw })
            .collect();
        let captures = scene::captures_at(battle, t);
        let events: Vec<super::minimap2d::EventView> = battle
            .random_events
            .iter()
            .filter(|e| e.warned <= t)
            .map(|e| {
                let done = e.start.is_some_and(|s| s <= t);
                super::minimap2d::EventView { name: e.name.clone(), warning: !done, done, pos: e.pos }
            })
            .collect();
        let input = ViewInput {
            units: &units,
            scenario: &scenario,
            events: &events,
            shots: &shots,
            settings: self.settings.active(),
            selected: self.pinned,
            hovered: list_hover,
            recorder_team: battle.recorder_team,
            captures: &captures,
            camera: None,
        };
        let mut out = None;
        let mut out_mini = None;
        let show_3d = self.settings.view_3d && loaded.view3d.is_some();
        let status_3d = if self.settings.view_3d && !show_3d {
            Some(loaded.status3d.clone().unwrap_or_else(|| "Preparing 3D view (first time per map/tank takes a moment)...".into()))
        } else {
            None
        };
        egui::CentralPanel::default().show(ctx, |ui| {
            if show_3d {
                let full = ui.max_rect();
                out = Some(loaded.view3d.as_mut().unwrap().show(ui, &input));
                let size = top_bar_size(full.width() - 16.0);
                let bar = egui::Rect::from_center_size(egui::pos2(full.center().x, full.top() + 6.0 + size.y / 2.0), size);
                top_bar(&ui.painter_at(full), bar, battle, t);
                capture_bar(&ui.painter_at(full), bar, &captures, battle.recorder_team);
                if self.settings.corner_minimap {
                    let size = (full.width().min(full.height()) * 0.32).max(160.0);
                    let r = egui::Rect::from_min_size(full.right_bottom() - egui::vec2(size + 8.0, size + 8.0), egui::vec2(size, size));
                    ui.painter().rect_filled(r.expand(2.0), 2.0, Color32::from_black_alpha(160));
                    let camera = loaded.view3d.as_ref().and_then(|v| v.camera_mark());
                    let input2d = ViewInput { settings: &self.settings.view2d, camera, ..input };
                    ui.allocate_ui_at_rect(r, |ui| {
                        out_mini = Some(loaded.view.show(ui, &input2d));
                    });
                }
            } else {
                if let Some(msg) = &status_3d {
                    ui.horizontal(|ui| {
                        if loaded.status3d.is_none() {
                            ui.spinner();
                        }
                        ui.label(msg);
                    });
                }
                let (bar, _) = ui.allocate_exact_size(top_bar_size(ui.available_width()), egui::Sense::hover());
                let bar = egui::Rect::from_center_size(egui::pos2(ui.max_rect().center().x, bar.center().y), bar.size());
                top_bar(ui.painter(), bar, battle, t);
                ui.vertical_centered(|ui| {
                    out = Some(loaded.view.show(ui, &input));
                });
                // Over the map's top edge, drawn after the map so it stays visible.
                capture_bar(&ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("capture_bar"))), bar, &captures, battle.recorder_team);
            }
        });
        let mut out = out.unwrap();
        if let Some(mini) = out_mini {
            // Clicking the corner minimap moves the 3D camera there.
            if let (Some([x, z]), Some(v)) = (mini.clicked_world, loaded.view3d.as_mut()) {
                v.look_at(x, z);
            }
            // Right-click a tank on the corner minimap: the 3D camera follows it (like in 3D).
            if let (Some(id), Some(v)) = (mini.follow_unit, loaded.view3d.as_mut()) {
                v.set_follow(Some(id));
            }
            out.hovered = mini.hovered.or(out.hovered);
            out.clicked_unit = mini.clicked_unit.or(out.clicked_unit);
            out.clicked_empty |= mini.clicked_empty;
            for (id, p) in mini.anchors {
                out.anchors.entry(id).or_insert(p);
            }
        }
        let menu_opened = out.context_at.is_some() && self.share_base.is_some();
        if let Some(p) = out.context_at.filter(|_| self.share_base.is_some()) {
            self.share_menu = Some(p);
        }
        self.map_hover = out.hovered;
        if let Some(id) = out.clicked_unit {
            self.pinned = Some(id);
        } else if out.clicked_empty {
            self.pinned = None;
        }

        // Hover popup (not interactive) and pinned popup (interactive, follows the tank).
        let realm = battle.server_realm.clone();
        if let Some(id) = out.hovered.filter(|h| Some(*h) != self.pinned) {
            if let Some(anchor) = out.anchors.get(&id) {
                egui::Area::new(egui::Id::new("hover_stats"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(*anchor + egui::vec2(16.0, 12.0))
                    .interactable(false)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| stats_panel(ui, &self.icons, battle, id, t, &realm, false, None));
                    });
            }
        }
        if let Some(id) = self.pinned {
            let anchor = out.anchors.get(&id).copied().unwrap_or(out.rect.left_top() + egui::vec2(10.0, 10.0));
            let mut action = PanelAction::None;
            // Follow toggle only exists in the 3D view.
            let following = loaded.view3d.as_ref().filter(|_| show_3d).map(|v| v.follow() == Some(id));
            egui::Area::new(egui::Id::new("pinned_stats"))
                .order(egui::Order::Foreground)
                .current_pos(anchor + egui::vec2(16.0, 12.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        action = stats_panel(ui, &self.icons, battle, id, t, &realm, true, following);
                    });
                });
            match action {
                PanelAction::Close => self.pinned = None,
                PanelAction::ToggleFollow => {
                    if let Some(v) = loaded.view3d.as_mut() {
                        let now = v.follow();
                        v.set_follow(if now == Some(id) { None } else { Some(id) });
                    }
                }
                PanelAction::None => {}
            }
        }

        if let Some(p) = self.share_menu {
            let link = self.share_link();
            let r = egui::Area::new(egui::Id::new("map_menu")).order(egui::Order::Foreground).fixed_pos(p).show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    if ui.button("Copy link to this moment and view").clicked() {
                        if let Some(l) = &link {
                            ui.ctx().copy_text(l.clone());
                        }
                        self.share_menu = None;
                    }
                    ui.label(RichText::new("Time, 2D/3D, selected tank and 3D camera").small().color(Color32::from_gray(150)));
                });
            });
            // (The right-click that opened it doesn't count as a click elsewhere.)
            if (!menu_opened && r.response.clicked_elsewhere()) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.share_menu = None;
            }
        }

        if settings_changed {
            self.settings.save();
        }
    }
}

/// Scale of everything except the map itself (egui zoom).
pub fn apply_ui_scale(ctx: &egui::Context, scale: f32) {
    let scale = scale.clamp(0.5, 3.0);
    if (ctx.zoom_factor() - scale).abs() > 0.001 {
        ctx.set_zoom_factor(scale);
    }
}


/// Returns true if the user asked to close the panel.
enum PanelAction {
    None,
    Close,
    ToggleFollow,
}

/// `following`: Some(is followed) when the 3D view is shown (offers the Follow toggle).
fn stats_panel(ui: &mut egui::Ui, icons: &super::theme::IconTextures, b: &Battle, id: i32, t: f32, realm: &str, pinned: bool, following: Option<bool>) -> PanelAction {
    use super::theme::{self, Icon};
    let mut close = false;
    let Some(v) = b.vehicles.get(&id) else { return PanelAction::None };
    let s: PlayerStats = scene::stats_at(b, id, t);
    let ally = v.team == b.recorder_team;
    let tile_w = 142.0;
    ui.set_width(tile_w * 2.0 + 8.0);

    // Header: class marker, player [clan], tank + tier, close button.
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
        super::minimap2d::class_icon(ui.painter(), r.center(), 9.0, v.class.unwrap_or(crate::gamedata::VehicleClass::Unknown), team_color(ally));
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&v.name).strong().size(15.0).color(theme::TEXT));
                if !v.clan.is_empty() {
                    ui.label(RichText::new(format!("[{}]", v.clan)).color(theme::MUTED));
                }
            });
            ui.label(RichText::new(format!("{} {}  {}", theme::roman(v.tier), v.tank_name, v.class.map(|c| c.short()).unwrap_or(""))).color(theme::MUTED));
        });
        if pinned {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if ui.small_button("×").clicked() {
                    close = true;
                }
            });
        }
    });

    // HP bar
    let hp = scene::health_at(v, t);
    let dead = s.death_time.or(v.death_time.filter(|d| *d <= t));
    let frac = if dead.is_some() { 0.0 } else { hp.map(|h| h as f32 / v.max_health.max(1) as f32).unwrap_or(1.0).clamp(0.0, 1.0) };
    let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
    ui.painter().rect_filled(bar, 9.0, theme::BG);
    let mut fill = bar;
    fill.set_width(bar.width() * frac);
    ui.painter().rect_filled(fill, 9.0, team_color(ally).gamma_multiply(0.85));
    let hp_text = match (dead, hp) {
        (Some(dt), _) => {
            let killer = s.killed_by.and_then(|k| b.vehicles.get(&k)).map(|k| format!(" by {}", k.name)).unwrap_or_default();
            format!("Destroyed{killer} · {}", fmt_clock(b.clock(dt).remaining.unwrap_or(0.0)))
        }
        (None, Some(h)) => format!("{h} / {}", v.max_health),
        (None, None) => format!("? / {}", v.max_health),
    };
    ui.painter().text(bar.center(), egui::Align2::CENTER_CENTER, hp_text, egui::FontId::proportional(12.0), theme::TEXT);
    ui.add_space(4.0);

    // Stats so far
    ui.label(RichText::new("SO FAR").size(10.5).color(theme::MUTED));
    egui::Grid::new(("now", id, pinned)).num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
        theme::stat_tile(ui, icons, Icon::Damage, Color32::from_rgb(140, 220, 70), &s.damage_dealt.to_string(), &format!("Damage · {} hits", s.damaging_hits), tile_w);
        theme::stat_tile(ui, icons, Icon::Kills, Color32::from_rgb(235, 75, 60), &s.kills.to_string(), "Kills", tile_w);
        ui.end_row();
        theme::stat_tile(ui, icons, Icon::Shell, Color32::from_rgb(140, 220, 70), &s.shots_seen.to_string(), "Shots seen", tile_w);
        theme::stat_tile(ui, icons, Icon::Received, Color32::from_rgb(255, 150, 50), &s.damage_received.to_string(), &format!("Received · {} hits", s.hits_received), tile_w);
        ui.end_row();
    });

    // End-of-battle results (complete replays)
    if let Some(f) = &v.final_stats {
        if t >= b.duration - 0.5 || pinned {
            ui.add_space(4.0);
            ui.label(RichText::new("BATTLE RESULT").size(10.5).color(theme::MUTED));
            let g = |k: &str| f.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
            egui::Grid::new(("final", id, pinned)).num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                theme::stat_tile(ui, icons, Icon::Damage, Color32::from_rgb(140, 220, 70), &g("damageDealt").to_string(), "Damage", tile_w);
                theme::stat_tile(ui, icons, Icon::Assist, Color32::from_rgb(140, 220, 70), &(g("damageAssistedRadio") + g("damageAssistedTrack") + g("damageAssistedStun")).to_string(), "Assisted", tile_w);
                ui.end_row();
                theme::stat_tile(ui, icons, Icon::Blocked, Color32::from_rgb(90, 160, 255), &g("damageBlockedByArmor").to_string(), "Blocked", tile_w);
                theme::stat_tile(ui, icons, Icon::Spotted, Color32::from_rgb(140, 220, 70), &g("spotted").to_string(), "Spotted", tile_w);
                ui.end_row();
                theme::stat_tile(ui, icons, Icon::Kills, Color32::from_rgb(235, 75, 60), &g("kills").to_string(), "Kills", tile_w);
                theme::stat_tile(ui, icons, Icon::Shots, Color32::from_rgb(140, 220, 70), &format!("{}/{}/{}", g("shots"), g("directEnemyHits"), g("piercingEnemyHits")), "Shots/hits/pens", tile_w);
                ui.end_row();
                theme::stat_tile(ui, icons, Icon::Xp, Color32::from_rgb(255, 210, 70), &g("xp").to_string(), "XP", tile_w);
                theme::stat_tile(ui, icons, Icon::Clock, Color32::from_gray(160), &fmt_clock(g("lifeTime") as f32), "Lifetime", tile_w);
                ui.end_row();
            });
        }
    }
    ui.add_space(2.0);
    ui.label(RichText::new("Live stats as seen by the recording player").size(10.0).color(theme::MUTED));

    if pinned {
        let url = if v.account_id > 0 {
            format!("https://tomato.gg/stats/{}/{}%3D{}", realm_code(realm), v.name, v.account_id)
        } else {
            format!("https://tomato.gg/stats/{}/{}", realm_code(realm), v.name)
        };
        ui.hyperlink_to("Player stats on tomato.gg ↗", url);
    }
    if let (true, Some(on)) = (pinned, following) {
        ui.add_space(4.0);
        if ui.add(egui::Button::new(if on { "Following — click to stop" } else { "Follow tank" }).selected(on)).clicked() {
            return PanelAction::ToggleFollow;
        }
        ui.label(RichText::new("Tip: right-click a tank (also on the minimap) to follow it; panning the camera stops following.").small().color(Color32::from_gray(140)));
    }
    if close {
        PanelAction::Close
    } else {
        PanelAction::None
    }
}

fn realm_code(region: &str) -> &'static str {
    match region.to_ascii_uppercase().as_str() {
        "NA" | "US" => "NA",
        "ASIA" | "SEA" => "ASIA",
        _ => "EU",
    }
}
