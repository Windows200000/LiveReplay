//! Data exchanged with the LiveReplay server (see the `livereplay-api` crate for the endpoints).
//! Battles travel in the frame format of [`crate::replay::wire`]; the game data the web viewer
//! needs (map definition + minimap textures, stat icons) travels as packs made by the desktop
//! client from its own game files and is never part of the repository.

use serde::{Deserialize, Serialize};

pub use livereplay_api as api;

use crate::gamedata::{dds::Image, ArenaDef};

/// Where the desktop client uploads to and the web viewer lives. `LIVEREPLAY_SERVER` overrides it.
pub const SERVER_URL: &str = "https://wot.arne.my";

/// Everything the 2D view of one map needs.
#[derive(Serialize, Deserialize)]
pub struct MapPack {
    pub arena: ArenaDef,
    /// The minimap texture as stored in the game (DDS, GPU-compressed).
    pub minimap: Vec<u8>,
    /// Random-event overlays (path, DDS).
    pub layers: Vec<(String, Vec<u8>)>,
}

/// Stat icons (index into `Icon::ALL`, image).
#[derive(Serialize, Deserialize, Default)]
pub struct IconPack {
    pub icons: Vec<(u8, u32, u32, Vec<u8>)>,
}

impl IconPack {
    pub fn get(&self, icon: crate::viewer::theme::Icon) -> Option<Image> {
        let i = crate::viewer::theme::Icon::ALL.iter().position(|x| *x == icon)? as u8;
        self.icons.iter().find(|x| x.0 == i).map(|x| Image { width: x.1 as usize, height: x.2 as usize, rgba: x.3.clone() })
    }
}

pub fn encode_pack<T: Serialize>(v: &T) -> Vec<u8> {
    crate::replay::wire::pack(v)
}

pub fn decode_pack<T: for<'de> Deserialize<'de>>(b: &[u8]) -> Option<T> {
    crate::replay::wire::unpack(b).ok()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn map_pack(game: &crate::gamedata::GameData, map: &str) -> Option<MapPack> {
    let arena = game.arena(map)?;
    let minimap = game.minimap_raw(&arena)?;
    let layers = game.minimap_event_layers_raw(&arena);
    Some(MapPack { arena, minimap, layers })
}

#[cfg(not(target_arch = "wasm32"))]
pub fn icon_pack(game: &crate::gamedata::GameData) -> IconPack {
    let icons = crate::viewer::theme::Icon::ALL
        .iter()
        .enumerate()
        .filter_map(|(i, ic)| {
            let img = game.gui_icon(ic.game_path()?)?;
            Some((i as u8, img.width as u32, img.height as u32, img.rgba))
        })
        .collect();
    IconPack { icons }
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data).iter().map(|x| format!("{x:02x}")).collect()
}
