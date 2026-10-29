use std::{
    io::{self, BufRead, BufReader},
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
};

const PAIRING_ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

enum ProcessUpdate {
    Connected(String),
    Stopped,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GuiMode {
    Listening,
    Connecting,
    Connected,
    Stopped,
}

struct SyncProcess {
    device_name: String,
    child: Option<Child>,
    updates: Option<Receiver<String>>,
}

impl SyncProcess {
    fn new(device_name: String) -> Self {
        Self {
            device_name,
            child: None,
            updates: None,
        }
    }

    fn is_running(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) | Err(_) => {
                self.child = None;
                self.updates = None;
                false
            }
        }
    }

    fn start_listener(&mut self) -> io::Result<()> {
        self.start(None)
    }

    fn start_peer(&mut self, peer: Ipv4Addr) -> io::Result<()> {
        self.start(Some(peer))
    }

    fn start(&mut self, peer: Option<Ipv4Addr>) -> io::Result<()> {
        if self.is_running() {
            return Ok(());
        }

        let mut command = Command::new(sync_executable()?);
        command
            .arg("sync")
            .arg(&self.device_name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(peer) = peer {
            command.arg("--peer").arg(peer.to_string());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            // Keep the CLI transport completely behind the GUI window.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("failed to capture ClipSync output"))?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(peer) = parse_transport_peer(&line) {
                    let _ = sender.send(peer.to_owned());
                }
            }
        });
        self.child = Some(child);
        self.updates = Some(receiver);
        Ok(())
    }

    fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        self.updates = None;
        #[cfg(unix)]
        unsafe {
            // The child is the leader of a dedicated process group. A negative
            // PID stops it and any wl-paste helper processes it created.
            kill(-(child.id() as i32), SIGTERM);
        }
        #[cfg(not(unix))]
        let _ = child.kill();
        let _ = child.wait();
    }

    fn poll_update(&mut self) -> Option<ProcessUpdate> {
        let connected = self
            .updates
            .as_ref()
            .and_then(|updates| updates.try_iter().last());
        let was_running = self.child.is_some();
        if was_running && !self.is_running() {
            Some(ProcessUpdate::Stopped)
        } else {
            connected.map(ProcessUpdate::Connected)
        }
    }
}

impl Drop for SyncProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn sync_executable() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "windows")]
    let file_name = "clipsync.exe";
    #[cfg(not(target_os = "windows"))]
    let file_name = "clipsync";
    Ok(executable.with_file_name(file_name))
}

fn parse_transport_peer(line: &str) -> Option<&str> {
    line.strip_prefix("Transport peer: ")?
        .rsplit_once(" [")
        .map(|(device_name, _)| device_name)
}

fn device_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| format!("ClipSync {}", std::env::consts::OS))
}

fn local_ipv4() -> io::Result<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.connect("192.0.2.1:9")?;
    match socket.local_addr()? {
        SocketAddr::V4(address) if !address.ip().is_loopback() => Ok(*address.ip()),
        _ => Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "no routable IPv4 address is available",
        )),
    }
}

fn pairing_code(ip: Ipv4Addr) -> String {
    let octets = ip.octets();
    let payload = [
        octets[0],
        octets[1],
        octets[2],
        octets[3],
        checksum(&octets),
    ];
    let value = payload
        .into_iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(byte));
    let mut encoded = String::with_capacity(9);
    for index in (0..8).rev() {
        if index == 3 {
            encoded.push('-');
        }
        encoded.push(PAIRING_ALPHABET[((value >> (index * 5)) & 31) as usize] as char);
    }
    encoded
}

