//! Windows builds get the app icon embedded in the executable.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let _ = embed_resource::compile("assets/app.rc", embed_resource::NONE);
    }
}
