use std::io;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    pub fn read_text() -> io::Result<Option<String>> {
        let (program, arguments): (&str, &[&str]) = if std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            ("wl-paste", &["--no-newline", "--type", "text"])
        } else if command_exists("xclip") {
            ("xclip", &["-selection", "clipboard", "-out"])
        } else {
            ("xsel", &["--clipboard", "--output"])
        };

        let output = Command::new(program).args(arguments).output().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "cannot run {program}; install wl-clipboard (Wayland) or xclip/xsel (X11): {error}"
                ),
            )
        })?;
        if !output.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Some(text))
    }

    pub fn write_text(text: &str) -> io::Result<()> {
        let (program, arguments): (&str, &[&str]) = if std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            ("wl-copy", &["--type", "text/plain;charset=utf-8"])
        } else if command_exists("xclip") {
            ("xclip", &["-selection", "clipboard", "-in"])
        } else {
            ("xsel", &["--clipboard", "--input"])
        };

        let mut child = Command::new(program)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            // wl-copy forks a persistent data-serving child. A piped stderr is
            // inherited by that child and would make wait_with_output hang.
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("cannot run clipboard writer {program}: {error}"),
                )
            })?;
        child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("clipboard writer has no stdin"))?
            .write_all(text.as_bytes())?;
        let status = child.wait()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("{program} failed with {status}")))
        }
    }

    fn command_exists(program: &str) -> bool {
        Command::new(program)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }
}

#[cfg(target_os = "linux")]
pub use linux::{read_text, write_text};

#[cfg(target_os = "windows")]
mod windows_native {
    use super::*;
    use std::{ffi::c_void, ptr, thread, time::Duration};

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn SetClipboardData(format: u32, memory: *mut c_void) -> *mut c_void;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalFree(memory: *mut c_void) -> *mut c_void;
        fn GlobalLock(memory: *mut c_void) -> *mut c_void;
        fn GlobalSize(memory: *mut c_void) -> usize;
        fn GlobalUnlock(memory: *mut c_void) -> i32;
    }

    struct ClipboardGuard;

    impl ClipboardGuard {
        fn open() -> io::Result<Self> {
            for _ in 0..10 {
                // SAFETY: A null owner is permitted. The guard closes a successful open.
                if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
                    return Ok(Self);
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(io::Error::last_os_error())
        }
    }

    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            // SAFETY: This guard exists only after OpenClipboard succeeded.
            unsafe { CloseClipboard() };
        }
    }

    pub fn read_text() -> io::Result<Option<String>> {
        // SAFETY: This call has no pointer preconditions.
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
            return Ok(None);
        }
        let _guard = ClipboardGuard::open()?;
        // SAFETY: The clipboard is open and the requested format was advertised.
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Clipboard global-memory handles may be locked for read access.
        let pointer = unsafe { GlobalLock(handle) }.cast::<u16>();
        if pointer.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `handle` is the global-memory handle returned for CF_UNICODETEXT.
        let units = unsafe { GlobalSize(handle) } / size_of::<u16>();
        // SAFETY: The slice is bounded by the allocation size, even if a broken
        // clipboard owner did not provide the required NUL terminator.
        let contents = unsafe { std::slice::from_raw_parts(pointer, units) };
        let length = contents.iter().position(|unit| *unit == 0).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "clipboard text is not terminated",
            )
        })?;
        let text = String::from_utf16_lossy(&contents[..length]);
        // SAFETY: `pointer` was returned by GlobalLock for this handle.
        unsafe { GlobalUnlock(handle) };
        Ok(Some(text))
    }

    pub fn write_text(text: &str) -> io::Result<()> {
        let utf16: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let bytes = utf16.len() * size_of::<u16>();
        // SAFETY: Allocation size is derived from a live vector.
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
        if memory.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Newly allocated movable global memory may be locked for writing.
        let pointer = unsafe { GlobalLock(memory) }.cast::<u16>();
        if pointer.is_null() {
            unsafe { GlobalFree(memory) };
            return Err(io::Error::last_os_error());
        }
        // SAFETY: The allocation is exactly large enough for all UTF-16 units.
        unsafe { ptr::copy_nonoverlapping(utf16.as_ptr(), pointer, utf16.len()) };
        unsafe { GlobalUnlock(memory) };

        let _guard = match ClipboardGuard::open() {
            Ok(guard) => guard,
            Err(error) => {
                unsafe { GlobalFree(memory) };
                return Err(error);
            }
        };
        // SAFETY: Clipboard is open. On success ownership transfers to the system.
        unsafe { EmptyClipboard() };
        if unsafe { SetClipboardData(CF_UNICODETEXT, memory) }.is_null() {
            unsafe { GlobalFree(memory) };
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub use windows_native::{read_text, write_text};

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("ClipSync currently supports only Windows and Linux");
