//! The desktop window: replay library on the start page, the viewer when a replay is open.

use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use eframe::egui::{self, Color32, RichText};

use super::config::Config;
use super::library::{self, ReplayEntry, ScanEvent};
use super::table::{Action, Table};
use super::desktop;
use super::update::{self, Updater};
use super::upload::{self, Uploader};
use crate::viewer::player::native::Engine;
use crate::viewer::player::ReplayViewer;

pub struct App {
    cfg: Config,
    entries: Vec<ReplayEntry>,
    status: String,
    selected: Option<usize>,
    scan: Option<mpsc::Receiver<ScanEvent>>,
    /// Settings window: Some(edited copy) while open.
    editing: Option<Config>,
    viewer: Option<ReplayViewer>,
    /// Game data shared between viewers (loaded once).
    engine: Option<Arc<Engine>>,
    first_frame: bool,
    /// Sorted column (see `table::COLUMNS`) and direction; default newest first.
    sort: (usize, bool),
    table: Table,
    uploader: Uploader,
    /// Settings window: chosen validity (days) for a new live viewing code.
    code_days: u32,
    /// Result of a code request running in the background.
    code_rx: Option<mpsc::Receiver<Result<CodeOp, String>>>,
    code_msg: String,
    /// Consent dialog open: replays waiting to be uploaded (empty = for live uploads).
    consent: Option<Vec<PathBuf>>,
    /// UI scale on the replay list (shared with the viewer's setting).
    ui_scale: f32,
    /// The battle being played right now (`temp.wotreplay`): its size when read, and its summary.
    /// Filled by a background thread (reading it needs the game data).
    in_progress: Arc<std::sync::Mutex<Option<(u64, ReplayEntry)>>>,
    in_progress_loading: Arc<std::sync::atomic::AtomicBool>,
    in_progress_checked: f64,
    /// The consent dialog is the first-start question.
    consent_first: bool,
    /// "Delete all my data" was clicked once (asks again before deleting).
    confirm_delete: bool,
    /// New versions (background check, download, install).
    updater: Updater,
    /// The update notice was closed for this state.
    update_dismissed: Option<update::State>,
    /// Last update state shown in the tray tooltip.
    tray_state: Option<update::State>,
    /// Go to the tray in the next frame (close button).
    hide_next: bool,
    /// Started with `--tray`: go there after this many frames (eframe shows the window itself
    /// after its first frame, which would undo it).
    tray_in_frames: u8,
}

enum CodeOp {
    Created(String, u64),
    Revoked(String),
    /// Server data requests (download / delete): message for the user.
    Done(String),
}

/// What uploading means, shown before the first upload (and in Settings).
const UPLOAD_NOTICE: &str = "Uploaded battles are public on wot.arne.my once they are over, including the names, clan tags, \
account IDs and battle results of all players in them. The original replay file is stored too, but not shown to anyone. A live battle is only visible to people with one of your viewing codes \
until it ends. You can delete your uploads at any time (Settings > Server, or right-click a replay).";

impl Default for App {
    fn default() -> Self {
        crate::dirs::migrate_old_folders();
        let cfg = Config::load();
        // First start: ask once whether to upload battles.
        let first_ask = !cfg.upload_asked && !cfg.upload_consent;
        // The start-up entry follows the exe (it may have moved).
        if cfg.autostart {
            let _ = desktop::set_autostart(true);
        }
        let uploader = Uploader::start(cfg.clone());
        Self {
            updater: {
                let (temp, status) = (PathBuf::from(&cfg.replays_dir).join("temp.wotreplay"), uploader.status.clone());
                let idle = move || std::fs::metadata(&temp).map_or(true, |m| m.len() == 0) && status.lock().map_or(false, |s| s.queued == 0);
                Updater::start(cfg.update_notify, cfg.auto_update && Updater::can_install(), Box::new(idle))
            },
            hide_next: false,
            tray_in_frames: 0,
            update_dismissed: None,
            tray_state: None,
            uploader,
            code_days: 7,
            code_rx: None,
            code_msg: String::new(),
            consent: first_ask.then(Vec::new),
            consent_first: first_ask,
            ui_scale: crate::viewer::settings::ViewerSettings::load().ui_scale,
            in_progress: Default::default(),
            in_progress_loading: Default::default(),
            in_progress_checked: -10.0,
            confirm_delete: false,
            cfg,
            entries: Vec::new(),
            status: String::new(),
            selected: None,
            scan: None,
            editing: None,
            viewer: None,
            engine: None,
            first_frame: true,
            sort: (4, true),
            table: Table::default(),
        }
    }
}

