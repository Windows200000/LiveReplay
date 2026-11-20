#[cfg(not(target_arch = "wasm32"))]
pub mod app;
#[cfg(not(target_arch = "wasm32"))]
pub mod dirs;
pub mod gamedata;
pub mod net;
pub mod replay;
pub mod viewer;