fn decode_pairing_code(code: &str) -> Result<Ipv4Addr, &'static str> {
    let normalized: Vec<u8> = code
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'-')
        .map(|byte| byte.to_ascii_uppercase())
        .collect();
    if normalized.len() != 8 {
        return Err("匹配码应为 8 位字符");
    }
    let mut value = 0_u64;
    for byte in normalized {
        let digit = PAIRING_ALPHABET
            .iter()
            .position(|candidate| *candidate == byte)
            .ok_or("匹配码包含无效字符")?;
        value = (value << 5) | digit as u64;
    }
    let bytes = value.to_be_bytes();
    let octets = [bytes[3], bytes[4], bytes[5], bytes[6]];
    if bytes[7] != checksum(&octets) {
        return Err("匹配码校验失败");
    }
    Ok(Ipv4Addr::from(octets))
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes
        .iter()
        .fold(0xA7, |value, byte| value.rotate_left(1) ^ byte)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::{decode_pairing_code, pairing_code, parse_transport_peer};

    #[test]
    fn extracts_connected_device_name() {
        assert_eq!(
            parse_transport_peer("Transport peer: Linux laptop [linux]"),
            Some("Linux laptop")
        );
        assert_eq!(parse_transport_peer("Connected to 10.0.0.2:24872"), None);
    }

    #[test]
    fn pairing_code_round_trip_preserves_ipv4() {
        let ip = Ipv4Addr::new(10, 29, 182, 14);
        let code = pairing_code(ip);
        assert_eq!(code.len(), 9);
        assert_eq!(decode_pairing_code(&code), Ok(ip));
        assert_eq!(decode_pairing_code(&code.to_ascii_lowercase()), Ok(ip));
    }

    #[test]
    fn pairing_code_rejects_typo() {
        let mut code = pairing_code(Ipv4Addr::new(10, 29, 182, 14)).into_bytes();
        code[0] = if code[0] == b'2' { b'3' } else { b'2' };
        assert_eq!(
            decode_pairing_code(std::str::from_utf8(&code).unwrap()),
            Err("匹配码校验失败")
        );
    }
}

#[cfg(unix)]
const SIGTERM: i32 = 15;

#[cfg(unix)]
#[link(name = "c")]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(target_os = "linux")]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}

