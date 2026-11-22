//! Replay list table: row hover highlight (whole row + hovered cell), right-click menu
//! (filter by / sort by), and per-column filters (value lists, date calendar with range
//! dragging, damage min/max slider with editable numbers).

use std::collections::BTreeSet;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::library::ReplayEntry as ReplayInfo;
use crate::viewer::theme;

pub const COLUMNS: [&str; 8] = ["Player", "Tank", "Map", "Mode", "Date", "Damage", "Server", "Version"];
const WIDTHS: [f32; 8] = [150.0, 190.0, 140.0, 200.0, 150.0, 80.0, 90.0, 230.0];
const DATE: usize = 4;
const DAMAGE: usize = 5;
const ROW_H: f32 = 26.0;

/// A calendar day as (year, month, day).
pub type Day = (i32, u32, u32);

pub fn cell(info: &ReplayInfo, col: usize) -> String {
    match col {
        0 => info.player_name.clone(),
        1 => info.tank.clone(),
        2 => info.map.clone(),
        3 => info.mode.clone(),
        4 => info.date.clone(),
        5 => info.damage.to_string(),
        6 => info.server.clone(),
        _ => info.version.clone(),
    }
}

/// "dd.mm.yyyy hh:mm:ss" -> (y, m, d)
pub fn day_of(date: &str) -> Option<Day> {
    let d = date.split(' ').next()?;
    let mut it = d.split('.');
    let (dd, mm, yy) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    Some((yy, mm, dd))
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// 0 = Monday
fn weekday(y: i32, m: u32, d: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let w = (y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d as i32) % 7; // 0 = Sunday
    ((w + 6) % 7) as u32
}

#[derive(Default)]
pub struct Table {
    /// Allowed values per text column (None = no filter).
    values: [Option<BTreeSet<String>>; 8],
    /// Inclusive date range.
    dates: Option<(Day, Day)>,
    damage: Option<(i64, i64)>,
    /// Calendar month being shown and a drag in progress.
    cal_month: Option<(i32, u32)>,
    cal_drag: Option<Day>,
    search: [String; 8],
    /// Filter menu that was just opened: its search field takes the keyboard.
    focus_search: Option<usize>,
}

pub enum Action {
    None,
    Open(String),
    Sort(usize, bool),
    /// Upload these replays to the server.
    Upload(Vec<String>),
    /// Copy the web link of an uploaded replay.
    CopyLink(String),
    /// Delete an uploaded replay from the server.
    DeleteUpload(String),
}

/// Filter (funnel) icon centred at `c`, half-width `r`.
fn funnel(p: &egui::Painter, c: Pos2, r: f32, color: Color32) {
    let top = c.y - r * 0.8;
    let mid = c.y + r * 0.1;
    let pts = vec![
        Pos2::new(c.x - r, top),
        Pos2::new(c.x + r, top),
        Pos2::new(c.x + r * 0.22, mid),
        Pos2::new(c.x + r * 0.22, c.y + r * 0.95),
        Pos2::new(c.x - r * 0.22, c.y + r * 0.65),
        Pos2::new(c.x - r * 0.22, mid),
    ];
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

impl Table {
    pub fn any_filter(&self) -> bool {
        self.values.iter().any(Option::is_some) || self.dates.is_some() || self.damage.is_some()
    }

    pub fn passes(&self, r: &ReplayInfo) -> bool {
        for (c, allowed) in self.values.iter().enumerate() {
            if let Some(set) = allowed {
                if !set.contains(&cell(r, c)) {
                    return false;
                }
            }
        }
        if let Some((a, b)) = self.dates {
            match day_of(&r.date) {
                Some(d) if d >= a && d <= b => {}
                _ => return false,
            }
        }
        if let Some((lo, hi)) = self.damage {
            if r.damage < lo || r.damage > hi {
                return false;
            }
        }
        true
    }

    fn filter_by_value(&mut self, col: usize, value: String) {
        match col {
            DATE => {
                if let Some(d) = day_of(&value) {
                    self.dates = Some((d, d));
                }
            }
            DAMAGE => {
                if let Ok(v) = value.parse::<i64>() {
                    self.damage = Some((v, v));
                }
            }
            _ => self.values[col] = Some(BTreeSet::from([value])),
        }
    }

    fn clear(&mut self, col: usize) {
        match col {
            DATE => self.dates = None,
            DAMAGE => self.damage = None,
            _ => self.values[col] = None,
        }
    }

    fn is_filtered(&self, col: usize) -> bool {
        match col {
            DATE => self.dates.is_some(),
            DAMAGE => self.damage.is_some(),
            _ => self.values[col].is_some(),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, replays: &[ReplayInfo], selected: &mut Option<usize>, sort: (usize, bool)) -> Action {
        let mut action = Action::None;
        let visible: Vec<usize> = (0..replays.len()).filter(|&i| self.passes(&replays[i])).collect();

        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} of {} replays", visible.len(), replays.len())).color(theme::MUTED));
            if self.any_filter() && ui.small_button("Clear filters").clicked() {
                *self = Table { cal_month: self.cal_month, ..Default::default() };
            }
        });

        let total_w: f32 = WIDTHS.iter().sum();
        egui::ScrollArea::horizontal().auto_shrink([false; 2]).show(ui, |ui| {
            ui.set_min_width(total_w);
            // Header: one cell per column at exactly the column's position; click = sort,
            // funnel = filter popup.
            let (hrect, _) = ui.allocate_exact_size(Vec2::new(total_w, 28.0), Sense::hover());
            let mut x = hrect.left();
            for (c, name) in COLUMNS.iter().enumerate() {
                let cell_rect = Rect::from_min_size(Pos2::new(x, hrect.top()), Vec2::new(WIDTHS[c], hrect.height()));
                x += WIDTHS[c];
                let funnel_rect = Rect::from_center_size(Pos2::new(cell_rect.right() - 14.0, cell_rect.center().y), Vec2::splat(20.0));
                let label_rect = Rect::from_min_max(cell_rect.min, Pos2::new(funnel_rect.left() - 2.0, cell_rect.max.y));
                let active = sort.0 == c;
                let arrow = if !active { "" } else if sort.1 { " \u{2B07}" } else { " \u{2B06}" };
                let label = egui::Button::selectable(active, RichText::new(format!("{name}{arrow}")).strong());
                if ui.put(label_rect, label).on_hover_text("Sort").clicked() {
                    action = Action::Sort(c, if active { !sort.1 } else { matches!(c, DATE | DAMAGE) });
                }
                let resp = ui.interact(funnel_rect, ui.scope_id().with(("filter", c)), Sense::click()).on_hover_text("Filter");
                let color = if self.is_filtered(c) {
                    theme::ACCENT
                } else if resp.hovered() {
                    theme::TEXT
                } else {
                    theme::MUTED
                };
                funnel(ui.painter(), funnel_rect.center(), 6.0, color);
                let popup_id = ui.scope_id().with(("filter_popup", c));
                if resp.clicked() && !egui::Popup::is_id_open(ui.ctx(), popup_id) {
                    self.focus_search = Some(c);
                }
                egui::Popup::from_toggle_button_response(&resp)
                    .id(popup_id)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| self.filter_popup(ui, c, replays));
            }
            ui.separator();

            // Rows (virtualised).
            let pointer = ui.ctx().pointer_hover_pos();
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show_rows(ui, ROW_H, visible.len(), |ui, range| {
                for vi in range {
                    let idx = visible[vi];
                    let info = &replays[idx];
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, ROW_H), Sense::click());
                    let hovered = resp.hovered();
                    let is_sel = *selected == Some(idx);
                    let p = ui.painter();
                    let bg = if is_sel {
                        theme::ACCENT.gamma_multiply(0.28)
                    } else if hovered {
                        theme::CARD_HOVER
                    } else if vi % 2 == 1 {
                        Color32::from_rgb(22, 26, 33)
                    } else {
                        Color32::TRANSPARENT
                    };
                    p.rect_filled(rect, 4.0, bg);
                    let mut x = rect.left();
                    let mut hovered_col = None;
                    for c in 0..COLUMNS.len() {
                        let cr = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(WIDTHS[c], ROW_H));
                        if hovered && pointer.map(|pt| cr.contains(pt)).unwrap_or(false) {
                            hovered_col = Some(c);
                            p.rect_filled(cr.shrink(1.0), 4.0, Color32::from_white_alpha(14));
                            p.rect_stroke(cr.shrink(1.0), 4.0, Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.5)), egui::StrokeKind::Middle);
                        }
                        let text = cell(info, c);
                        let galley = p.layout_no_wrap(text, egui::FontId::proportional(13.5), if is_sel { theme::TEXT } else { Color32::from_gray(205) });
                        let clip = p.with_clip_rect(cr.shrink2(Vec2::new(6.0, 0.0)));
                        clip.galley(Pos2::new(cr.left() + 8.0, cr.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
                        x += WIDTHS[c];
                    }
                    if resp.clicked() {
                        *selected = Some(idx);
                    }
                    if resp.double_clicked() {
                        action = Action::Open(info.path.clone());
                    }
                    let col = hovered_col.unwrap_or(0);
                    resp.context_menu(|ui| {
                        *selected = Some(idx);
                        let value = cell(info, col);
                        let short: String = value.chars().take(32).collect();
                        if ui.button(format!("Filter by {} = {short}", COLUMNS[col])).clicked() {
                            self.filter_by_value(col, value.clone());
                            ui.close();
                        }
                        if self.is_filtered(col) && ui.button(format!("Clear {} filter", COLUMNS[col])).clicked() {
                            self.clear(col);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button(format!("Sort by {} \u{2B06}", COLUMNS[col])).clicked() {
                            action = Action::Sort(col, false);
                            ui.close();
                        }
                        if ui.button(format!("Sort by {} \u{2B07}", COLUMNS[col])).clicked() {
                            action = Action::Sort(col, true);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Open minimap replay").clicked() {
                            action = Action::Open(info.path.clone());
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Upload to wot.arne.my").clicked() {
                            action = Action::Upload(vec![info.path.clone()]);
                            ui.close();
                        }
                        let shown: Vec<String> = replays.iter().filter(|r| self.passes(r)).map(|r| r.path.clone()).collect();
                        if shown.len() > 1 && ui.button(format!("Upload all {} shown replays", shown.len())).clicked() {
                            action = Action::Upload(shown);
                            ui.close();
                        }
                        if ui.button("Copy web link").clicked() {
                            action = Action::CopyLink(info.path.clone());
                            ui.close();
                        }
                        if ui.button("Delete from server").clicked() {
                            action = Action::DeleteUpload(info.path.clone());
                            ui.close();
                        }
                    });
                }
            });
        });
        action
    }

    fn filter_popup(&mut self, ui: &mut egui::Ui, c: usize, replays: &[ReplayInfo]) {
        ui.set_min_width(240.0);
        match c {
            DATE => self.date_filter(ui, replays),
            DAMAGE => self.damage_filter(ui, replays),
            _ => self.value_filter(ui, c, replays),
        }
        ui.separator();
        if ui.button("Clear").clicked() {
            self.clear(c);
        }
    }

    fn value_filter(&mut self, ui: &mut egui::Ui, c: usize, replays: &[ReplayInfo]) {
        let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for r in replays {
            *counts.entry(cell(r, c)).or_default() += 1;
        }
        let search = ui.add(egui::TextEdit::singleline(&mut self.search[c]).hint_text("Type to search"));
        if self.focus_search == Some(c) {
            search.request_focus();
            self.focus_search = None;
        }
        ui.horizontal(|ui| {
            if ui.small_button("All").clicked() {
                self.values[c] = None;
            }
            if ui.small_button("None").clicked() {
                self.values[c] = Some(BTreeSet::new());
            }
        });
        let q = self.search[c].to_lowercase();
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for (v, n) in &counts {
                if !q.is_empty() && !v.to_lowercase().contains(&q) {
                    continue;
                }
                let mut on = self.values[c].as_ref().map(|s| s.contains(v)).unwrap_or(true);
                if ui.checkbox(&mut on, format!("{}  ({n})", if v.is_empty() { "(empty)" } else { v })).changed() {
                    match &mut self.values[c] {
                        // Everything shown: a click means "only this one".
                        None => self.values[c] = Some(BTreeSet::from([v.clone()])),
                        Some(set) => {
                            if on {
                                set.insert(v.clone());
                            } else {
                                set.remove(v);
                            }
                            // Nothing or everything ticked = no filter.
                            if set.is_empty() || set.len() == counts.len() {
                                self.values[c] = None;
                            }
                        }
                    }
                }
            }
        });
    }

    fn damage_filter(&mut self, ui: &mut egui::Ui, replays: &[ReplayInfo]) {
        let max_dmg = replays.iter().map(|r| r.damage).max().unwrap_or(0).max(1);
        let (mut lo, mut hi) = self.damage.unwrap_or((0, max_dmg));
        // Two-handle range slider.
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(220.0), 26.0), Sense::click_and_drag());
        let to_x = |v: i64| rect.left() + 8.0 + (v as f32 / max_dmg as f32) * (rect.width() - 16.0);
        let to_v = |x: f32| (((x - rect.left() - 8.0) / (rect.width() - 16.0)).clamp(0.0, 1.0) * max_dmg as f32).round() as i64;
        let p = ui.painter();
        let y = rect.center().y;
        p.line_segment([Pos2::new(rect.left() + 8.0, y), Pos2::new(rect.right() - 8.0, y)], Stroke::new(4.0_f32, theme::BG));
        p.line_segment([Pos2::new(to_x(lo), y), Pos2::new(to_x(hi), y)], Stroke::new(4.0_f32, theme::ACCENT));
        for v in [lo, hi] {
            p.circle(Pos2::new(to_x(v), y), 7.0, theme::TEXT, Stroke::new(2.0_f32, theme::ACCENT));
        }
        if let Some(pt) = resp.interact_pointer_pos() {
            if resp.dragged() || resp.clicked() {
                let v = to_v(pt.x);
                if (pt.x - to_x(lo)).abs() <= (pt.x - to_x(hi)).abs() {
                    lo = v.min(hi);
                } else {
                    hi = v.max(lo);
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label("Min");
            ui.add(egui::DragValue::new(&mut lo).range(0..=hi).speed(10.0));
            ui.label("Max");
            ui.add(egui::DragValue::new(&mut hi).range(lo..=max_dmg.max(lo)).speed(10.0));
        });
        self.damage = if lo == 0 && hi == max_dmg { None } else { Some((lo, hi)) };
    }

    fn date_filter(&mut self, ui: &mut egui::Ui, replays: &[ReplayInfo]) {
        let days: BTreeSet<Day> = replays.iter().filter_map(|r| day_of(&r.date)).collect();
        let latest = days.iter().next_back().copied().unwrap_or((2026, 1, 1));
        let (mut y, mut m) = self.cal_month.unwrap_or((latest.0, latest.1));
        ui.horizontal(|ui| {
            if ui.small_button("<").clicked() {
                if m == 1 {
                    m = 12;
                    y -= 1;
                } else {
                    m -= 1;
                }
            }
            const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
            ui.label(RichText::new(format!("{} {y}", MONTHS[(m - 1) as usize])).strong());
            if ui.small_button(">").clicked() {
                if m == 12 {
                    m = 1;
                    y += 1;
                } else {
                    m += 1;
                }
            }
        });
        self.cal_month = Some((y, m));
        let cell_sz = 30.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(cell_sz * 7.0, cell_sz * 7.0), Sense::hover());
        let p = ui.painter();
        for (i, wd) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
            p.text(Pos2::new(rect.left() + (i as f32 + 0.5) * cell_sz, rect.top() + cell_sz * 0.5), egui::Align2::CENTER_CENTER, *wd, egui::FontId::proportional(11.0), theme::MUTED);
        }
        let first = weekday(y, m, 1);
        let pointer_down = ui.input(|i| i.pointer.primary_down());
        let mut released = false;
        for d in 1..=days_in_month(y, m) {
            let slot = first + d - 1;
            let r = Rect::from_min_size(Pos2::new(rect.left() + (slot % 7) as f32 * cell_sz, rect.top() + (1 + slot / 7) as f32 * cell_sz), Vec2::splat(cell_sz)).shrink(2.0);
            let day = (y, m, d);
            let resp = ui.interact(r, ui.scope_id().with(("day", y, m, d)), Sense::click_and_drag());
            if resp.drag_started() || (resp.is_pointer_button_down_on() && self.cal_drag.is_none()) {
                self.cal_drag = Some(day);
                self.dates = Some((day, day));
            }
            if let Some(start) = self.cal_drag {
                if pointer_down && ui.rect_contains_pointer(r) {
                    self.dates = Some((start.min(day), start.max(day)));
                }
            }
            if resp.drag_stopped() || resp.clicked() {
                released = true;
            }
            let in_range = self.dates.map(|(a, b)| day >= a && day <= b).unwrap_or(false);
            let has = days.contains(&day);
            let p = ui.painter();
            if in_range {
                p.rect_filled(r, 6.0, theme::ACCENT.gamma_multiply(0.55));
            } else if resp.hovered() {
                p.rect_filled(r, 6.0, theme::CARD_HOVER);
            }
            p.text(r.center(), egui::Align2::CENTER_CENTER, d.to_string(), egui::FontId::proportional(12.5), if has { theme::TEXT } else { Color32::from_gray(90) });
            if has {
                p.circle_filled(Pos2::new(r.center().x, r.bottom() - 3.0), 1.8, theme::ACCENT);
            }
        }
        if released || !pointer_down {
            self.cal_drag = None;
        }
        if let Some((a, b)) = self.dates {
            ui.label(RichText::new(if a == b { format!("{:02}.{:02}.{}", a.2, a.1, a.0) } else { format!("{:02}.{:02}.{} – {:02}.{:02}.{}", a.2, a.1, a.0, b.2, b.1, b.0) }).color(theme::MUTED));
        }
        ui.label(RichText::new("Click a day, or drag across days for a range").size(11.0).color(theme::MUTED));
    }
}
