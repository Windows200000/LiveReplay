//! New versions: checks the latest GitHub release (not the rolling `latest-build`), tells the
//! user, and on Windows replaces the running exe with the new one (used from the next start).
//!
//! The download is checked before it is used: the size and SHA-256 digest GitHub lists for the
//! file, and that it is a Windows program. The old exe is kept as `<name>.old` until the next start.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use sha2::Digest;

const API: &str = "https://api.github.com/repos/Windows200000/LiveReplay/releases/latest";
/// How often a running app looks again.
const EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    /// A newer release (version, release page); not installed.
    Available(String, String),
    Downloading(String),
    /// Installed; used from the next start.
    Ready(String),
    Failed(String),
}

/// What the app shows about updates (shared with the background checker).
#[derive(Clone)]
pub struct Updater {
    pub state: Arc<Mutex<State>>,
    install: Arc<Mutex<Option<Release>>>,
    notify: Arc<AtomicBool>,
    auto: Arc<AtomicBool>,
    /// Wakes the checker (settings changed).
    poke: mpsc::Sender<()>,
}

/// "Nothing is going on" (no battle, no upload): a new version may restart the app then.
pub type IdleCheck = Box<dyn Fn() -> bool + Send>;

#[derive(Clone, Debug)]
struct Release {
    version: String,
    page: String,
    url: String,
    size: u64,
    sha256: Option<String>,
}

/// This build's file on the release page (self-update is for Windows only).
fn asset_name() -> Option<&'static str> {
    if !cfg!(windows) {
        None
    } else if cfg!(target_arch = "aarch64") {
        Some("livereplay-windows-arm64.exe")
    } else {
        Some("livereplay-windows-x64.exe")
    }
}

fn numbers(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

pub fn newer(remote: &str, local: &str) -> bool {
    numbers(remote) > numbers(local)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(300)).user_agent("LiveReplay-updater").build()
}

fn latest() -> Result<Option<Release>, String> {
    let url = std::env::var("LIVEREPLAY_UPDATE_URL").unwrap_or_else(|_| API.to_string());
    let j: serde_json::Value = agent().get(&url).set("Accept", "application/vnd.github+json").call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())?;
    let tag = j["tag_name"].as_str().unwrap_or_default();
    if !newer(tag, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let asset = asset_name().and_then(|n| j["assets"].as_array()?.iter().find(|a| a["name"] == n));
    Ok(Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        page: j["html_url"].as_str().unwrap_or("https://github.com/Windows200000/LiveReplay/releases/latest").to_string(),
        url: asset.and_then(|a| a["browser_download_url"].as_str()).unwrap_or_default().to_string(),
        size: asset.and_then(|a| a["size"].as_u64()).unwrap_or(0),
        sha256: asset.and_then(|a| a["digest"].as_str()).and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase),
    }))
}