#[cfg(target_os = "windows")]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    windows::run()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    Err("ClipSync GUI currently supports only Windows and Linux".into())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use libloading::Library;
    use std::{ffi::CString, os::raw::c_char, ptr, sync::OnceLock};

    type Widget = *mut std::ffi::c_void;
    type SignalCallback = unsafe extern "C" fn();
    type TimeoutCallback = unsafe extern "C" fn(*mut std::ffi::c_void) -> i32;

    struct GtkApi {
        gtk_init: unsafe extern "C" fn(*mut i32, *mut *mut *mut c_char),
        gtk_main: unsafe extern "C" fn(),
        gtk_main_quit: unsafe extern "C" fn(),
        gtk_window_new: unsafe extern "C" fn(i32) -> Widget,
        gtk_window_set_title: unsafe extern "C" fn(Widget, *const c_char),
        gtk_window_set_default_size: unsafe extern "C" fn(Widget, i32, i32),
        gtk_window_set_transient_for: unsafe extern "C" fn(Widget, Widget),
        gtk_window_set_modal: unsafe extern "C" fn(Widget, i32),
        gtk_button_new_with_label: unsafe extern "C" fn(*const c_char) -> Widget,
        gtk_button_set_label: unsafe extern "C" fn(Widget, *const c_char),
        gtk_container_add: unsafe extern "C" fn(Widget, Widget),
        gtk_widget_show_all: unsafe extern "C" fn(Widget),
        gtk_widget_destroy: unsafe extern "C" fn(Widget),
        gtk_dialog_new: unsafe extern "C" fn() -> Widget,
        gtk_dialog_add_button: unsafe extern "C" fn(Widget, *const c_char, i32) -> Widget,
        gtk_dialog_get_content_area: unsafe extern "C" fn(Widget) -> Widget,
        gtk_dialog_run: unsafe extern "C" fn(Widget) -> i32,
        gtk_entry_new: unsafe extern "C" fn() -> Widget,
        gtk_entry_set_placeholder_text: unsafe extern "C" fn(Widget, *const c_char),
        gtk_entry_get_text: unsafe extern "C" fn(Widget) -> *const c_char,
        gtk_box_pack_start: unsafe extern "C" fn(Widget, Widget, i32, i32, u32),
        g_signal_connect_data: unsafe extern "C" fn(
            Widget,
            *const c_char,
            Option<SignalCallback>,
            *mut std::ffi::c_void,
            Option<unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void)>,
            u32,
        ) -> u64,
        g_timeout_add:
            unsafe extern "C" fn(u32, Option<TimeoutCallback>, *mut std::ffi::c_void) -> u32,
    }

    static GTK: OnceLock<GtkApi> = OnceLock::new();

    struct State {
        process: SyncProcess,
        button: Widget,
        window: Widget,
        pairing_code: String,
        mode: GuiMode,
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let pairing_code = pairing_code(local_ipv4()?);
        let api = load_gtk()?;
        GTK.set(api).map_err(|_| "GTK is already initialized")?;
        let api = GTK.get().unwrap();

        unsafe {
            (api.gtk_init)(ptr::null_mut(), ptr::null_mut());
            let window = (api.gtk_window_new)(0);
            let title = CString::new("ClipSync")?;
            (api.gtk_window_set_title)(window, title.as_ptr());
            (api.gtk_window_set_default_size)(window, 300, 120);

            let start = CString::new(format!("本机匹配码：{pairing_code}\n点击输入对方匹配码"))?;
            let button = (api.gtk_button_new_with_label)(start.as_ptr());
            (api.gtk_container_add)(window, button);

            let mut process = SyncProcess::new(device_name());
            let mode = if process.start_listener().is_ok() {
                GuiMode::Listening
            } else {
                GuiMode::Stopped
            };
            let state = Box::new(State {
                process,
                button,
                window,
                pairing_code,
                mode,
            });
            let state = Box::into_raw(state);
            connect_signal(button, "clicked", on_clicked, state.cast())?;
            connect_signal(window, "destroy", on_destroy, state.cast())?;
            (api.g_timeout_add)(100, Some(on_timer), state.cast());
            (api.gtk_widget_show_all)(window);
            (api.gtk_main)();

            drop(Box::from_raw(state));
        }
        Ok(())
    }

    unsafe fn connect_signal(
        widget: Widget,
        signal: &str,
        callback: unsafe extern "C" fn(Widget, *mut State),
        data: *mut std::ffi::c_void,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let signal = CString::new(signal)?;
        let callback: SignalCallback = unsafe { std::mem::transmute(callback) };
        unsafe {
            (GTK.get().unwrap().g_signal_connect_data)(
                widget,
                signal.as_ptr(),
                Some(callback),
                data,
                None,
                0,
            );
        }
        Ok(())
    }

    unsafe extern "C" fn on_clicked(_button: Widget, state: *mut State) {
        let state = unsafe { &mut *state };
        match state.mode {
            GuiMode::Connecting | GuiMode::Connected => {
                state.process.stop();
                state.mode = GuiMode::Stopped;
                set_button_label(state.button, "已停止（点击重新监听）");
            }
            GuiMode::Stopped => match state.process.start_listener() {
                Ok(()) => {
                    state.mode = GuiMode::Listening;
                    set_pairing_label(state);
                }
                Err(_) => set_button_label(state.button, "启动失败（点击重试）"),
            },
            GuiMode::Listening => {
                let Some(code) = prompt_pairing_code(state.window) else {
                    return;
                };
                match decode_pairing_code(&code) {
                    Ok(peer) => {
                        state.process.stop();
                        match state.process.start_peer(peer) {
                            Ok(()) => {
                                state.mode = GuiMode::Connecting;
                                set_button_label(
                                    state.button,
                                    &format!("正在连接 {peer}（点击停止）"),
                                );
                            }
                            Err(_) => {
                                state.mode = GuiMode::Stopped;
                                set_button_label(state.button, "启动失败（点击重试）");
                            }
                        }
                    }
                    Err(error) => set_button_label(state.button, error),
                }
            }
        }
    }

    unsafe extern "C" fn on_timer(state: *mut std::ffi::c_void) -> i32 {
        let state = unsafe { &mut *state.cast::<State>() };
        match state.process.poll_update() {
            Some(ProcessUpdate::Connected(peer)) => {
                state.mode = GuiMode::Connected;
                set_button_label(state.button, &format!("已连接：{peer}（点击停止）"));
            }
            Some(ProcessUpdate::Stopped) => {
                state.mode = GuiMode::Stopped;
                set_button_label(state.button, "连接已停止（点击重试）")
            }
            None => {}
        }
        1
    }

    unsafe extern "C" fn on_destroy(_window: Widget, state: *mut State) {
        unsafe { (&mut *state).process.stop() };
        unsafe { (GTK.get().unwrap().gtk_main_quit)() };
    }

    fn set_button_label(button: Widget, label: &str) {
        if let Ok(label) = CString::new(label) {
            unsafe { (GTK.get().unwrap().gtk_button_set_label)(button, label.as_ptr()) };
        }
    }

    fn set_pairing_label(state: &State) {
        set_button_label(
            state.button,
            &format!("本机匹配码：{}\n点击输入对方匹配码", state.pairing_code),
        );
    }

    fn prompt_pairing_code(parent: Widget) -> Option<String> {
        let api = GTK.get().unwrap();
        unsafe {
            let dialog = (api.gtk_dialog_new)();
            let title = CString::new("输入匹配码").unwrap();
            (api.gtk_window_set_title)(dialog, title.as_ptr());
            (api.gtk_window_set_transient_for)(dialog, parent);
            (api.gtk_window_set_modal)(dialog, 1);
            (api.gtk_window_set_default_size)(dialog, 320, 110);

            let entry = (api.gtk_entry_new)();
            let placeholder = CString::new("例如：ABCD-EFGH").unwrap();
            (api.gtk_entry_set_placeholder_text)(entry, placeholder.as_ptr());
            let content = (api.gtk_dialog_get_content_area)(dialog);
            (api.gtk_box_pack_start)(content, entry, 1, 1, 12);
            let cancel = CString::new("取消").unwrap();
            let connect = CString::new("连接").unwrap();
            (api.gtk_dialog_add_button)(dialog, cancel.as_ptr(), -6);
            (api.gtk_dialog_add_button)(dialog, connect.as_ptr(), -3);
            (api.gtk_widget_show_all)(dialog);
            let response = (api.gtk_dialog_run)(dialog);
            let result = if response == -3 {
                let text = (api.gtk_entry_get_text)(entry);
                Some(
                    std::ffi::CStr::from_ptr(text)
                        .to_string_lossy()
                        .into_owned(),
                )
            } else {
                None
            };
            (api.gtk_widget_destroy)(dialog);
            result
        }
    }

    fn load_gtk() -> Result<GtkApi, Box<dyn std::error::Error>> {
        let gtk = Box::leak(Box::new(unsafe { Library::new("libgtk-3.so.0")? }));
        let gobject = Box::leak(Box::new(unsafe { Library::new("libgobject-2.0.so.0")? }));
        let glib = Box::leak(Box::new(unsafe { Library::new("libglib-2.0.so.0")? }));
        unsafe {
            Ok(GtkApi {
                gtk_init: *gtk.get(b"gtk_init\0")?,
                gtk_main: *gtk.get(b"gtk_main\0")?,
                gtk_main_quit: *gtk.get(b"gtk_main_quit\0")?,
                gtk_window_new: *gtk.get(b"gtk_window_new\0")?,
                gtk_window_set_title: *gtk.get(b"gtk_window_set_title\0")?,
                gtk_window_set_default_size: *gtk.get(b"gtk_window_set_default_size\0")?,
                gtk_window_set_transient_for: *gtk.get(b"gtk_window_set_transient_for\0")?,
                gtk_window_set_modal: *gtk.get(b"gtk_window_set_modal\0")?,
                gtk_button_new_with_label: *gtk.get(b"gtk_button_new_with_label\0")?,
                gtk_button_set_label: *gtk.get(b"gtk_button_set_label\0")?,
                gtk_container_add: *gtk.get(b"gtk_container_add\0")?,
                gtk_widget_show_all: *gtk.get(b"gtk_widget_show_all\0")?,
                gtk_widget_destroy: *gtk.get(b"gtk_widget_destroy\0")?,
                gtk_dialog_new: *gtk.get(b"gtk_dialog_new\0")?,
                gtk_dialog_add_button: *gtk.get(b"gtk_dialog_add_button\0")?,
                gtk_dialog_get_content_area: *gtk.get(b"gtk_dialog_get_content_area\0")?,
                gtk_dialog_run: *gtk.get(b"gtk_dialog_run\0")?,
                gtk_entry_new: *gtk.get(b"gtk_entry_new\0")?,
                gtk_entry_set_placeholder_text: *gtk.get(b"gtk_entry_set_placeholder_text\0")?,
                gtk_entry_get_text: *gtk.get(b"gtk_entry_get_text\0")?,
                gtk_box_pack_start: *gtk.get(b"gtk_box_pack_start\0")?,
                g_signal_connect_data: *gobject.get(b"g_signal_connect_data\0")?,
                g_timeout_add: *glib.get(b"g_timeout_add\0")?,
            })
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::{ffi::c_void, mem, ptr};

    type Hwnd = *mut c_void;
    type Hinstance = *mut c_void;
    type Wparam = usize;
    type Lparam = isize;
    type Lresult = isize;

    const WM_CREATE: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_CLOSE: u32 = 0x0010;
    const WM_TIMER: u32 = 0x0113;
    const WM_COMMAND: u32 = 0x0111;
    const GWLP_USERDATA: i32 = -21;
    const WS_OVERLAPPED: u32 = 0x00000000;
    const WS_CAPTION: u32 = 0x00C00000;
    const WS_SYSMENU: u32 = 0x00080000;
    const WS_MINIMIZEBOX: u32 = 0x00020000;
    const WS_VISIBLE: u32 = 0x10000000;
    const WS_CHILD: u32 = 0x40000000;
    const WS_POPUP: u32 = 0x80000000;
    const WS_BORDER: u32 = 0x00800000;
    const BS_PUSHBUTTON: u32 = 0x00000000;
    const ES_AUTOHSCROLL: u32 = 0x0080;
    const CW_USEDEFAULT: i32 = i32::MIN;
    const BUTTON_ID: usize = 1001;
    const STATUS_TIMER_ID: usize = 1;
    const CONNECT_ID: usize = 2001;
    const CANCEL_ID: usize = 2002;

    #[repr(C)]
    struct WndClassW {
        style: u32,
        wnd_proc: Option<unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult>,
        cls_extra: i32,
        wnd_extra: i32,
        instance: Hinstance,
        icon: *mut c_void,
        cursor: *mut c_void,
        background: *mut c_void,
        menu_name: *const u16,
        class_name: *const u16,
    }

    #[repr(C)]
    struct Message {
        hwnd: Hwnd,
        message: u32,
        wparam: Wparam,
        lparam: Lparam,
        time: u32,
        point_x: i32,
        point_y: i32,
        private: u32,
    }

    #[repr(C)]
    struct CreateStructW {
        create_params: *mut c_void,
        instance: Hinstance,
        menu: *mut c_void,
        parent: Hwnd,
        cy: i32,
        cx: i32,
        y: i32,
        x: i32,
        style: i32,
        name: *const u16,
        class: *const u16,
        extended_style: u32,
    }

    struct State {
        process: SyncProcess,
        button: Hwnd,
        pairing_code: String,
        mode: GuiMode,
    }

    struct PairDialogState {
        edit: Hwnd,
        result: Option<String>,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassW(class: *const WndClassW) -> u16;
        fn CreateWindowExW(
            extended_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Hwnd,
            menu: *mut c_void,
            instance: Hinstance,
            parameter: *mut c_void,
        ) -> Hwnd;
        fn DefWindowProcW(hwnd: Hwnd, message: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
        fn GetMessageW(message: *mut Message, hwnd: Hwnd, min: u32, max: u32) -> i32;
        fn TranslateMessage(message: *const Message) -> i32;
        fn DispatchMessageW(message: *const Message) -> Lresult;
        fn PostQuitMessage(exit_code: i32);
        fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
        fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
        fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
        fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max_count: i32) -> i32;
        fn DestroyWindow(hwnd: Hwnd) -> i32;
        fn IsWindow(hwnd: Hwnd) -> i32;
        fn EnableWindow(hwnd: Hwnd, enabled: i32) -> i32;
        fn SetForegroundWindow(hwnd: Hwnd) -> i32;
        fn SetTimer(hwnd: Hwnd, event_id: usize, interval_ms: u32, callback: *mut c_void) -> usize;
        fn KillTimer(hwnd: Hwnd, event_id: usize) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(module_name: *const u16) -> Hinstance;
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let pairing_code = pairing_code(local_ipv4()?);
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let class_name = wide("ClipSyncWindow");
            let class = WndClassW {
                style: 0,
                wnd_proc: Some(window_proc),
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon: ptr::null_mut(),
                cursor: ptr::null_mut(),
                background: 6usize as *mut c_void,
                menu_name: ptr::null(),
                class_name: class_name.as_ptr(),
            };
            if RegisterClassW(&class) == 0 {
                return Err(io::Error::last_os_error().into());
            }

            let dialog_class_name = wide("ClipSyncPairDialog");
            let dialog_class = WndClassW {
                style: 0,
                wnd_proc: Some(pair_dialog_proc),
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon: ptr::null_mut(),
                cursor: ptr::null_mut(),
                background: 6usize as *mut c_void,
                menu_name: ptr::null(),
                class_name: dialog_class_name.as_ptr(),
            };
            if RegisterClassW(&dialog_class) == 0 {
                return Err(io::Error::last_os_error().into());
            }

            let mut process = SyncProcess::new(device_name());
            let mode = if process.start_listener().is_ok() {
                GuiMode::Listening
            } else {
                GuiMode::Stopped
            };
            let state = Box::into_raw(Box::new(State {
                process,
                button: ptr::null_mut(),
                pairing_code,
                mode,
            }));
            let title = wide("ClipSync");
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                380,
                160,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                state.cast(),
            );
            if hwnd.is_null() {
                drop(Box::from_raw(state));
                return Err(io::Error::last_os_error().into());
            }

            let mut message: Message = mem::zeroed();
            while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            drop(Box::from_raw(state));
        }
        Ok(())
    }

    unsafe extern "system" fn window_proc(
        hwnd: Hwnd,
        message: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult {
        match message {
            WM_CREATE => {
                let create = unsafe { &*(lparam as *const CreateStructW) };
                let state = create.create_params.cast::<State>();
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize) };
                let button_class = wide("BUTTON");
                let label = wide(&format!(
                    "本机匹配码：{}\r\n点击输入对方匹配码",
                    unsafe { &(*state).pairing_code }
                ));
                let button = unsafe {
                    CreateWindowExW(
                        0,
                        button_class.as_ptr(),
                        label.as_ptr(),
                        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                        40,
                        35,
                        300,
                        50,
                        hwnd,
                        BUTTON_ID as *mut c_void,
                        create.instance,
                        ptr::null_mut(),
                    )
                };
                unsafe { (*state).button = button };
                unsafe { SetTimer(hwnd, STATUS_TIMER_ID, 100, ptr::null_mut()) };
                0
            }
            WM_COMMAND if wparam & 0xffff == BUTTON_ID => {
                let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State };
                if !state.is_null() {
                    handle_main_button(hwnd, unsafe { &mut *state });
                }
                0
            }
            WM_TIMER if wparam == STATUS_TIMER_ID => {
                let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State };
                if !state.is_null() {
                    match unsafe { (*state).process.poll_update() } {
                        Some(ProcessUpdate::Connected(peer)) => {
                            unsafe { (*state).mode = GuiMode::Connected };
                            set_button_label(
                                unsafe { (*state).button },
                                &format!("已连接：{peer}（点击停止）"),
                            );
                        }
                        Some(ProcessUpdate::Stopped) => {
                            unsafe { (*state).mode = GuiMode::Stopped };
                            set_button_label(unsafe { (*state).button }, "连接已停止（点击重试）")
                        }
                        None => {}
                    }
                }
                0
            }
            WM_DESTROY => {
                unsafe { KillTimer(hwnd, STATUS_TIMER_ID) };
                let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State };
                if !state.is_null() {
                    unsafe { (*state).process.stop() };
                }
                unsafe { PostQuitMessage(0) };
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn handle_main_button(window: Hwnd, state: &mut State) {
        match state.mode {
            GuiMode::Connecting | GuiMode::Connected => {
                state.process.stop();
                state.mode = GuiMode::Stopped;
                set_button_label(state.button, "已停止（点击重新监听）");
            }
            GuiMode::Stopped => match state.process.start_listener() {
                Ok(()) => {
                    state.mode = GuiMode::Listening;
                    set_button_label(
                        state.button,
                        &format!("本机匹配码：{}\r\n点击输入对方匹配码", state.pairing_code),
                    );
                }
                Err(_) => set_button_label(state.button, "启动失败（点击重试）"),
            },
            GuiMode::Listening => {
                let Some(code) = prompt_pairing_code(window) else {
                    return;
                };
                match decode_pairing_code(&code) {
                    Ok(peer) => {
                        state.process.stop();
                        match state.process.start_peer(peer) {
                            Ok(()) => {
                                state.mode = GuiMode::Connecting;
                                set_button_label(
                                    state.button,
                                    &format!("正在连接 {peer}（点击停止）"),
                                );
                            }
                            Err(_) => {
                                state.mode = GuiMode::Stopped;
                                set_button_label(state.button, "启动失败（点击重试）");
                            }
                        }
                    }
                    Err(error) => set_button_label(state.button, error),
                }
            }
        }
    }

    fn prompt_pairing_code(parent: Hwnd) -> Option<String> {
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let state = Box::into_raw(Box::new(PairDialogState {
                edit: ptr::null_mut(),
                result: None,
            }));
            EnableWindow(parent, 0);
            let dialog = CreateWindowExW(
                1,
                wide("ClipSyncPairDialog").as_ptr(),
                wide("输入匹配码").as_ptr(),
                WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                360,
                170,
                parent,
                ptr::null_mut(),
                instance,
                state.cast(),
            );
            if dialog.is_null() {
                EnableWindow(parent, 1);
                drop(Box::from_raw(state));
                return None;
            }

            let mut message: Message = mem::zeroed();
            while IsWindow(dialog) != 0 {
                let result = GetMessageW(&mut message, ptr::null_mut(), 0, 0);
                if result <= 0 {
                    if result == 0 {
                        PostQuitMessage(0);
                    }
                    break;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            EnableWindow(parent, 1);
            SetForegroundWindow(parent);
            Box::from_raw(state).result
        }
    }

    unsafe extern "system" fn pair_dialog_proc(
        hwnd: Hwnd,
        message: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult {
        match message {
            WM_CREATE => {
                let create = unsafe { &*(lparam as *const CreateStructW) };
                let state = create.create_params.cast::<PairDialogState>();
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize) };
                let static_class = wide("STATIC");
                let edit_class = wide("EDIT");
                let button_class = wide("BUTTON");
                unsafe {
                    CreateWindowExW(
                        0,
                        static_class.as_ptr(),
                        wide("请输入另一台设备显示的 8 位匹配码：").as_ptr(),
                        WS_CHILD | WS_VISIBLE,
                        18,
                        14,
                        315,
                        22,
                        hwnd,
                        ptr::null_mut(),
                        create.instance,
                        ptr::null_mut(),
                    );
                    let edit = CreateWindowExW(
                        0,
                        edit_class.as_ptr(),
                        wide("").as_ptr(),
                        WS_CHILD | WS_VISIBLE | WS_BORDER | ES_AUTOHSCROLL,
                        18,
                        42,
                        315,
                        28,
                        hwnd,
                        ptr::null_mut(),
                        create.instance,
                        ptr::null_mut(),
                    );
                    (*state).edit = edit;
                    CreateWindowExW(
                        0,
                        button_class.as_ptr(),
                        wide("连接").as_ptr(),
                        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                        166,
                        82,
                        80,
                        30,
                        hwnd,
                        CONNECT_ID as *mut c_void,
                        create.instance,
                        ptr::null_mut(),
                    );
                    CreateWindowExW(
                        0,
                        button_class.as_ptr(),
                        wide("取消").as_ptr(),
                        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                        253,
                        82,
                        80,
                        30,
                        hwnd,
                        CANCEL_ID as *mut c_void,
                        create.instance,
                        ptr::null_mut(),
                    );
                }
                0
            }
            WM_COMMAND if wparam & 0xffff == CONNECT_ID => {
                let state =
                    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PairDialogState };
                if !state.is_null() {
                    let length = unsafe { GetWindowTextLengthW((*state).edit) };
                    let mut text = vec![0_u16; length.max(0) as usize + 1];
                    unsafe {
                        GetWindowTextW((*state).edit, text.as_mut_ptr(), text.len() as i32);
                        (*state).result = Some(String::from_utf16_lossy(&text[..length as usize]));
                        DestroyWindow(hwnd);
                    }
                }
                0
            }
            WM_COMMAND if wparam & 0xffff == CANCEL_ID => {
                unsafe { DestroyWindow(hwnd) };
                0
            }
            WM_CLOSE => {
                unsafe { DestroyWindow(hwnd) };
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn set_button_label(button: Hwnd, label: &str) {
        let label = wide(label);
        unsafe { SetWindowTextW(button, label.as_ptr()) };
    }
}
