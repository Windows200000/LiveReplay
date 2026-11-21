//! New versions: checks the latest GitHub release (not the rolling `latest-build`), tells the
//! user, and replaces the running program with the new one (used from the next start).
//!
//! The download is checked before it is used: the size and SHA-256 digest GitHub lists for the
//! file, and that it is a program for this system. Windows: the old exe is kept as `<name>.old`
//! until the next start (a running exe can be renamed, not overwritten). macOS / Linux: the
//! `livereplay` file from the release's `.tar.gz` replaces the running one (allowed there).

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

/// This build's file on the release page.
fn asset_name() -> Option<&'static str> {
    let arm = cfg!(target_arch = "aarch64");
    if cfg!(windows) {
        Some(if arm { "livereplay-windows-arm64.exe" } else { "livereplay-windows-x64.exe" })
    } else if cfg!(target_os = "macos") {
        Some(if arm { "livereplay-macos-arm64.tar.gz" } else { "livereplay-macos-x64.tar.gz" })
    } else if cfg!(target_os = "linux") {
        Some(if arm { "livereplay-linux-arm64.tar.gz" } else { "livereplay-linux-x64.tar.gz" })
    } else {
        None
    }
}

/// The `livereplay` program inside a release `.tar.gz` (one file; tar read by hand).
fn from_tar_gz(gz: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(gz).take(512 << 20).read_to_end(&mut tar).map_err(|_| "the download is not a .tar.gz")?;
    let mut at = 0;
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        let name = String::from_utf8_lossy(&h[..100]).trim_end_matches('\0').to_string();
        if name.is_empty() {
            break;
        }
        let size = usize::from_str_radix(String::from_utf8_lossy(&h[124..136]).trim_matches(|c: char| c == '\0' || c == ' '), 8).map_err(|_| "broken .tar.gz")?;
        let data = tar.get(at + 512..at + 512 + size).ok_or("broken .tar.gz")?;
        let regular = h[156] == b'0' || h[156] == 0;
        if regular && name.trim_start_matches("./") == "livereplay" {
            return Ok(data.to_vec());
        }
        at += 512 + size.div_ceil(512) * 512;
    }
    Err("no livereplay program in the download".into())
}

/// A program for this system: Windows PE, Linux ELF, macOS Mach-O.
fn is_program(b: &[u8]) -> bool {
    b.len() > 1 << 20
        && if cfg!(windows) {
            b.starts_with(b"MZ")
        } else if cfg!(target_os = "macos") {
            [[0xcf, 0xfa, 0xed, 0xfe], [0xfe, 0xed, 0xfa, 0xcf], [0xca, 0xfe, 0xba, 0xbe]].iter().any(|m| b.starts_with(m))
        } else {
            b.starts_with(b"\x7fELF")
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
    let program = if r.url.ends_with(".tar.gz") { from_tar_gz(&body)? } else { body };
    if !is_program(&program) {
        return Err("the download is not a program for this system".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    replace_exe(&exe, &program)
}

/// macOS / Linux: the new file is written next to it and renamed over it (the running program
/// keeps its old copy until it ends).
#[cfg(not(windows))]
fn replace_exe(exe: &Path, body: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let new = exe.with_extension("new");
    std::fs::write(&new, body).map_err(|e| format!("can't write next to the app: {e}"))?;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    std::fs::rename(&new, exe).map_err(|e| {
        let _ = std::fs::remove_file(&new);
        format!("can't replace the app: {e}")
    })
}

/// Windows lets a running exe be renamed, not overwritten: current -> `.old`, new -> current.
#[cfg(windows)]
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
    fn tar_gz() {
        // A tar with a README and the program, gzipped.
        let mut tar = Vec::new();
        for (name, data) in [("README.md", &b"hello"[..]), ("livereplay", &b"\x7fELF-program"[..])] {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}\0", data.len());
            h[124..136].copy_from_slice(size.as_bytes());
            h[156] = b'0';
            tar.extend_from_slice(&h);
            tar.extend_from_slice(data);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
        tar.extend_from_slice(&[0u8; 1024]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        assert_eq!(from_tar_gz(&gz.finish().unwrap()).unwrap(), b"\x7fELF-program");
        assert!(from_tar_gz(b"not gzip").is_err());
    }

    #[test]
    fn replace() {
        let dir = std::env::temp_dir().join(format!("lr-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("app.exe");
        std::fs::write(&exe, b"old").unwrap();
        replace_exe(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        // Windows keeps the running exe as .old (removed at the next start); elsewhere it is
        // replaced directly.
        assert_eq!(std::fs::read(old_path(&exe)).ok().as_deref(), if cfg!(windows) { Some(&b"old"[..]) } else { None });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
