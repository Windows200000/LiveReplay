//! "Watch in game": hand a replay to the World of Tanks client.

use super::config::Config;
use std::path::Path;
use std::process::Command;

/// Start the game with the replay (returns right away; the game runs on its own).
pub fn watch_in_game(cfg: &Config, replay: &Path) -> Result<(), String> {
    let exe = Path::new(&cfg.game_dir).join("WorldOfTanks.exe");
    let mut cmd = if cfg!(windows) {
        if !exe.is_file() {
            return Err("Game folder not set (Settings)".into());
        }
        let mut c = Command::new(&exe);
        c.arg(replay).current_dir(&cfg.game_dir);
        c
    } else {
        // Linux: the Windows client runs inside a Bottles bottle.
        if cfg.bottle.is_empty() {
            return Err("Bottle name not set (Settings)".into());
        }
        let mut c = Command::new("bottles-cli");
        c.env_remove("LD_LIBRARY_PATH").args(["run", "-b", &cfg.bottle, "-e"]).arg(&exe).arg("--args").arg(replay);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| format!("Could not start the game: {e}"))
}
