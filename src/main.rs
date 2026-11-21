#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! LiveReplay desktop app.
//!
//! Command line: `livereplay [<file.wotreplay> [--at <seconds>]] [--live] [--2d|--3d] [--play]
//! [--look x,z [--cam yaw,pitch,dist]]` — the viewer options are mainly for testing.
//! `livereplay --upload <file.wotreplay>...` uploads replays without opening a window and prints
//! their web links (server: `LIVEREPLAY_SERVER`, default wot.arne.my).
//! `livereplay --tray` starts in the tray (how Windows starts it, see Settings).

use livereplay::app::shell::App;

/// Value following `flag` on the command line.
fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

/// Comma-separated numbers, e.g. "12.5,-40".
fn numbers(s: &str) -> Vec<f32> {
    s.split(',').filter_map(|v| v.trim().parse().ok()).collect()
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--upload") {
        let cfg = livereplay::app::config::Config::load();
        let files: Vec<std::path::PathBuf> = args[1..].iter().map(Into::into).collect();
        for (f, r) in files.iter().zip(livereplay::app::upload::upload_files(&cfg, &files)) {
            match r {
                Ok(link) => println!("{} -> {link}", f.display()),
                Err(e) => println!("{}: {e}", f.display()),
            }
        }
        return Ok(());
    }
    let to_tray = args.iter().any(|a| a == "--tray");
    // A plain start while LiveReplay runs (perhaps in the tray) brings that one to the front.
    if args.iter().all(|a| a == "--tray") && !livereplay::app::desktop::first_instance() {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        depth_buffer: 24,
        viewport: egui::ViewportBuilder::default().with_title("LiveReplay").with_inner_size([1280.0, 860.0])
            .with_drag_and_drop(true)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()),
        ..Default::default()
    };
    eframe::run_native(
        "LiveReplay",
        options,
        Box::new(move |cc| {
            livereplay::app::desktop::init(cc);
            let mut app = App::default();
            if to_tray {
                app.start_in_tray();
            }
            if let Some(dir) = arg_after(&args, "--replays") {
                app.watch_replays_dir(dir);
            }
            if args.iter().any(|a| a == "--live") {
                app.open_live();
            } else if let Some(file) = args.iter().find(|a| a.ends_with(".wotreplay")) {
                match arg_after(&args, "--at").and_then(|s| s.parse().ok()) {
                    Some(at) => app.open_viewer_at(file.into(), at),
                    None => app.open_viewer(file.into()),
                }
            }
            if let Some(q) = arg_after(&args, "--link") {
                app.apply_link(&q);
            }
            if args.iter().any(|a| a == "--settings") {
                app.open_settings();
            }
            if args.iter().any(|a| a == "--3d") {
                app.force_3d();
            }
            if args.iter().any(|a| a == "--2d") {
                app.force_2d();
            }
            if args.iter().any(|a| a == "--pin") {
                app.pin_own();
            }
            if args.iter().any(|a| a == "--play") {
                app.autoplay();
            }
            if let Some([x, z]) = arg_after(&args, "--look").map(|s| numbers(&s)).as_deref().and_then(|v| <[f32; 2]>::try_from(v).ok()) {
                app.look_at(x, z);
            }
            if let Some([yaw, pitch, dist]) = arg_after(&args, "--cam").map(|s| numbers(&s)).as_deref().and_then(|v| <[f32; 3]>::try_from(v).ok()) {
                app.orbit(yaw, pitch, dist);
            }
            Ok(Box::new(app))
        }),
    )
}
