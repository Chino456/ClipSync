use std::{
    io,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

struct SyncProcess {
    arguments: Vec<String>,
    child: Option<Child>,
}

impl SyncProcess {
    fn new(arguments: Vec<String>) -> Self {
        Self {
            arguments,
            child: None,
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
                false
            }
        }
    }

    fn start(&mut self) -> io::Result<()> {
        if self.is_running() {
            return Ok(());
        }
        if self.arguments.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: clipsync-gui <device-name> [--peer <IPv4[:port]>] [--port <port>]",
            ));
        }

        let mut command = Command::new(sync_executable()?);
        command
            .arg("sync")
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
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
        self.child = Some(command.spawn()?);
        Ok(())
    }

    fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
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

    fn toggle(&mut self) -> io::Result<bool> {
        if self.is_running() {
            self.stop();
            Ok(false)
        } else {
            self.start()?;
            Ok(true)
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

#[cfg(unix)]
const SIGTERM: i32 = 15;

#[cfg(unix)]
#[link(name = "c")]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(target_os = "linux")]
pub fn run(arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    linux::run(arguments)
}

#[cfg(target_os = "windows")]
pub fn run(arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    windows::run(arguments)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn run(_arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    Err("ClipSync GUI currently supports only Windows and Linux".into())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use libloading::Library;
    use std::{ffi::CString, os::raw::c_char, ptr, sync::OnceLock};

    type Widget = *mut std::ffi::c_void;
    type SignalCallback = unsafe extern "C" fn();

    struct GtkApi {
        gtk_init: unsafe extern "C" fn(*mut i32, *mut *mut *mut c_char),
        gtk_main: unsafe extern "C" fn(),
        gtk_main_quit: unsafe extern "C" fn(),
        gtk_window_new: unsafe extern "C" fn(i32) -> Widget,
        gtk_window_set_title: unsafe extern "C" fn(Widget, *const c_char),
        gtk_window_set_default_size: unsafe extern "C" fn(Widget, i32, i32),
        gtk_button_new_with_label: unsafe extern "C" fn(*const c_char) -> Widget,
        gtk_button_set_label: unsafe extern "C" fn(Widget, *const c_char),
        gtk_container_add: unsafe extern "C" fn(Widget, Widget),
        gtk_widget_show_all: unsafe extern "C" fn(Widget),
        g_signal_connect_data: unsafe extern "C" fn(
            Widget,
            *const c_char,
            Option<SignalCallback>,
            *mut std::ffi::c_void,
            Option<unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void)>,
            u32,
        ) -> u64,
    }

    static GTK: OnceLock<GtkApi> = OnceLock::new();

    struct State {
        process: SyncProcess,
        button: Widget,
    }

    pub fn run(arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
        let api = load_gtk()?;
        GTK.set(api).map_err(|_| "GTK is already initialized")?;
        let api = GTK.get().unwrap();

        unsafe {
            (api.gtk_init)(ptr::null_mut(), ptr::null_mut());
            let window = (api.gtk_window_new)(0);
            let title = CString::new("ClipSync")?;
            (api.gtk_window_set_title)(window, title.as_ptr());
            (api.gtk_window_set_default_size)(window, 300, 120);

            let start = CString::new("开始连接")?;
            let button = (api.gtk_button_new_with_label)(start.as_ptr());
            (api.gtk_container_add)(window, button);

            let state = Box::new(State {
                process: SyncProcess::new(arguments),
                button,
            });
            let state = Box::into_raw(state);
            connect_signal(button, "clicked", on_clicked, state.cast())?;
            connect_signal(window, "destroy", on_destroy, state.cast())?;
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
        let label = match state.process.toggle() {
            Ok(true) => "停止连接",
            Ok(false) => "开始连接",
            Err(_) => "启动失败",
        };
        if let Ok(label) = CString::new(label) {
            unsafe { (GTK.get().unwrap().gtk_button_set_label)(state.button, label.as_ptr()) };
        }
    }

    unsafe extern "C" fn on_destroy(_window: Widget, state: *mut State) {
        unsafe { (&mut *state).process.stop() };
        unsafe { (GTK.get().unwrap().gtk_main_quit)() };
    }

    fn load_gtk() -> Result<GtkApi, Box<dyn std::error::Error>> {
        let gtk = Box::leak(Box::new(unsafe { Library::new("libgtk-3.so.0")? }));
        let gobject = Box::leak(Box::new(unsafe { Library::new("libgobject-2.0.so.0")? }));
        unsafe {
            Ok(GtkApi {
                gtk_init: *gtk.get(b"gtk_init\0")?,
                gtk_main: *gtk.get(b"gtk_main\0")?,
                gtk_main_quit: *gtk.get(b"gtk_main_quit\0")?,
                gtk_window_new: *gtk.get(b"gtk_window_new\0")?,
                gtk_window_set_title: *gtk.get(b"gtk_window_set_title\0")?,
                gtk_window_set_default_size: *gtk.get(b"gtk_window_set_default_size\0")?,
                gtk_button_new_with_label: *gtk.get(b"gtk_button_new_with_label\0")?,
                gtk_button_set_label: *gtk.get(b"gtk_button_set_label\0")?,
                gtk_container_add: *gtk.get(b"gtk_container_add\0")?,
                gtk_widget_show_all: *gtk.get(b"gtk_widget_show_all\0")?,
                g_signal_connect_data: *gobject.get(b"g_signal_connect_data\0")?,
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
    const WM_COMMAND: u32 = 0x0111;
    const GWLP_USERDATA: i32 = -21;
    const WS_OVERLAPPED: u32 = 0x00000000;
    const WS_CAPTION: u32 = 0x00C00000;
    const WS_SYSMENU: u32 = 0x00080000;
    const WS_MINIMIZEBOX: u32 = 0x00020000;
    const WS_VISIBLE: u32 = 0x10000000;
    const WS_CHILD: u32 = 0x40000000;
    const BS_PUSHBUTTON: u32 = 0x00000000;
    const CW_USEDEFAULT: i32 = i32::MIN;
    const BUTTON_ID: usize = 1001;

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
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(module_name: *const u16) -> Hinstance;
    }

    pub fn run(arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
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

            let state = Box::into_raw(Box::new(State {
                process: SyncProcess::new(arguments),
                button: ptr::null_mut(),
            }));
            let title = wide("ClipSync");
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                320,
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
                let label = wide("开始连接");
                let button = unsafe {
                    CreateWindowExW(
                        0,
                        button_class.as_ptr(),
                        label.as_ptr(),
                        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                        60,
                        35,
                        180,
                        50,
                        hwnd,
                        BUTTON_ID as *mut c_void,
                        create.instance,
                        ptr::null_mut(),
                    )
                };
                unsafe { (*state).button = button };
                0
            }
            WM_COMMAND if wparam & 0xffff == BUTTON_ID => {
                let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State };
                if !state.is_null() {
                    let label = match unsafe { (*state).process.toggle() } {
                        Ok(true) => "停止连接",
                        Ok(false) => "开始连接",
                        Err(_) => "启动失败",
                    };
                    let label = wide(label);
                    unsafe { SetWindowTextW((*state).button, label.as_ptr()) };
                }
                0
            }
            WM_DESTROY => {
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

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}