impl App {
    fn rescan(&mut self) {
        self.scan = Some(library::scan(PathBuf::from(&self.cfg.replays_dir), self.cfg.game_dir.clone()));
    }

    fn sort_entries(&mut self) {
        let keep = self.selected.and_then(|i| self.entries.get(i)).map(|e| e.path.clone());
        let (col, desc) = self.sort;
        let text = |s: &str| s.to_lowercase();
        self.entries.sort_by(|a, b| {
            let primary = match col {
                0 => text(&a.player_name).cmp(&text(&b.player_name)),
                1 => text(&a.tank).cmp(&text(&b.tank)),
                2 => text(&a.map).cmp(&text(&b.map)),
                3 => a.mode.cmp(&b.mode),
                5 => a.damage.cmp(&b.damage),
                6 => a.server.cmp(&b.server),
                7 => a.version.cmp(&b.version),
                _ => a.date_key().cmp(&b.date_key()),
            };
            let primary = if desc { primary.reverse() } else { primary };
            // Ties: newest first.
            primary.then_with(|| b.date_key().cmp(&a.date_key()))
        });
        self.selected = keep.and_then(|p| self.entries.iter().position(|e| e.path == p));
    }

    fn engine_from_viewer(&mut self) {
        if self.engine.is_none() {
            self.engine = self.viewer.as_ref().and_then(|v| v.engine.clone());
        }
    }

    pub fn open_viewer(&mut self, path: PathBuf) {
        self.engine_from_viewer();
        let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let mut v = ReplayViewer::open(path, self.engine.clone(), self.cfg.game_dir.clone());
        // Uploaded replays can be shared (right-click the map).
        v.share_base = upload::uploaded().get(&name).map(|id| upload::battle_link(id));
        self.viewer = Some(v);
    }

    /// Open a replay `at` seconds after battle start (paused).
    pub fn open_viewer_at(&mut self, path: PathBuf, at: f32) {
        self.open_viewer(path);
        if let Some(v) = &mut self.viewer {
            v.start_at = Some(at);
        }
    }

    /// Follow the battle being played right now.
    pub fn open_live(&mut self) {
        self.engine_from_viewer();
        self.viewer = Some(ReplayViewer::open_live(PathBuf::from(&self.cfg.replays_dir), self.engine.clone(), self.cfg.game_dir.clone()));
    }

    /// Apply a share link's options (`--link "t=..&v=3d&cam=.."`, testing).
    pub fn apply_link(&mut self, query: &str) {
        if let Some(v) = &mut self.viewer {
            v.apply_link(query);
        }
    }

    /// Look for the battle in progress in another folder (`--replays <dir>`, testing; not saved).
    pub fn watch_replays_dir(&mut self, dir: String) {
        self.cfg.replays_dir = dir;
    }

    /// Go to the tray in the first frame (`--tray`: started with the computer).
    pub fn start_in_tray(&mut self) {
        self.tray_in_frames = 3;
    }

    /// Open the settings window (`--settings`, testing).
    pub fn open_settings(&mut self) {
        self.editing = Some(self.cfg.clone());
    }

    pub fn force_3d(&mut self) {
        if let Some(v) = &mut self.viewer {
            v.set_3d(true);
        }
    }

    pub fn force_2d(&mut self) {
        if let Some(v) = &mut self.viewer {
            v.set_3d(false);
        }
    }

    /// Start playing right away (also after `--at`).
    pub fn autoplay(&mut self) {
        if let Some(v) = &mut self.viewer {
            v.autoplay = true;
        }
    }