/// Download, check and put the new exe in place of the running one.
fn install(r: &Release) -> Result<(), String> {
    if r.url.is_empty() {
        return Err("no download for this system in the release".into());
    }
    let mut body = Vec::new();
    use std::io::Read;
    agent().get(&r.url).call().map_err(|e| e.to_string())?.into_reader().take(256 << 20).read_to_end(&mut body).map_err(|e| e.to_string())?;
    if r.size != 0 && body.len() as u64 != r.size {
        return Err("download incomplete".into());
    }
    if let Some(want) = &r.sha256 {
        let got: String = sha2::Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
        if &got != want {
            return Err("download damaged (checksum differs)".into());
        }
    }
    if body.len() < 1 << 20 || !body.starts_with(b"MZ") {
        return Err("the download is not a Windows program".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    replace_exe(&exe, &body)
}

/// Windows lets a running exe be renamed, not overwritten: current -> `.old`, new -> current.
fn replace_exe(exe: &Path, body: &[u8]) -> Result<(), String> {
    let new = exe.with_extension("exe.new");
    let old = old_path(exe);
    std::fs::write(&new, body).map_err(|e| format!("can't write next to the app: {e}"))?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|e| format!("can't replace the app: {e}"))?;
    if let Err(e) = std::fs::rename(&new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(format!("can't replace the app: {e}"));
    }
    Ok(())
}

fn old_path(exe: &Path) -> PathBuf {
    exe.with_extension("exe.old")
}

impl Updater {
    /// Starts the background check (at once, then every few hours). `notify`: look for new
    /// versions at all; `auto`: install them without asking (Windows).
    pub fn start(notify: bool, auto: bool, idle: IdleCheck) -> Self {
        // The exe an update replaced is no longer running now.
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::remove_file(old_path(&exe));
            let _ = std::fs::remove_file(exe.with_extension("exe.new"));
        }
        let (poke, woken) = mpsc::channel();
        let u = Updater {
            state: Arc::new(Mutex::new(State::Idle)),
            install: Arc::new(Mutex::new(None)),
            notify: Arc::new(AtomicBool::new(notify)),
            auto: Arc::new(AtomicBool::new(auto)),
            poke,
        };
        let u2 = u.clone();
        std::thread::spawn(move || loop {
            if u2.notify.load(Ordering::Relaxed) {
                u2.check(u2.auto.load(Ordering::Relaxed));
            }
            // Installed while in the tray: start the new version as soon as nothing is going on.
            let ready = matches!(u2.state(), State::Ready(_));
            if ready && super::desktop::hidden() && idle() {
                restart(true);
            }
            let wait = if ready { Duration::from_secs(30) } else { EVERY };
            if let Err(mpsc::RecvTimeoutError::Disconnected) = woken.recv_timeout(wait) {
                return;
            }
        });
        u
    }

    /// Settings changed: look again now with the new choice.
    pub fn set_options(&self, notify: bool, auto: bool) {
        let before = (self.notify.swap(notify, Ordering::Relaxed), self.auto.swap(auto, Ordering::Relaxed));
        if before != (notify, auto) {
            if !notify {
                self.set(State::Idle);
            }
            let _ = self.poke.send(());
        }
    }

    fn set(&self, s: State) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = s;
        super::desktop::wake();
    }

    pub fn state(&self) -> State {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn check(&self, auto: bool) {
        if matches!(self.state(), State::Ready(_) | State::Downloading(_)) {
            return;
        }
        match latest() {
            Ok(Some(r)) => {
                *self.install.lock().unwrap_or_else(|e| e.into_inner()) = Some(r.clone());
                if auto && asset_name().is_some() && !r.url.is_empty() {
                    self.install_now();
                } else {
                    self.set(State::Available(r.version, r.page));
                }
            }
            Ok(None) => {}
            // Offline etc.: try again next time, quietly.
            Err(_) => {}
        }
    }

    /// Install the release found, in the background (the "Install" button).
    pub fn install_in_background(&self) {
        let u = self.clone();
        std::thread::spawn(move || u.install_now());
    }

    /// Download and install the release found (blocking).
    fn install_now(&self) {
        let Some(r) = self.install.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
        self.set(State::Downloading(r.version.clone()));
        match install(&r) {
            Ok(()) => self.set(State::Ready(r.version)),
            Err(e) => self.set(State::Failed(format!("Update to {} failed: {e}", r.version))),
        }
    }

    /// Self-update works here (Windows build).
    pub fn can_install() -> bool {
        asset_name().is_some()
    }
}

/// Start the (new) exe again with the same mode and end this one.
pub fn restart(to_tray: bool) -> ! {
    super::desktop::release_instance();
    if let Ok(exe) = std::env::current_exe() {
        let mut cmd = std::process::Command::new(exe);
        if to_tray {
            cmd.arg("--tray");
        }
        let _ = cmd.spawn();
    }
    super::desktop::quit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("v1.0.1", "1.0.0"));
        assert!(newer("v1.10.0", "1.9.9"));
        assert!(!newer("v1.0.0", "1.0.0"));
        assert!(!newer("v0.9.9", "1.0.0"));
        assert!(!newer("latest-build", "1.0.0"));
    }

    #[test]
    fn replace() {
        let dir = std::env::temp_dir().join(format!("lr-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("app.exe");
        std::fs::write(&exe, b"old").unwrap();
        replace_exe(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
