//! The web app: battle list, live battles (viewing codes), and the viewer.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use egui::{Color32, RichText};
use livereplay::gamedata::dds;
use livereplay::net::{decode_pack, IconPack, MapPack};
use livereplay::replay::battle::Battle;
use livereplay::replay::wire;
use livereplay::viewer::minimap2d::Minimap2D;
use livereplay::viewer::player::{Assets, Load3D, Loaded, Msg, ReplayViewer, Request3D};
use livereplay::viewer::view3d::Scene3D;
use livereplay_api::*;

use crate::fetch::{self, Reply};

const CODES_KEY: &str = "livereplay.codes";
/// Live battles: how often new data is fetched, and the live list refreshed.
const POLL_SECS: f64 = 3.0;
const LIVE_LIST_SECS: f64 = 15.0;

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn location() -> (String, String) {
    let l = web_sys::window().map(|w| w.location());
    let path = l.as_ref().and_then(|l| l.pathname().ok()).unwrap_or_default();
    let search = l.as_ref().and_then(|l| l.search().ok()).unwrap_or_default();
    (path, search)
}

fn go(path: &str) {
    if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
    }
}

fn origin() -> String {
    web_sys::window().and_then(|w| w.location().origin().ok()).unwrap_or_default()
}

fn pretty_code(c: &str) -> String {
    if c.len() > 4 {
        format!("{}-{}", &c[..c.len() / 2], &c[c.len() / 2..])
    } else {
        c.to_string()
    }
}

/// Game files the viewer needs, from the server.
struct WebAssets {
    icons: Option<Arc<IconPack>>,
    /// Map definition and minimap picture (the 3D view's corner minimap).
    map: Arc<(livereplay::gamedata::ArenaDef, Option<dds::Image>)>,
    ctx: egui::Context,
}