    /// Open with your own tank's stats panel pinned (testing).
    pub fn pin_own(&mut self) {
        if let Some(v) = &mut self.viewer {
            v.pin_own = true;
        }
    }

    /// Point the 3D camera at a world position (x, z).
    pub fn look_at(&mut self, x: f32, z: f32) {
        if let Some(v) = &mut self.viewer {
            v.look_at = Some([x, z]);
        }
    }

    /// Camera orbit for `--look` (yaw and pitch in degrees, distance in metres).
    pub fn orbit(&mut self, yaw: f32, pitch: f32, dist: f32) {
        if let Some(v) = &mut self.viewer {
            v.orbit = Some([yaw.to_radians(), pitch.to_radians(), dist]);
        }
    }

    fn library_page(&mut self, ctx: &egui::Context) {
        for ev in self.scan.as_ref().map(|rx| rx.try_iter().collect::<Vec<_>>()).unwrap_or_default() {
            match ev {
                ScanEvent::Progress { done, total } => self.status = format!("Reading replays {done}/{total}"),
                ScanEvent::Finished(entries) => {
                    self.status = format!("{} replays", entries.len());
                    self.entries = entries;
                    self.sort_entries();
                    self.scan = None;
                }
            }
        }
        if self.scan.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        egui::TopBottomPanel::top("library_top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("LiveReplay");
                ui.separator();
                if ui.button("Settings").clicked() {
                    self.editing = Some(self.cfg.clone());
                }
                if ui.add_enabled(self.scan.is_none(), egui::Button::new("Rescan")).clicked() {
                    self.rescan();
                }
                if ui
                    .button(RichText::new("Live battle").color(Color32::from_rgb(255, 90, 80)))
                    .on_hover_text("Follow the battle you are playing right now (while World of Tanks is running)")
                    .clicked()
                {
                    self.open_live();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let chosen = self.selected.and_then(|i| self.entries.get(i)).map(|e| PathBuf::from(&e.path));
                    if ui.add_enabled(chosen.is_some(), egui::Button::new("Watch in game")).clicked() {
                        if let Err(e) = super::launch::watch_in_game(&self.cfg, chosen.as_deref().unwrap()) {
                            self.status = e;
                        }
                    }
                    if ui.add_enabled(chosen.is_some(), egui::Button::new("Open")).clicked() {
                        self.open_viewer(chosen.clone().unwrap());
                    }
                    let up = self.uploader.status.lock().unwrap().clone();
                    let line = if up.line.is_empty() { self.status.clone() } else { format!("{}   {}", self.status, up.line) };
                    // One line, cut to the free space (long errors used to run over the buttons).
                    let w = ui.available_width().max(40.0);
                    ui.add_sized([w, 20.0], egui::Label::new(RichText::new(&line).color(Color32::from_gray(160))).truncate()).on_hover_text(&line);
                    if up.queued > 0 {
                        ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    }
                });
            });
            ui.add_space(4.0);
        });

        // The battle in progress: checked every 2 s; read once per battle (the file only grows).
        let now = ctx.input(|i| i.time);
        if now - self.in_progress_checked > 2.0 {
            self.in_progress_checked = now;
            let path = PathBuf::from(&self.cfg.replays_dir).join("temp.wotreplay");
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let mut cur = self.in_progress.lock().unwrap();
            if len == 0 {
                *cur = None;
            } else if cur.as_ref().map_or(true, |(l, _)| len < *l) {
                use std::sync::atomic::Ordering;
                if !self.in_progress_loading.swap(true, Ordering::SeqCst) {
                    let (out, busy) = (self.in_progress.clone(), self.in_progress_loading.clone());
                    let (engine, game_dir, ctx) = (self.engine.clone(), self.cfg.game_dir.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        let entry = crate::viewer::player::native::get_engine(engine, &game_dir).ok().and_then(|e| {
                            let b = crate::replay::battle::Battle::load(&path, &e.game, &e.defs).ok()?;
                            Some(library::summarize(&path, Some(&e.game)).unwrap_or_else(|| library::live_entry(&path, &b, &e.game)))
                        });
                        if let Some(e) = entry.filter(|e| !e.player_name.is_empty()) {
                            *out.lock().unwrap() = Some((len, e));
                            ctx.request_repaint();
                        }
                        busy.store(false, Ordering::SeqCst);
                    });
                }
            } else if let Some((l, _)) = cur.as_mut() {
                *l = len;
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        let mut watch_live = false;
        let in_progress = self.in_progress.lock().unwrap().as_ref().map(|(_, e)| e.clone());
        if let Some(e) = &in_progress {
            egui::TopBottomPanel::top("in_progress").show(ctx, |ui| {
                ui.add_space(4.0);
                let red = Color32::from_rgb(255, 80, 70);
                let text = format!("LIVE   {}   {}   {}   {}   in progress", e.player_name, e.tank, e.map, e.mode);
                let b = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
                    .fill(red.gamma_multiply(0.35))
                    .stroke(egui::Stroke::new(1.0_f32, red))
                    .min_size(egui::vec2(ui.available_width(), 30.0));
                watch_live = ui.add(b).on_hover_text("Watch the battle you are playing right now").clicked();
                ui.add_space(2.0);
            });
        }
        if watch_live {
            self.open_live();
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.table.show(ui, &self.entries, &mut self.selected, self.sort) {
            Action::Open(p) => self.open_viewer(PathBuf::from(p)),
            Action::Sort(col, desc) => {
                self.sort = (col, desc);
                self.sort_entries();
            }
            Action::Upload(paths) => {
                let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
                if self.cfg.upload_consent {
                    self.uploader.upload(paths);
                } else {
                    self.consent = Some(paths);
                }
            }
            Action::DeleteUpload(path) => {
                let name = std::path::Path::new(&path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                self.status = match upload::delete_upload(&self.cfg.upload_key, &name) {
                    Ok(()) => "Deleted from the server".into(),
                    Err(e) => format!("Delete failed: {e}"),
                };
            }
            Action::CopyLink(path) => {
                let name = std::path::Path::new(&path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                match upload::uploaded().get(&name) {
                    Some(id) => {
                        ctx.copy_text(upload::battle_link(id));
                        self.status = "Web link copied".into();
                    }
                    None => self.status = "Not uploaded yet (right-click > Upload)".into(),
                }
            }
            Action::None => {}
        });
    }

    /// Asked once before anything is uploaded.
    fn consent_window(&mut self, ctx: &egui::Context) {
        if self.consent.is_none() {
            return;
        }
        let (mut yes, mut no) = (false, false);
        let title = if self.consent_first { "Upload your battles?" } else { "Upload to wot.arne.my?" };
        egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.set_max_width(460.0);
            if self.consent_first {
                ui.add(egui::Label::new("LiveReplay can upload the battles you play to wot.arne.my, so you can watch them in the browser, share them and let friends with a code watch you live. You can change this any time in Settings.").wrap());
                ui.add_space(6.0);
            }
            ui.add(egui::Label::new(RichText::new(UPLOAD_NOTICE).color(Color32::from_gray(170))).wrap());
            ui.add_space(4.0);
            ui.hyperlink_to("Privacy policy", upload::privacy_url());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                // Grey "no" on the left, blue "yes" on the right.
                let size = egui::vec2(130.0, 32.0);
                no = ui.add(egui::Button::new(RichText::new("Don't upload").color(Color32::from_gray(225))).fill(Color32::from_gray(68)).min_size(size)).clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    yes = ui.add(egui::Button::new(RichText::new("Upload").color(Color32::WHITE).strong()).fill(Color32::from_rgb(38, 112, 226)).min_size(size)).clicked();
                });
            });
        });
        if yes || no {
            self.cfg.upload_asked = true;
            if let Some(d) = &mut self.editing {
                d.upload_asked = true;
            }
        }
        if yes {
            let paths = self.consent.take().unwrap_or_default();
            self.cfg.upload_consent = true;
            if paths.is_empty() {
                // Live uploads on (first start, or the Settings checkbox).
                self.cfg.upload_live = true;
            }
            if let Some(d) = &mut self.editing {
                d.upload_consent = true;
                if paths.is_empty() {
                    d.upload_live = true;
                }
            }
            self.cfg.save();
            self.uploader.set_config(self.cfg.clone());
            if !paths.is_empty() {
                self.uploader.upload(paths);
            }
            self.consent_first = false;
        } else if no {
            self.consent = None;
            self.consent_first = false;
            self.cfg.save();
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(draft) = &mut self.editing else { return };
        let mut open = true;
        let mut save = false;
        egui::Window::new("Settings").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            egui::Grid::new("settings_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Game folder");
                ui.add(egui::TextEdit::singleline(&mut draft.game_dir).desired_width(380.0));
                ui.end_row();
                ui.label("Replays folder");
                ui.add(egui::TextEdit::singleline(&mut draft.replays_dir).desired_width(380.0));
                ui.end_row();
                if !cfg!(windows) {
                    ui.label("Bottles bottle");
                    ui.add(egui::TextEdit::singleline(&mut draft.bottle).desired_width(380.0));
                    ui.end_row();
                }
            });
            ui.add_space(10.0);
            ui.label(RichText::new("App").strong());
            if desktop::tray_supported() {
                let r = ui.checkbox(&mut draft.autostart, "Start with the computer (in the tray)");
                if cfg!(windows) {
                    r.on_hover_text("Note: LiveReplay is not code-signed yet, and Windows Defender may take a program that adds itself to the start-up apps for malware and block it");
                }
                ui.checkbox(&mut draft.close_to_tray, "Closing the window keeps LiveReplay in the tray")
                    .on_hover_text(desktop::TRAY_HINT);
            }
            ui.checkbox(&mut draft.update_notify, "Tell me about new versions");
            if Updater::can_install() {
                ui.add_enabled(draft.update_notify, egui::Checkbox::new(&mut draft.auto_update, "Install new versions by itself"))
                    .on_hover_text("Downloaded from the LiveReplay releases on GitHub and used from the next start");
            }
            ui.label(RichText::new(format!("LiveReplay {}", env!("CARGO_PKG_VERSION"))).small().color(Color32::from_gray(150)));
            ui.add_space(10.0);
            ui.label(RichText::new("Server (wot.arne.my)").strong());
            if ui.checkbox(&mut draft.upload_live, "Upload the battles I play").changed() && draft.upload_live && !draft.upload_consent {
                draft.upload_live = false;
                self.consent = Some(Vec::new());
            }
            ui.add_enabled(draft.upload_live, egui::Checkbox::new(&mut draft.upload_during_battle, "Already during the battle (live, for viewing codes)"))
                .on_hover_text("Off: each battle is uploaded when it is over, so nothing uses your connection while you play");
            ui.add(egui::Label::new(RichText::new(UPLOAD_NOTICE).small().color(Color32::from_gray(150))).wrap());
            ui.label(RichText::new("Past replays: right-click one in the list > Upload.").color(Color32::from_gray(150)));
            ui.horizontal(|ui| {
                ui.hyperlink_to("Privacy policy", upload::privacy_url());
                ui.hyperlink_to("Open-source licences", "https://github.com/Windows200000/LiveReplay/blob/main/THIRD_PARTY_LICENSES.md");
                if ui.add_enabled(self.code_rx.is_none(), egui::Button::new("Download my server data")).clicked() {
                    let (tx, rx) = mpsc::channel();
                    let key = draft.upload_key.clone();
                    std::thread::spawn(move || {
                        let r = upload::my_data(&key).and_then(|json| {
                            let dir = directories::UserDirs::new().and_then(|u| u.download_dir().map(PathBuf::from)).or_else(crate::dirs::config_dir).ok_or("no folder to save to")?;
                            let p = dir.join("livereplay-my-server-data.json");
                            std::fs::write(&p, json).map_err(|e| e.to_string())?;
                            Ok(CodeOp::Done(format!("Saved to {}", p.display())))
                        });
                        let _ = tx.send(r);
                    });
                    self.code_rx = Some(rx);
                }
                let label = if self.confirm_delete { "Click again: delete ALL my uploads" } else { "Delete all my data on the server" };
                if ui.add_enabled(self.code_rx.is_none(), egui::Button::new(RichText::new(label).color(Color32::from_rgb(255, 120, 110)))).clicked() {
                    if self.confirm_delete {
                        self.confirm_delete = false;
                        let (tx, rx) = mpsc::channel();
                        let key = draft.upload_key.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(upload::delete_my_data(&key).map(|n| CodeOp::Done(format!("Deleted {n} battles, your codes and player names from the server"))));
                        });
                        self.code_rx = Some(rx);
                    } else {
                        self.confirm_delete = true;
                    }
                }
            });
            ui.add_space(6.0);
            ui.label("Live viewing codes (let someone watch your battles while you play):");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("code_days")
                    .selected_text(match self.code_days { 1 => "1 day", 7 => "1 week", _ => "1 month" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.code_days, 1, "1 day");
                        ui.selectable_value(&mut self.code_days, 7, "1 week");
                        ui.selectable_value(&mut self.code_days, 31, "1 month");
                    });
                if ui.add_enabled(self.code_rx.is_none(), egui::Button::new("New code")).clicked() {
                    let (tx, rx) = mpsc::channel();
                    let (key, days) = (draft.upload_key.clone(), self.code_days);
                    std::thread::spawn(move || {
                        let _ = tx.send(upload::create_code(&key, days).map(|c| CodeOp::Created(c.code, c.expires)));
                    });
                    self.code_rx = Some(rx);
                }
                ui.label(RichText::new(&self.code_msg).color(Color32::from_gray(150)));
            });
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            draft.codes.retain(|c| c.1 > now);
            for (code, expires) in draft.codes.clone() {
                ui.horizontal(|ui| {
                    let pretty = format!("{}-{}", &code[..code.len() / 2], &code[code.len() / 2..]);
                    ui.label(RichText::new(&pretty).monospace().strong());
                    let left = expires.saturating_sub(now);
                    ui.label(RichText::new(if left > 86400 { format!("{} days left", left / 86400) } else { format!("{} h left", left / 3600) }).color(Color32::from_gray(150)));
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(pretty.clone());
                    }
                    if ui.add_enabled(self.code_rx.is_none(), egui::Button::new("Revoke").small()).clicked() {
                        let (tx, rx) = mpsc::channel();
                        let key = draft.upload_key.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(upload::revoke_code(&key, &code).map(|_| CodeOp::Revoked(code)));
                        });
                        self.code_rx = Some(rx);
                    }
                });
            }
            if let Some(r) = self.code_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                self.code_rx = None;
                match r {
                    Ok(CodeOp::Created(code, expires)) => {
                        draft.codes.push((code, expires));
                        self.code_msg.clear();
                    }
                    Ok(CodeOp::Revoked(code)) => draft.codes.retain(|c| c.0 != code),
                    Ok(CodeOp::Done(msg)) => self.code_msg = msg,
                    Err(e) => self.code_msg = e,
                }
                // Codes are kept right away, also without "Save".
                self.cfg.codes = draft.codes.clone();
                self.cfg.save();
            }
            if self.code_rx.is_some() {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
            ui.add_space(6.0);
            save = ui.button("Save").clicked();
        });
        if save {
            let before = self.cfg.clone();
            self.cfg = self.editing.take().unwrap_or_default();
            if self.cfg.autostart != before.autostart {
                if let Err(e) = desktop::set_autostart(self.cfg.autostart) {
                    self.status = e;
                    self.cfg.autostart = before.autostart;
                }
            }
            self.updater.set_options(self.cfg.update_notify, self.cfg.auto_update && Updater::can_install());
            self.cfg.save();
            self.uploader.set_config(self.cfg.clone());
            self.rescan();
        } else if !open {
            self.editing = None;
        }
    }
}

