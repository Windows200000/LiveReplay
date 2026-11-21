//! Windows integration: tray icon, hiding the window to the tray, starting with Windows, one
//! running copy. Elsewhere these do nothing (the window simply closes).
//!
//! While the window is hidden, egui gets no input and the app draws nothing (`hidden()` makes
//! `update` return at once), so the tray costs next to nothing; the uploader keeps running in its
//! own thread and only checks for a battle every few seconds.

#[cfg(windows)]
pub use win::*;

static CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

/// Remember the UI context (to redraw from other threads).
pub fn set_context(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
}

/// Redraw soon (something shown changed in a background thread). Not while the window is in
/// the tray: a hidden window is never redrawn, so the request would keep the event loop busy.
pub fn wake() {
    if hidden() {
        return;
    }
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

#[cfg(not(windows))]
mod other {
    pub fn init(cc: &eframe::CreationContext<'_>) {
        super::set_context(&cc.egui_ctx);
    }
    pub fn hidden() -> bool {
        false
    }
    pub fn hide(_ctx: &egui::Context) {}
    pub fn tray_supported() -> bool {
        false
    }
    pub const TRAY_HINT: &str = "";
    pub fn show() {}
    pub fn set_tooltip(_text: &str) {}
    pub fn quit() -> ! {
        std::process::exit(0)
    }
    pub fn tray_available() -> bool {
        false
    }
    pub fn set_autostart(_on: bool) -> Result<(), String> {
        Ok(())
    }
    pub fn first_instance() -> bool {
        true
    }
    pub fn release_instance() {}
    pub fn autostart_entry() -> Option<String> {
        None
    }
}
#[cfg(not(windows))]
pub use other::*;

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
    use std::sync::Mutex;

    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;
    const SW_RESTORE: i32 = 9;
    const SW_SHOWMINNOACTIVE: i32 = 7;
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOOLWINDOW: isize = 0x80;
    const WS_EX_APPWINDOW: isize = 0x40000;
    const WM_APP: u32 = 0x8000;
    const ERROR_ALREADY_EXISTS: u32 = 183;
    const TITLE: &str = "LiveReplay";

    #[repr(C)]
    struct Msg {
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: [i32; 2],
        private: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn IsIconic(hwnd: isize) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn GetMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> isize;
        fn PostThreadMessageW(thread: u32, msg: u32, wparam: usize, lparam: isize) -> i32;
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> isize;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetCurrentThreadId() -> u32;
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    static HWND: AtomicIsize = AtomicIsize::new(0);
    static MUTEX: AtomicIsize = AtomicIsize::new(0);
    /// The tray icon has a thread of its own (with its own message loop), so it answers clicks
    /// and can be removed while the window draws nothing at all.
    static TRAY_THREAD: AtomicU32 = AtomicU32::new(0);
    static TRAY_OK: AtomicBool = AtomicBool::new(false);
    static IN_TRAY: AtomicBool = AtomicBool::new(false);
    static TRAY_TODO: Mutex<Vec<Todo>> = Mutex::new(Vec::new());
    thread_local! {
        static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    }

    enum Todo {
        Tooltip(String),
        Quit,
    }

    /// Remember the window and put the icon in the tray: left click opens the window, the menu
    /// (right click) only has Quit.
    pub fn init(cc: &eframe::CreationContext<'_>) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = cc.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                HWND.store(w.hwnd.get(), Ordering::Relaxed);
            }
        }
        super::set_context(&cc.egui_ctx);
        let (ready, started) = std::sync::mpsc::channel();
        std::thread::spawn(move || tray_thread(ready));
        let _ = started.recv_timeout(std::time::Duration::from_secs(3));
    }

    fn tray_thread(ready: std::sync::mpsc::Sender<()>) {
        TRAY_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
        let tray = (|| {
            let img = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon.png")).ok()?;
            let icon = Icon::from_rgba(img.rgba, img.width, img.height).ok()?;
            let menu = Menu::new();
            let quit_item = MenuItem::new("Quit", true, None);
            menu.append(&quit_item).ok()?;
            let quit_id = quit_item.id().clone();
            let tray = TrayIconBuilder::new().with_icon(icon).with_tooltip(TITLE).with_menu(Box::new(menu)).with_menu_on_left_click(false).build().ok()?;
            TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    show();
                }
            }));
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if e.id == quit_id {
                    quit();
                }
            }));
            Some(tray)
        })();
        let ok = tray.is_some();
        TRAY.with(|t| *t.borrow_mut() = tray);
        TRAY_OK.store(ok, Ordering::Relaxed);
        let _ = ready.send(());
        if !ok {
            return;
        }
        let mut msg = Msg { hwnd: 0, message: 0, wparam: 0, lparam: 0, time: 0, pt: [0; 2], private: 0 };
        while unsafe { GetMessageW(&mut msg, 0, 0, 0) } > 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let todo = std::mem::take(&mut *TRAY_TODO.lock().unwrap_or_else(|e| e.into_inner()));
            for t in todo {
                match t {
                    Todo::Tooltip(text) => TRAY.with(|t| {
                        if let Some(t) = t.borrow().as_ref() {
                            let _ = t.set_tooltip(Some(text));
                        }
                    }),
                    Todo::Quit => quit(),
                }
            }
        }
    }

    fn tray_do(t: Todo) {
        TRAY_TODO.lock().unwrap_or_else(|e| e.into_inner()).push(t);
        unsafe { PostThreadMessageW(TRAY_THREAD.load(Ordering::Relaxed), WM_APP, 0, 0) };
    }

    pub fn tray_available() -> bool {
        TRAY_OK.load(Ordering::Relaxed)
    }

    /// The window is in the tray (or not shown yet).
    pub fn hidden() -> bool {
        let h = HWND.load(Ordering::Relaxed);
        IN_TRAY.load(Ordering::Relaxed) || (h != 0 && unsafe { IsWindowVisible(h) } == 0)
    }

    pub const TRAY_HINT: &str = "Left-click the tray icon to open the window, right-click > Quit to end LiveReplay";

    pub fn tray_supported() -> bool {
        true
    }

    /// Into the tray: minimised (Windows keeps minimised windows off-screen) and made a tool
    /// window (no taskbar button, not in Alt+Tab). Not hidden: a hidden window is never redrawn,
    /// and a redraw eframe had planned would then keep its event loop busy for good.
    pub fn hide(_ctx: &egui::Context) {
        let h = HWND.load(Ordering::Relaxed);
        if h == 0 {
            return;
        }
        IN_TRAY.store(true, Ordering::Relaxed);
        unsafe {
            ShowWindow(h, SW_HIDE);
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            SetWindowLongPtrW(h, GWL_EXSTYLE, (ex | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW);
            ShowWindow(h, SW_SHOWMINNOACTIVE);
        }
    }

    /// Bring the window back (from the tray, or minimised) to the front.
    pub fn show() {
        let h = HWND.load(Ordering::Relaxed);
        if h == 0 {
            return;
        }
        unsafe {
            if IN_TRAY.swap(false, Ordering::Relaxed) {
                ShowWindow(h, SW_HIDE);
                let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
                SetWindowLongPtrW(h, GWL_EXSTYLE, ex & !WS_EX_TOOLWINDOW);
            }
            ShowWindow(h, if IsIconic(h) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(h);
        }
        super::wake();
    }

    pub fn set_tooltip(text: &str) {
        if tray_available() {
            tray_do(Todo::Tooltip(text.to_string()));
        }
    }

    /// Remove the tray icon and end the app (settings are saved when they change). Works from
    /// any thread: the icon is removed on its own thread.
    pub fn quit() -> ! {
        if tray_available() && unsafe { GetCurrentThreadId() } != TRAY_THREAD.load(Ordering::Relaxed) {
            tray_do(Todo::Quit);
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        TRAY.with(|t| drop(t.borrow_mut().take()));
        std::process::exit(0)
    }

    /// The shortcut in the user's Startup folder (what Windows lists under start-up apps).
    fn startup_link() -> Option<std::path::PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        Some(std::path::PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup").join("LiveReplay.lnk"))
    }

    /// Start with Windows (in the tray), or not.
    pub fn set_autostart(on: bool) -> Result<(), String> {
        let link = startup_link().ok_or("no Startup folder")?;
        if !on {
            return match std::fs::remove_file(&link) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            };
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut sl = mslnk::ShellLink::new(&exe).map_err(|e| e.to_string())?;
        sl.set_arguments(Some("--tray".into()));
        sl.set_working_dir(exe.parent().map(|d| d.to_string_lossy().into_owned()));
        sl.create_lnk(&link).map_err(|e| format!("can't create the start-up shortcut: {e}"))
    }

    /// The start-up shortcut exists.
    pub fn autostart_entry() -> Option<String> {
        startup_link().filter(|p| p.exists()).map(|p| p.display().to_string())
    }

    /// `false` when LiveReplay already runs (its window is brought to the front instead).
    pub fn first_instance() -> bool {
        let name = wide("Local\\LiveReplay.SingleInstance");
        let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if h != 0 && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            let title = wide(TITLE);
            let w = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
            if w != 0 {
                unsafe {
                    ShowWindow(w, if IsIconic(w) != 0 { SW_RESTORE } else { SW_SHOW });
                    SetForegroundWindow(w);
                }
            }
            unsafe { CloseHandle(h) };
            return false;
        }
        MUTEX.store(h, Ordering::Relaxed);
        true
    }

    /// Let a new copy start (restart after an update).
    pub fn release_instance() {
        let h = MUTEX.swap(0, Ordering::Relaxed);
        if h != 0 {
            unsafe { CloseHandle(h) };
        }
    }
}