impl Assets for WebAssets {
    /// 3D packs from the server (uploaded by players' desktop apps after their battles). If some
    /// are being uploaded right now (or an app announced them), waits and reports the progress;
    /// if the map's own data is simply not there, says so (the viewer goes back to 2D). Stops
    /// when the viewer no longer waits (the user went back to 2D).
    fn request_3d(&self, req: Request3D, tx: Sender<Load3D>) {
        use livereplay::gamedata::webpack as wp;
        let (map, ctx) = (self.map.clone(), self.ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            async fn get(name: &str) -> Option<Vec<u8>> {
                fetch::get_async(&format!("/api/assets/{name}"), "").await.ok().filter(|r| r.ok()).map(|r| r.body)
            }
            async fn sleep(ms: i32) {
                let p = js_sys::Promise::new(&mut |resolve, _| {
                    if let Some(w) = web_sys::window() {
                        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
                    }
                });
                let _ = wasm_bindgen_futures::JsFuture::from(p).await;
            }
            let (terrain_name, scene_name) = (wp::terrain_asset(&req.map), wp::scene_asset(&req.map, req.gameplay_id));
            let mut names = vec![terrain_name.clone(), scene_name.clone()];
            names.extend(req.vehicle_types.iter().map(|vt| wp::tank_asset(vt)));
            names.extend(req.prefabs.iter().map(|pf| wp::prefab_asset(pf)));
            let mut got: HashMap<String, Vec<u8>> = HashMap::new();
            loop {
                // The map's own data first; tanks and objects only once it is there.
                for n in &names[..2] {
                    if !got.contains_key(n) {
                        if let Some(b) = get(n).await {
                            got.insert(n.clone(), b);
                        }
                    }
                }
                let map_there = got.contains_key(&terrain_name) && got.contains_key(&scene_name);
                for n in names[2..].iter().filter(|_| map_there) {
                    if !got.contains_key(n) {
                        if let Some(b) = get(n).await {
                            got.insert(n.clone(), b);
                        }
                    }
                }
                let wanted = if map_there { &names[..] } else { &names[..2] };
                let missing: Vec<&String> = wanted.iter().filter(|n| !got.contains_key(*n)).collect();
                if missing.is_empty() && map_there {
                    break;
                }
                let list = missing.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(",");
                let status: HashMap<String, AssetStatus> = fetch::get_async(&format!("/api/asset-status?names={list}"), "")
                    .await
                    .ok()
                    .filter(|r| r.ok())
                    .and_then(|r| serde_json::from_slice(&r.body).ok())
                    .unwrap_or_default();
                let coming: Vec<&AssetStatus> = missing.iter().filter_map(|n| status.get(*n)).filter(|s| s.state == "uploading" || s.state == "expected").collect();
                if coming.is_empty() {
                    if !got.contains_key(&terrain_name) || !got.contains_key(&scene_name) {
                        let _ = tx.send(Load3D::Missing(
                            "There is no 3D data for this map on the server yet. It is added when someone with the LiveReplay app \
                             uploads a battle on this map. Showing the map in 2D."
                                .into(),
                        ));
                        ctx.request_repaint();
                        return;
                    }
                    // Only some tanks or map-event objects are missing and nobody is sending them.
                    break;
                }
                let have: u64 = got.values().map(|b| b.len() as u64).sum();
                let (received, total) = coming.iter().fold((0, 0), |(r, t), s| (r + s.received, t + s.total));
                let text = if received == 0 && coming.iter().all(|s| s.state == "expected") {
                    "The 3D data for this map is about to be uploaded by a player's LiveReplay app. Waiting...".to_string()
                } else {
                    format!("The 3D data for this map is being uploaded right now: {}%", (have + received) * 100 / (have + total).max(1))
                };
                if tx.send(Load3D::Progress(text)).is_err() {
                    return;
                }
                ctx.request_repaint();
                sleep(2000).await;
            }
            let terrain = got.get(&terrain_name).and_then(|b| wp::decode_terrain_pack(b));
            let scene = got.get(&scene_name).and_then(|b| wp::decode_scene_pack(b));
            let (Some(terrain), Some(scene)) = (terrain, scene) else {
                let _ = tx.send(Load3D::Failed("The 3D data for this map on the server is unreadable".into()));
                ctx.request_repaint();
                return;
            };
            let mut tanks = HashMap::new();
            for vt in &req.vehicle_types {
                if let Some(t) = got.get(&wp::tank_asset(vt)).and_then(|b| wp::decode_tank_pack(b)) {
                    tanks.insert(vt.clone(), t);
                }
            }
            let mut prefabs = HashMap::new();
            for pf in &req.prefabs {
                if let Some(m) = got.get(&wp::prefab_asset(pf)).and_then(|b| wp::decode_mesh_pack(b)) {
                    prefabs.insert(pf.clone(), m);
                }
            }
            let _ = tx.send(Load3D::Ready(Scene3D { arena: map.0.clone(), terrain, minimap: map.1.clone(), map: scene, tanks, prefabs }));
            ctx.request_repaint();
        });
    }
    fn icon(&self, icon: livereplay::viewer::theme::Icon) -> Option<dds::Image> {
        self.icons.as_ref()?.get(icon)
    }
    fn crit_icon(&self, kind: &str, state: u8) -> Option<dds::Image> {
        self.icons.as_ref()?.crit(kind, state)
    }
}

enum Ev {
    List(Result<BattleList, String>),
    Live(Result<LiveList, String>),
    Icons(Option<IconPack>),
    /// For the open battle with this serial number.
    Info(u64, Result<BattleInfo, String>),
    Map(u64, Result<MapPack, String>),
    Data(u64, Result<Reply, String>),
    /// Other uploads of the same battle.
    Group(u64, Result<BattleList, String>),
    /// Data of another player's upload of the same battle (battle id).
    Other(u64, String, Result<Reply, String>),
}

/// A battle being watched: loads info, map and data, then keeps a live battle up to date.
struct Open {
    id: String,
    serial: u64,
    viewer: ReplayViewer,
    vtx: Sender<Msg>,
    _alive: Arc<()>,
    info: Option<BattleInfo>,
    map: Option<MapPack>,
    battle: Option<Battle>,
    data_done: bool,
    offset: u64,
    gen: u32,
    live: bool,
    loaded: bool,
    polling: bool,
    next_poll: f64,
    error: Option<String>,
    /// Other players' uploads of this battle, which of them are merged in, and their data.
    members: Vec<BattleInfo>,
    shown: Vec<String>,
    others: HashMap<String, Battle>,
    /// Per other viewpoint: (generation, bytes read), for fetching only what is new while live.
    other_pos: HashMap<String, (u32, u64)>,
    other_polling: std::collections::HashSet<String>,
    /// While live: look again for players who recorded this battle too.
    next_group: f64,
}

