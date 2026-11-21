// Starts the web viewer (a file of its own: the site's Content-Security-Policy allows no inline scripts).
import init from "/livereplay_web.js";
init({ module_or_path: "/livereplay_web_bg.wasm" }).then(() => document.getElementById("loading").remove());
