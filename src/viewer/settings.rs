use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HpText {
    Off,
    Current,
    Max,
    CurrentOfMax,
}

/// Marker / overlay options; the 2D map and the 3D view each have their own set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSettings {
    /// Labels next to markers (player name above tank name when both are on).
    pub label_player: bool,
    pub label_tank: bool,
    pub hp_ring: bool,
    /// HP shown above the marker.
    pub hp_text: HpText,
    pub show_last_known: bool,
    pub show_grid: bool,
    pub show_bases: bool,
    /// Your 564 m draw-distance circle.
    pub range_circle: bool,
    pub show_shots: bool,
    /// Marker size relative to the in-game proportion (1.0 = like the game).
    pub marker_scale: f32,
    pub map_brightness: f32,
    /// 3D: textured ground and buildings; off = plain grey (water faint blue).
    pub textures: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            label_player: false,
            label_tank: true,
            hp_ring: true,
            hp_text: HpText::Off,
            show_last_known: true,
            show_grid: true,
            show_bases: true,
            range_circle: true,
            show_shots: true,
            marker_scale: 1.0,
            map_brightness: 0.85,
            textures: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewerSettings {
    /// 2D minimap (also used for the corner minimap in 3D).
    pub view2d: ViewSettings,
    pub view3d: ViewSettings,
    /// Scale of all other UI (panels, buttons, text).
    pub ui_scale: f32,
    pub speed: f32,
    /// 3D view on/off. Off = nothing 3D is loaded or rendered.
    pub view_3d: bool,
    /// Minimap in the bottom-right corner of the 3D view.
    pub corner_minimap: bool,
}

impl Default for ViewerSettings {
    fn default() -> Self {
        Self {
            view2d: ViewSettings::default(),
            view3d: ViewSettings { show_grid: false, map_brightness: 1.0, ..ViewSettings::default() },
            ui_scale: 1.0,
            speed: 1.0,
            view_3d: false,
            corner_minimap: true,
        }
    }
}

impl ViewerSettings {
    /// Settings of the view currently shown.
    pub fn active(&self) -> &ViewSettings {
        if self.view_3d {
            &self.view3d
        } else {
            &self.view2d
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn path() -> Option<std::path::PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("viewer.json"))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_raw() -> Option<String> {
    std::fs::read_to_string(path()?).ok()
}
#[cfg(not(target_arch = "wasm32"))]
fn write_raw(s: &str) {
    if let Some(p) = path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, s);
    }
}

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}
#[cfg(target_arch = "wasm32")]
fn read_raw() -> Option<String> {
    storage()?.get_item("viewer_settings").ok()?
}
#[cfg(target_arch = "wasm32")]
fn write_raw(s: &str) {
    if let Some(st) = storage() {
        let _ = st.set_item("viewer_settings", s);
    }
}

impl ViewerSettings {
    pub fn load() -> Self {
        read_raw().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            write_raw(&s);
        }
    }
}