impl Open {
    /// The battle with the chosen other viewpoints merged in.
    fn merged(&self) -> Option<Battle> {
        let mut b = self.battle.clone()?;
        for id in &self.shown {
            if let Some(o) = self.others.get(id) {
                livereplay::replay::merge::add_viewpoint(&mut b, o.clone());
            }
        }
        Some(b)
    }

    fn push_update(&mut self) {
        if self.loaded {
            if let Some(b) = self.merged() {
                let _ = self.vtx.send(Msg::Update(b));
            }
        }
        let vp = if self.shown.is_empty() { String::new() } else { format!("?vp={}", self.shown.join(",")) };
        self.viewer.share_base = Some(format!("{}/b/{}{vp}", origin(), self.id));
    }
}

pub struct WebApp {
    tx: Sender<Ev>,
    rx: Receiver<Ev>,
    codes: Vec<String>,
    code_names: HashMap<String, String>,
    code_input: String,
    code_msg: String,
    live: Vec<BattleInfo>,
    live_loading: bool,
    next_live: f64,
    list: Option<BattleList>,
    list_loading: bool,
    list_error: Option<String>,
    player: String,
    page: usize,
    /// Player filter edited: reload when typing pauses.
    filter_changed_at: Option<f64>,
    icons: Option<Arc<IconPack>>,
    icons_state: u8, // 0 = not asked, 1 = loading, 2 = done
    open: Option<Open>,
    serial: u64,
    route: String,
    ctx: egui::Context,
}

