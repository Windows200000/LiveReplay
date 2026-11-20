//! Minimal HTTP GET through the browser's fetch (no extra crates).

use std::sync::mpsc::Sender;

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    headers: web_sys::Headers,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).ok().flatten()
    }
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub async fn get_async(url: &str, codes: &str) -> Result<Reply, String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let opts = web_sys::RequestInit::new();
    opts.set_method("GET");
    let req = web_sys::Request::new_with_str_and_init(url, &opts).map_err(js)?;
    if !codes.is_empty() {
        req.headers().set(livereplay_api::CODES_HEADER, codes).map_err(js)?;
    }
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_request(&req)).await.map_err(|_| "network error".to_string())?.dyn_into().map_err(js)?;
    let buf = JsFuture::from(resp.array_buffer().map_err(js)?).await.map_err(js)?;
    Ok(Reply { status: resp.status(), body: js_sys::Uint8Array::new(&buf).to_vec(), headers: resp.headers() })
}

/// GET `url` (with the saved viewing codes); the reply, wrapped by `wrap`, arrives on `tx`.
pub fn get<T: 'static>(ctx: &egui::Context, url: String, codes: String, tx: Sender<T>, wrap: impl FnOnce(Result<Reply, String>) -> T + 'static) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let r = get_async(&url, &codes).await;
        let _ = tx.send(wrap(r));
        ctx.request_repaint();
    });
}
