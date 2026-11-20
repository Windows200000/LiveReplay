//! LiveReplay in the browser: the desktop viewer compiled to WebAssembly, fed from the server.
//! Pages: `/` battle list (+ live battles unlocked by viewing codes), `/b/<id>` viewer
//! (`?t=<seconds after battle start>` opens at that moment).

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod fetch;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    wasm_bindgen_futures::spawn_local(async {
        use wasm_bindgen::JsCast;
        let options = eframe::WebOptions::default();
        let Some(canvas) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("livereplay"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        else {
            return;
        };
        let r = eframe::WebRunner::new().start(canvas, options, Box::new(|cc| Ok(Box::new(app::WebApp::new(cc))))).await;
        if let Err(e) = r {
            web_sys::console::error_1(&e);
        }
    });
}