impl WebApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        livereplay::viewer::theme::apply(&cc.egui_ctx);
        livereplay::viewer::player::apply_ui_scale(&cc.egui_ctx, livereplay::viewer::settings::ViewerSettings::load().ui_scale);
        let (tx, rx) = channel();
        let codes = storage()
            .and_then(|s| s.get_item(CODES_KEY).ok().flatten())
            .map(|v| v.split(',').map(normalize_code).filter(|c| !c.is_empty()).collect())
            .unwrap_or_default();
        Self {
            tx,
            rx,
            codes,
            code_names: HashMap::new(),
            code_input: String::new(),
            code_msg: String::new(),
            live: Vec::new(),
            live_loading: false,
            next_live: 0.0,
            list: None,
            list_loading: false,
            list_error: None,
            player: String::new(),
            page: 0,
            filter_changed_at: None,
            icons: None,
            icons_state: 0,
            open: None,
            serial: 0,
            route: String::new(),
            ctx: cc.egui_ctx.clone(),
        }
    }

    fn codes_header(&self) -> String {
        self.codes.join(",")
    }

    fn save_codes(&self) {
        if let Some(s) = storage() {
            let _ = s.set_item(CODES_KEY, &self.codes.join(","));
        }
    }

    fn load_list(&mut self, ctx: &egui::Context) {
        self.list_loading = true;
        let url = format!("/api/battles?page={}&player={}", self.page, js_sys::encode_uri_component(&self.player));
        fetch::get(ctx, url, String::new(), self.tx.clone(), |r| {
            Ev::List(r.and_then(|r| if r.ok() { serde_json::from_slice(&r.body).map_err(|e| e.to_string()) } else { Err(r.text()) }))
        });
    }

    fn load_live(&mut self, ctx: &egui::Context) {
        self.live_loading = true;
        fetch::get(ctx, "/api/live".into(), self.codes_header(), self.tx.clone(), |r| {
            Ev::Live(r.and_then(|r| if r.ok() { serde_json::from_slice(&r.body).map_err(|e| e.to_string()) } else { Err(r.text()) }))
        });
    }

    /// Open a battle; `query` is a share link's options (`t=..&v=3d&...`).
    fn open_battle(&mut self, ctx: &egui::Context, id: &str, query: &str) {
        self.serial += 1;
        let serial = self.serial;
        let (vtx, vrx) = channel();
        let alive = Arc::new(());
        let mut viewer = ReplayViewer::remote(String::new(), vrx, alive.clone(), false);
        viewer.share_base = Some(format!("{}/b/{id}", origin()));
        viewer.apply_link(query);
        self.open = Some(Open {
            id: id.to_string(),
            serial,
            viewer,
            vtx,
            _alive: alive,
            info: None,
            map: None,
            battle: None,
            data_done: false,
            offset: 0,
            gen: 0,
            live: false,
            loaded: false,
            polling: true,
            next_poll: 0.0,
            error: None,
            members: Vec::new(),
            shown: query
                .trim_start_matches('?')
                .split('&')
                .find_map(|kv| kv.strip_prefix("vp="))
                .map(|v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect())
                .unwrap_or_default(),
            others: HashMap::new(),
            other_pos: HashMap::new(),
            other_polling: Default::default(),
            next_group: 0.0,
        });
        for other in self.open.as_ref().map(|o| o.shown.clone()).unwrap_or_default() {
            self.fetch_other(ctx, &other);
        }
        if let Some(o) = self.open.as_mut() {
            o.push_update();
        }
        let codes = self.codes_header();
        fetch::get(ctx, format!("/api/battles/{id}"), codes.clone(), self.tx.clone(), move |r| {
            Ev::Info(serial, r.and_then(|r| if r.ok() { serde_json::from_slice(&r.body).map_err(|e| e.to_string()) } else { Err(r.text()) }))
        });
        fetch::get(ctx, format!("/api/battles/{id}/data?gen=0&from=0"), codes, self.tx.clone(), move |r| Ev::Data(serial, r));
        if self.icons_state == 0 {
            self.icons_state = 1;
            fetch::get(ctx, format!("/api/assets/{}", livereplay::net::ICONS_ASSET), String::new(), self.tx.clone(), |r| {
                Ev::Icons(r.ok().filter(|r| r.ok()).and_then(|r| decode_pack(&r.body)))
            });
        }
    }

    fn fetch_other(&self, ctx: &egui::Context, id: &str) {
        let Some(o) = &self.open else { return };
        let (serial, other) = (o.serial, id.to_string());
        fetch::get(ctx, format!("/api/battles/{id}/data?gen=0&from=0"), self.codes_header(), self.tx.clone(), move |r| Ev::Other(serial, other, r));
    }

    /// Follow the address bar (also the browser's back/forward buttons).
    fn sync_route(&mut self, ctx: &egui::Context) {
        let (path, search) = location();
        if path == self.route {
            return;
        }
        self.route = path.clone();
        if let Some(id) = path.strip_prefix("/b/").map(|s| s.trim_matches('/').to_string()).filter(|s| !s.is_empty()) {
            if self.open.as_ref().map(|o| o.id.as_str()) != Some(id.as_str()) {
                self.open_battle(ctx, &id, &search);
            }
        } else {
            self.open = None;
            if self.list.is_none() && !self.list_loading {
                self.load_list(ctx);
            }
            self.next_live = 0.0;
        }
    }

    fn handle(&mut self, ctx: &egui::Context, ev: Ev) {
        let codes = self.codes_header();
        match ev {
            Ev::List(r) => {
                self.list_loading = false;
                match r {
                    Ok(l) => {
                        self.list = Some(l);
                        self.list_error = None;
                    }
                    Err(e) => self.list_error = Some(e),
                }
            }
            Ev::Live(r) => {
                self.live_loading = false;
                if let Ok(l) = r {
                    // Forget expired / revoked codes.
                    self.codes.retain(|c| l.valid.iter().any(|v| v.code == *c));
                    self.save_codes();
                    self.code_names = l.valid.iter().map(|v| (v.code.clone(), v.player.clone())).collect();
                    self.live = l.battles;
                }
            }
            Ev::Icons(p) => {
                self.icons = p.map(Arc::new);
                self.icons_state = 2;
            }
            Ev::Info(s, r) => {
                let Some(o) = self.open.as_mut().filter(|o| o.serial == s) else { return };
                match r {
                    Ok(info) => {
                        o.live = info.live;
                        o.viewer.title = format!("{} · {} · {}", info.player, info.tank, info.map_display);
                        let serial = o.serial;
                        fetch::get(ctx, format!("/api/assets/{}", map_asset(&info.map)), String::new(), self.tx.clone(), move |r| {
                            Ev::Map(serial, match r {
                                Ok(r) if r.ok() => decode_pack(&r.body).ok_or_else(|| "map data unreadable".to_string()),
                                Ok(r) => Err(format!("map data not on the server ({})", r.status)),
                                Err(e) => Err(e),
                            })
                        });
                        if !info.group.is_empty() {
                            fetch::get(ctx, format!("/api/battles?group={}", info.group), codes, self.tx.clone(), move |r| {
                                Ev::Group(serial, r.and_then(|r| if r.ok() { serde_json::from_slice(&r.body).map_err(|e| e.to_string()) } else { Err(r.text()) }))
                            });
                        }
                        o.info = Some(info);
                    }
                    Err(e) => o.error = Some(if e.contains("not found") { "Battle not found (live battles need a viewing code)".into() } else { e }),
                }
            }
            Ev::Group(s, r) => {
                let Some(o) = self.open.as_mut().filter(|o| o.serial == s) else { return };
                if let Ok(l) = r {
                    o.members = l.battles.into_iter().filter(|b| b.id != o.id).collect();
                }
            }
            Ev::Other(s, id, r) => {
                let Some(o) = self.open.as_mut().filter(|o| o.serial == s) else { return };
                o.other_polling.remove(&id);
                let Some(r) = r.ok().filter(|r| r.ok()) else { return };
                let gen = r.header(GEN_HEADER).and_then(|g| g.parse().ok()).unwrap_or(0);
                let (old_gen, offset) = o.other_pos.get(&id).copied().unwrap_or((0, 0));
                if gen != old_gen || !o.others.contains_key(&id) {
                    if let Some(b) = wire::decode_battle(&r.body) {
                        o.others.insert(id.clone(), b);
                    }
                    o.other_pos.insert(id, (gen, r.body.len() as u64));
                } else {
                    let (chunks, used) = wire::decode_frames(&r.body);
                    let mut b = o.others.remove(&id);
                    for c in chunks {
                        b = Some(wire::apply(b, c));
                    }
                    if let Some(b) = b {
                        o.others.insert(id.clone(), b);
                    }
                    o.other_pos.insert(id, (gen, offset + used as u64));
                }
                o.push_update();
            }
            Ev::Map(s, r) => {
                let Some(o) = self.open.as_mut().filter(|o| o.serial == s) else { return };
                match r {
                    Ok(m) => o.map = Some(m),
                    Err(e) => o.error = Some(e),
                }
            }
            Ev::Data(s, r) => {
                let Some(o) = self.open.as_mut().filter(|o| o.serial == s) else { return };
                o.polling = false;
                o.next_poll = ctx.input(|i| i.time) + POLL_SECS;
                let r = match r {
                    Ok(r) if r.ok() => r,
                    Ok(r) => {
                        o.error = Some(r.text());
                        return;
                    }
                    Err(e) => {
                        // Network hiccup: try again at the next poll.
                        if !o.data_done {
                            o.error = Some(e);
                        }
                        return;
                    }
                };
                let gen = r.header(GEN_HEADER).and_then(|g| g.parse().ok()).unwrap_or(0);
                let finished = r.header("X-Livereplay-Finished").as_deref() == Some("1");
                if gen != o.gen {
                    // First load, or the uploader replaced the battle (e.g. the final replay).
                    match wire::decode_battle_checked(&r.body) {
                        Ok(b) => o.battle = Some(b),
                        Err(e) if !r.body.is_empty() => o.error = Some(format!("Can't read this battle: {e}")),
                        Err(_) => o.battle = None,
                    }
                    o.offset = r.body.len() as u64;
                    o.gen = gen;
                } else {
                    let (chunks, used) = wire::decode_frames(&r.body);
                    for c in chunks {
                        o.battle = Some(wire::apply(o.battle.take(), c));
                    }
                    o.offset += used as u64;
                }
                o.data_done = true;
                let was_live = o.live;
                o.live = !finished;
                if o.loaded {
                    o.push_update();
                    if was_live && finished {
                        let _ = o.vtx.send(Msg::Finished);
                    }
                }
            }
        }
    }

    /// Hand the battle to the viewer once everything is there.
    fn start_viewer(&mut self) {
        let icons = self.icons.clone();
        let icons_ready = self.icons_state == 2;
        let Some(o) = self.open.as_mut() else { return };
        if o.loaded || !icons_ready || o.map.is_none() || o.battle.is_none() {
            return;
        }
        let (Some(map), Some(mut battle)) = (o.map.as_ref(), o.merged()) else { return };
        if battle.vehicles.is_empty() {
            return;
        }
        battle.live = o.live;
        let gameplay = map.arena.gameplay(&battle.gameplay);
        let image = dds::decode(&map.minimap);
        let map3d = Arc::new((map.arena.clone(), image.clone()));
        let layers = map.layers.iter().filter_map(|(p, b)| Some((p.clone(), dds::decode(b)?))).collect();
        let view = Minimap2D::new(map.arena.clone(), gameplay, image).with_event_layers(layers);
        o.viewer.set_live(o.live);
        let _ = o.vtx.send(Msg::Loaded(Loaded::new(battle, Box::new(view)), Arc::new(WebAssets { icons, map: map3d, ctx: self.ctx.clone() })));
        o.loaded = true;
    }

    fn battles_page(&mut self, ctx: &egui::Context) {
        let t = ctx.input(|i| i.time);
        if t >= self.next_live && !self.live_loading && !self.codes.is_empty() {
            self.next_live = t + LIVE_LIST_SECS;
            self.load_live(ctx);
        }
        if let Some(at) = self.filter_changed_at {
            if t - at > 0.4 {
                self.filter_changed_at = None;
                self.page = 0;
                self.load_list(ctx);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        if !self.codes.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
        let mut open: Option<String> = None;
        egui::TopBottomPanel::top("web_top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("LiveReplay").strong());
                ui.label(RichText::new("World of Tanks battles on the minimap").color(Color32::from_gray(150)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.hyperlink_to("Impressum", "/impressum");
                    ui.hyperlink_to("Privacy", "/privacy");
                    ui.hyperlink_to("Licences", "https://github.com/Windows200000/LiveReplay/blob/main/THIRD_PARTY_LICENSES.md");
                    ui.add_space(8.0);
                    ui.hyperlink_to(RichText::new("Download the app").strong(), "https://github.com/Windows200000/LiveReplay/releases/latest")
                        .on_hover_text("LiveReplay for Windows, Linux and macOS: watch your replays and share your battles live");
                });
            });
            ui.add_space(6.0);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("Live battles").size(17.0).strong());
                ui.horizontal_wrapped(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.code_input).hint_text("Viewing code from a player").desired_width(200.0));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("Add").clicked() || enter) && !self.code_input.trim().is_empty() {
                        let c = normalize_code(&self.code_input);
                        if !self.codes.contains(&c) {
                            self.codes.push(c);
                        }
                        self.code_input.clear();
                        // Only codes the server knows are kept (checked on the next refresh).
                        self.code_msg = "Checking code...".into();
                        self.save_codes();
                        self.next_live = 0.0;
                    }
                    for c in self.codes.clone() {
                        let name = self.code_names.get(&c).cloned().unwrap_or_default();
                        let label = if name.is_empty() { pretty_code(&c) } else { format!("{name} ({})", pretty_code(&c)) };
                        if ui.button(format!("{label}  \u{1F5D9}")).on_hover_text("Forget this code").clicked() {
                            self.codes.retain(|x| *x != c);
                            self.save_codes();
                        }
                    }
                });
                if !self.code_names.is_empty() || self.codes.is_empty() {
                    self.code_msg.clear();
                }
                if !self.code_msg.is_empty() {
                    ui.label(RichText::new(&self.code_msg).color(Color32::from_gray(150)));
                }
                if self.live.is_empty() {
                    let hint = if self.codes.is_empty() {
                        "To watch someone's battles while they play, ask them for a viewing code (LiveReplay app > Settings)."
                    } else {
                        "None of your players is in a battle right now."
                    };
                    ui.label(RichText::new(hint).color(Color32::from_gray(150)));
                }
                for b in &self.live {
                    let text = format!("LIVE   {}   {}   {}   {}", names(b), b.tank, b.map_display, b.mode);
                    if ui.add(egui::Button::new(RichText::new(text).color(Color32::from_rgb(255, 120, 110)))).clicked() {
                        open = Some(b.id.clone());
                    }
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Battles").size(17.0).strong());
                    ui.add_space(12.0);
                    if ui.add(egui::TextEdit::singleline(&mut self.player).hint_text("Player").desired_width(160.0)).changed() {
                        self.filter_changed_at = Some(ui.input(|i| i.time));
                    }
                    if self.list_loading {
                        ui.spinner();
                    }
                });
                if let Some(e) = &self.list_error {
                    ui.colored_label(Color32::from_rgb(255, 120, 110), e);
                }
                let Some(list) = &self.list else { return };
                if list.battles.is_empty() {
                    ui.label(RichText::new("No battles uploaded yet.").color(Color32::from_gray(150)));
                }
                const COLS: [(&str, f32); 6] = [("Date", 130.0), ("Player", 150.0), ("Tank", 150.0), ("Map", 130.0), ("Mode", 170.0), ("Damage", 70.0)];
                let row_h = 24.0;
                let (head, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
                let mut x = head.left() + 6.0;
                for (name, w) in COLS {
                    ui.painter().text(egui::pos2(x, head.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(13.0), Color32::from_gray(150));
                    x += w;
                }
                for b in &list.battles {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 3.0, Color32::from_white_alpha(14));
                    }
                    let date = b.date.get(..16).unwrap_or(&b.date).to_string();
                    let mode = if b.interrupted { format!("{} (interrupted)", b.mode) } else { b.mode.clone() };
                    let cells = [date, names(b), b.tank.clone(), b.map_display.clone(), mode, b.damage.to_string()];
                    let mut x = rect.left() + 6.0;
                    for ((_, w), c) in COLS.iter().zip(cells) {
                        let clip = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + w - 8.0, rect.bottom()));
                        ui.painter().with_clip_rect(clip).text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, c, egui::FontId::proportional(14.0), Color32::from_gray(225));
                        x += w;
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        open = Some(b.id.clone());
                    }
                }
                let pages = list.total.div_ceil(list.per_page.max(1)).max(1);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.page > 0, egui::Button::new("\u{2039} Newer")).clicked() {
                        self.page -= 1;
                        self.filter_changed_at = Some(-1.0);
                    }
                    ui.label(format!("Page {} of {pages}  ({} battles)", self.page + 1, list.total));
                    if ui.add_enabled(self.page + 1 < pages, egui::Button::new("Older \u{203A}")).clicked() {
                        self.page += 1;
                        self.filter_changed_at = Some(-1.0);
                    }
                });
            });
        });
        if let Some(id) = open {
            let path = format!("/b/{id}");
            go(&path);
            self.route = path;
            self.open_battle(ctx, &id, "");
        }
        // Page buttons reload at once (the filter box waits for typing to pause).
        if self.filter_changed_at == Some(-1.0) {
            self.filter_changed_at = None;
            self.load_list(ctx);
        }
    }

    fn viewer_page(&mut self, ctx: &egui::Context) {
        let t = ctx.input(|i| i.time);
        let codes = self.codes_header();
        let icons_done = self.icons_state == 2;
        let mut back = false;
        let Some(o) = self.open.as_mut() else { return };
        if o.live && o.data_done && !o.polling && t >= o.next_poll {
            o.polling = true;
            let serial = o.serial;
            fetch::get(ctx, format!("/api/battles/{}/data?gen={}&from={}", o.id, o.gen, o.offset), codes.clone(), self.tx.clone(), move |r| Ev::Data(serial, r));
            // The other viewpoints shown: only what is new.
            for id in o.shown.clone() {
                if o.others.contains_key(&id) && o.other_polling.insert(id.clone()) {
                    let (gen, from) = o.other_pos.get(&id).copied().unwrap_or((0, 0));
                    let other = id.clone();
                    fetch::get(ctx, format!("/api/battles/{id}/data?gen={gen}&from={from}"), codes.clone(), self.tx.clone(), move |r| Ev::Other(serial, other, r));
                }
            }
            // Someone else may start uploading this battle while it runs.
            if t >= o.next_group {
                o.next_group = t + 30.0;
                if let Some(group) = o.info.as_ref().map(|i| if i.group.is_empty() { i.id.clone() } else { i.group.clone() }) {
                    fetch::get(ctx, format!("/api/battles?group={group}"), codes.clone(), self.tx.clone(), move |r| {
                        Ev::Group(serial, r.and_then(|r| if r.ok() { serde_json::from_slice(&r.body).map_err(|e| e.to_string()) } else { Err(r.text()) }))
                    });
                }
            }
        }
        if o.live {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        egui::TopBottomPanel::top("viewer_nav").show(ctx, |ui| {
            ui.horizontal(|ui| {
                back = ui.button("\u{2B05} Battles").clicked();
                ui.label(&o.viewer.title);
                if !o.members.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Also recorded by:").color(Color32::from_gray(150)))
                        .on_hover_text("Merge their view of this battle: tanks they saw appear too, with their view range");
                    let mut changed = None;
                    for m in &o.members {
                        let mut on = o.shown.contains(&m.id);
                        if ui.checkbox(&mut on, &m.player).changed() {
                            changed = Some((m.id.clone(), on));
                        }
                    }
                    if let Some((id, on)) = changed {
                        if on {
                            o.shown.push(id.clone());
                            if !o.others.contains_key(&id) {
                                let serial = o.serial;
                                let other = id.clone();
                                fetch::get(ctx, format!("/api/battles/{id}/data?gen=0&from=0"), codes.clone(), self.tx.clone(), move |r| Ev::Other(serial, other, r));
                            }
                        } else {
                            o.shown.retain(|x| *x != id);
                        }
                        o.push_update();
                    }
                }
                if let Some(link) = o.viewer.share_link() {
                    if ui.button("Copy link").on_hover_text("Link to this moment and view (also: right-click the map)").clicked() {
                        ui.ctx().copy_text(link);
                    }
                }
            });
        });
        if let Some(e) = &o.error {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(e).color(Color32::from_rgb(255, 140, 120))));
            });
        } else if !o.loaded {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.label("Loading battle...");
                    let step = |done: bool, what: &str| if done { String::new() } else { format!(" {what}") };
                    let waiting = format!(
                        "{}{}{}{}",
                        step(o.info.is_some(), "details"),
                        step(o.map.is_some(), "map"),
                        step(o.battle.is_some(), "battle data"),
                        step(icons_done, "icons")
                    );
                    if !waiting.is_empty() {
                        ui.label(RichText::new(format!("waiting for:{waiting}")).small().color(Color32::from_gray(130)));
                    }
                });
            });
        } else {
            o.viewer.ui(ctx);
        }
        if back {
            go("/");
            self.route = "/".into();
            self.open = None;
            self.next_live = 0.0;
            if self.list.is_none() {
                self.load_list(ctx);
            }
        }
    }
}

impl eframe::App for WebApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_route(ctx);
        while let Ok(ev) = self.rx.try_recv() {
            self.handle(ctx, ev);
        }
        self.start_viewer();
        if self.open.is_some() {
            self.viewer_page(ctx);
        } else {
            self.battles_page(ctx);
        }
    }
}

/// The player(s) of a battle in a list: everyone who uploaded it.
fn names(b: &BattleInfo) -> String {
    if b.uploaders.len() > 1 {
        b.uploaders.join(", ")
    } else {
        b.player.clone()
    }
}