impl App {
    /// Update notice above everything (list and viewer); the tray tooltip says it too.
    fn update_notice(&mut self, ctx: &egui::Context) {
        let state = self.updater.state();
        if self.tray_state.as_ref() != Some(&state) {
            desktop::set_tooltip(&match &state {
                update::State::Available(v, _) => format!("LiveReplay: version {v} is available"),
                update::State::Ready(v) => format!("LiveReplay: version {v} installed, used from the next start"),
                _ => "LiveReplay".to_string(),
            });
            self.tray_state = Some(state.clone());
        }
        if state == update::State::Idle || self.update_dismissed.as_ref() == Some(&state) {
            return;
        }
        let blue = Color32::from_rgb(90, 160, 255);
        egui::TopBottomPanel::top("update_notice").show(ctx, |ui| {
            ui.horizontal(|ui| {
                match &state {
                    update::State::Available(v, page) => {
                        ui.label(RichText::new(format!("LiveReplay {v} is available.")).color(blue).strong());
                        if Updater::can_install() && ui.button("Install").clicked() {
                            self.updater.install_in_background();
                        }
                        ui.hyperlink_to("What is new", page);
                    }
                    update::State::Downloading(v) => {
                        ui.spinner();
                        ui.label(format!("Downloading LiveReplay {v}..."));
                    }
                    update::State::Ready(v) => {
                        ui.label(RichText::new(format!("LiveReplay {v} is installed.")).color(blue).strong());
                        if ui.button("Restart now").clicked() {
                            update::restart(false);
                        }
                        ui.label(RichText::new("(or it is used from the next start)").color(Color32::from_gray(150)));
                    }
                    update::State::Failed(msg) => {
                        ui.label(RichText::new(msg).color(Color32::from_rgb(255, 120, 110)));
                        ui.hyperlink_to("Download page", "https://github.com/Windows200000/LiveReplay/releases/latest");
                    }
                    update::State::Idle => {}
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("\u{d7}").on_hover_text("Hide this notice").clicked() {
                        self.update_dismissed = Some(state.clone());
                    }
                });
            });
        });
        if matches!(state, update::State::Downloading(_)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Close button: into the tray (unless turned off in Settings).
        if ctx.input(|i| i.viewport().close_requested()) && self.cfg.close_to_tray && desktop::tray_available() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_next = true;
        }
        if self.tray_in_frames > 0 {
            self.tray_in_frames -= 1;
            self.hide_next = self.tray_in_frames == 0;
            if !self.hide_next {
                ctx.request_repaint();
                return;
            }
        }
        if self.hide_next {
            self.hide_next = false;
            desktop::hide(ctx);
            return;
        }
        if desktop::hidden() {
            // Nothing is drawn and no redraw is asked for: the window costs nothing until shown.
            return;
        }
        self.update_notice(ctx);
        if self.first_frame {
            self.first_frame = false;
            crate::viewer::theme::apply(ctx);
            crate::viewer::player::apply_ui_scale(ctx, self.ui_scale);
            self.entries = library::cached();
            self.sort_entries();
            self.rescan();
        }

        // A .wotreplay dropped anywhere opens in the viewer.
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).find(|p| p.extension().is_some_and(|e| e == "wotreplay"))) {
            self.open_viewer(path);
        }

        if let Some(viewer) = &mut self.viewer {
            let mut back = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            egui::TopBottomPanel::top("viewer_nav").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    back |= ui.button("\u{2B05} Replays").clicked();
                    ui.label(&viewer.title);
                });
            });
            viewer.ui(ctx);
            if self.engine.is_none() {
                self.engine = viewer.engine.clone();
            }
            if back {
                self.viewer = None;
                // The viewer may have changed the UI scale.
                self.ui_scale = crate::viewer::settings::ViewerSettings::load().ui_scale;
            }
            return;
        }

        // Ctrl + wheel / Ctrl +/- on the replay list too.
        if crate::viewer::player::ui_scale_input(ctx, &mut self.ui_scale) {
            let mut vs = crate::viewer::settings::ViewerSettings::load();
            vs.ui_scale = self.ui_scale;
            vs.save();
        }
        crate::viewer::player::apply_ui_scale(ctx, self.ui_scale);
        self.library_page(ctx);
        self.settings_window(ctx);
        self.consent_window(ctx);
    }
}
