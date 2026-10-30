use std::io;

use clipsync_core::MAX_CLIPBOARD_IMAGE_BYTES;
#[cfg(target_os = "linux")]
use clipsync_core::MAX_CLIPBOARD_TEXT_BYTES;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardContent {
    Text(String),
    ImagePng(Vec<u8>),
}

impl ClipboardContent {
    pub fn description(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::ImagePng(_) => "image",
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        io::{Read, Write},
        process::{Command, Output, Stdio},
        sync::mpsc::{SyncSender, TrySendError},
        thread,
        time::Duration,
    };

    pub fn read_content() -> io::Result<Option<ClipboardContent>> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return read_wayland_content();
        }
        if command_exists("xclip") {
            return read_x11_content();
        }
        read_text_with("xsel", &["--clipboard", "--output"])
    }

    fn read_wayland_content() -> io::Result<Option<ClipboardContent>> {
        let types = command_output("wl-paste", &["--list-types"])?;
        let offered = String::from_utf8_lossy(&types.stdout);
        if types.status.success() && offered.lines().any(is_png_mime) {
            let output = command_output("wl-paste", &["--type", "image/png"])?;
            if output.status.success() && valid_png(&output.stdout) {
                return Ok(Some(ClipboardContent::ImagePng(output.stdout)));
            }
        }
        read_text_with("wl-paste", &["--no-newline", "--type", "text"])
    }

    fn read_x11_content() -> io::Result<Option<ClipboardContent>> {
        let targets = command_output("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"])?;
        let offered = String::from_utf8_lossy(&targets.stdout);
        if targets.status.success() && offered.split_whitespace().any(is_png_mime) {
            let output = command_output(
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"],
            )?;
            if output.status.success() && valid_png(&output.stdout) {
                return Ok(Some(ClipboardContent::ImagePng(output.stdout)));
            }
        }
        read_text_with("xclip", &["-selection", "clipboard", "-out"])
    }

    fn is_png_mime(value: &str) -> bool {
        value.trim().eq_ignore_ascii_case("image/png")
    }

    fn read_text_with(program: &str, arguments: &[&str]) -> io::Result<Option<ClipboardContent>> {
        let output = command_output(program, arguments)?;
        if !output.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Some(ClipboardContent::Text(text)))
    }

    fn command_output(program: &str, arguments: &[&str]) -> io::Result<Output> {
        Command::new(program).args(arguments).output().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "cannot run {program}; install wl-clipboard (Wayland) or xclip/xsel (X11): {error}"
                ),
            )
        })
    }

    pub fn write_content(content: &ClipboardContent) -> io::Result<()> {
        match content {
            ClipboardContent::Text(text) => {
                if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    write_bytes(
                        "wl-copy",
                        &["--type", "text/plain;charset=utf-8"],
                        text.as_bytes(),
                    )
                } else if command_exists("xclip") {
                    write_bytes(
                        "xclip",
                        &["-selection", "clipboard", "-in"],
                        text.as_bytes(),
                    )
                } else {
                    write_bytes("xsel", &["--clipboard", "--input"], text.as_bytes())
                }
            }
            ClipboardContent::ImagePng(png) => {
                if !valid_png(png) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid PNG image",
                    ));
                }
                if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    write_bytes("wl-copy", &["--type", "image/png"], png)
                } else if command_exists("xclip") {
                    write_bytes(
                        "xclip",
                        &["-selection", "clipboard", "-t", "image/png", "-in"],
                        png,
                    )
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "image clipboard on X11 requires xclip",
                    ))
                }
            }
        }
    }

    fn write_bytes(program: &str, arguments: &[&str], bytes: &[u8]) -> io::Result<()> {
        let mut child = Command::new(program)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
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
            .write_all(bytes)?;
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

    pub fn watch_wayland_content(local_updates: SyncSender<ClipboardContent>) -> io::Result<()> {
        let text_updates = local_updates.clone();
        thread::Builder::new()
            .name("clipsync-wayland-text-watch".to_owned())
            .spawn(move || {
                if let Err(error) = watch_wayland_kind("text", text_updates) {
                    eprintln!("Wayland text watcher stopped: {error}");
                }
            })?;
        watch_wayland_kind("image", local_updates)
    }

    fn watch_wayland_kind(
        kind: &'static str,
        local_updates: SyncSender<ClipboardContent>,
    ) -> io::Result<()> {
        let executable = std::env::current_exe()?;
        let mut last_observed = read_content()?.filter(|content| content_kind(content) == kind);

        loop {
            let mime = if kind == "image" { "image/png" } else { "text" };
            let mut child = Command::new("wl-paste")
                .args(["--type", mime, "--watch"])
                .arg(&executable)
                .arg("__clipboard-frame")
                .arg(kind)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!("cannot start wl-paste --watch for {kind}: {error}"),
                    )
                })?;
            let mut output = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("wl-paste watcher has no stdout"))?;

            loop {
                let mut length = [0_u8; 4];
                if output.read_exact(&mut length).is_err() {
                    break;
                }
                let length = u32::from_be_bytes(length) as usize;
                let limit = if kind == "image" {
                    MAX_CLIPBOARD_IMAGE_BYTES
                } else {
                    MAX_CLIPBOARD_TEXT_BYTES
                };
                if length > limit {
                    break;
                }
                let mut bytes = vec![0_u8; length];
                output.read_exact(&mut bytes)?;
                let content = if kind == "image" {
                    if !valid_png(&bytes) {
                        continue;
                    }
                    ClipboardContent::ImagePng(bytes)
                } else {
                    let text = String::from_utf8(bytes)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    ClipboardContent::Text(text)
                };
                if last_observed.as_ref() == Some(&content) {
                    continue;
                }
                last_observed = Some(content.clone());
                match local_updates.try_send(content) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        eprintln!("! Clipboard event queue is busy; update dropped")
                    }
                    Err(TrySendError::Disconnected(_)) => return Ok(()),
                }
            }

            let _ = child.kill();
            let _ = child.wait();
            eprintln!("! Wayland {kind} watcher exited; restarting in 2 seconds");
            thread::sleep(Duration::from_secs(2));
        }
    }

    fn content_kind(content: &ClipboardContent) -> &'static str {
        match content {
            ClipboardContent::Text(_) => "text",
            ClipboardContent::ImagePng(_) => "image",
        }
    }

    pub fn run_frame_helper(kind: &str) -> Result<(), Box<dyn std::error::Error>> {
        let limit = match kind {
            "text" => MAX_CLIPBOARD_TEXT_BYTES,
            "image" => MAX_CLIPBOARD_IMAGE_BYTES,
            _ => return Ok(()),
        };
        let mut bytes = Vec::new();
        io::stdin()
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit
            || (kind == "text" && std::str::from_utf8(&bytes).is_err())
            || (kind == "image" && !valid_png(&bytes))
        {
            return Ok(());
        }
        let length = u32::try_from(bytes.len())?;
        let mut stdout = io::stdout().lock();
        stdout.write_all(&length.to_be_bytes())?;
        stdout.write_all(&bytes)?;
        stdout.flush()?;
        Ok(())
    }

    fn valid_png(bytes: &[u8]) -> bool {
        !bytes.is_empty()
            && bytes.len() <= MAX_CLIPBOARD_IMAGE_BYTES
            && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
    }
}

#[cfg(target_os = "linux")]
pub use linux::{read_content, run_frame_helper, watch_wayland_content, write_content};

#[cfg(target_os = "windows")]
mod windows_native {
    use super::*;
    use png::{BitDepth, ColorType, Decoder, Encoder, Transformations};
    use std::{ffi::c_void, io::Cursor, ptr, thread, time::Duration};

    const CF_DIB: u32 = 8;
    const CF_UNICODETEXT: u32 = 13;
    const CF_DIBV5: u32 = 17;
    const GMEM_MOVEABLE: u32 = 0x0002;
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    const MAX_IMAGE_PIXELS: usize = 40_000_000;
    const MAX_DIB_BYTES: usize = MAX_IMAGE_PIXELS * 4 + 1_024;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn SetClipboardData(format: u32, memory: *mut c_void) -> *mut c_void;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
        fn RegisterClipboardFormatW(format: *const u16) -> u32;
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
            unsafe { CloseClipboard() };
        }
    }

    pub fn read_content() -> io::Result<Option<ClipboardContent>> {
        let png_format = png_clipboard_format();
        let has_png = png_format != 0 && unsafe { IsClipboardFormatAvailable(png_format) } != 0;
        let has_dibv5 = unsafe { IsClipboardFormatAvailable(CF_DIBV5) } != 0;
        let has_dib = unsafe { IsClipboardFormatAvailable(CF_DIB) } != 0;
        let has_text = unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } != 0;
        if !has_png && !has_dibv5 && !has_dib && !has_text {
            return Ok(None);
        }

        let _guard = ClipboardGuard::open()?;
        if has_png
            && let Ok(png) = read_global_bytes(png_format, MAX_CLIPBOARD_IMAGE_BYTES)
            && valid_png(&png)
        {
            return Ok(Some(ClipboardContent::ImagePng(png)));
        }
        if has_dibv5 || has_dib {
            let format = if has_dibv5 { CF_DIBV5 } else { CF_DIB };
            if let Ok(dib) = read_global_bytes(format, MAX_DIB_BYTES)
                && let Ok(png) = dib_to_png(&dib)
            {
                return Ok(Some(ClipboardContent::ImagePng(png)));
            }
        }
        if has_text {
            return read_text_locked().map(|text| Some(ClipboardContent::Text(text)));
        }
        Ok(None)
    }

    fn read_text_locked() -> io::Result<String> {
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let pointer = unsafe { GlobalLock(handle) }.cast::<u16>();
        if pointer.is_null() {
            return Err(io::Error::last_os_error());
        }
        let units = unsafe { GlobalSize(handle) } / size_of::<u16>();
        let contents = unsafe { std::slice::from_raw_parts(pointer, units) };
        let length = contents.iter().position(|unit| *unit == 0);
        let text = length.map(|length| String::from_utf16_lossy(&contents[..length]));
        unsafe { GlobalUnlock(handle) };
        text.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "clipboard text is not terminated",
            )
        })
    }

    fn read_global_bytes(format: u32, limit: usize) -> io::Result<Vec<u8>> {
        let handle = unsafe { GetClipboardData(format) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let size = unsafe { GlobalSize(handle) };
        if size == 0 || size > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "clipboard image exceeds the supported size",
            ));
        }
        let pointer = unsafe { GlobalLock(handle) }.cast::<u8>();
        if pointer.is_null() {
            return Err(io::Error::last_os_error());
        }
        let bytes = unsafe { std::slice::from_raw_parts(pointer, size) }.to_vec();
        unsafe { GlobalUnlock(handle) };
        Ok(bytes)
    }

    pub fn write_content(content: &ClipboardContent) -> io::Result<()> {
        match content {
            ClipboardContent::Text(text) => write_text(text),
            ClipboardContent::ImagePng(png) => write_png(png),
        }
    }

    fn write_text(text: &str) -> io::Result<()> {
        let utf16: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let bytes = unsafe {
            std::slice::from_raw_parts(utf16.as_ptr().cast::<u8>(), utf16.len() * size_of::<u16>())
        };
        let _guard = ClipboardGuard::open()?;
        let memory = allocate_global(bytes)?;
        unsafe { EmptyClipboard() };
        set_clipboard_memory(CF_UNICODETEXT, memory)
    }

    fn write_png(png: &[u8]) -> io::Result<()> {
        if !valid_png(png) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid PNG image",
            ));
        }
        let dib = png_to_dibv5(png)?;
        let _guard = ClipboardGuard::open()?;
        let png_memory = allocate_global(png)?;
        let dib_memory = match allocate_global(&dib) {
            Ok(memory) => memory,
            Err(error) => {
                unsafe { GlobalFree(png_memory) };
                return Err(error);
            }
        };
        let png_format = png_clipboard_format();
        if png_format == 0 {
            unsafe {
                GlobalFree(png_memory);
                GlobalFree(dib_memory);
            }
            return Err(io::Error::last_os_error());
        }
        unsafe { EmptyClipboard() };
        if let Err(error) = set_clipboard_memory(png_format, png_memory) {
            unsafe { GlobalFree(dib_memory) };
            return Err(error);
        }
        // PNG is the lossless source of truth. CF_DIBV5 is added for apps that
        // do not understand the registered PNG clipboard format.
        let _ = set_clipboard_memory(CF_DIBV5, dib_memory);
        Ok(())
    }

    fn allocate_global(bytes: &[u8]) -> io::Result<*mut c_void> {
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
        if memory.is_null() {
            return Err(io::Error::last_os_error());
        }
        let pointer = unsafe { GlobalLock(memory) }.cast::<u8>();
        if pointer.is_null() {
            unsafe { GlobalFree(memory) };
            return Err(io::Error::last_os_error());
        }
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len()) };
        unsafe { GlobalUnlock(memory) };
        Ok(memory)
    }

    fn set_clipboard_memory(format: u32, memory: *mut c_void) -> io::Result<()> {
        if unsafe { SetClipboardData(format, memory) }.is_null() {
            unsafe { GlobalFree(memory) };
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn png_clipboard_format() -> u32 {
        let name: Vec<u16> = "PNG".encode_utf16().chain(Some(0)).collect();
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    }

    fn png_to_dibv5(png: &[u8]) -> io::Result<Vec<u8>> {
        let mut decoder = Decoder::new(Cursor::new(png));
        decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
        let mut reader = decoder
            .read_info()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let dimensions = reader.info();
        (dimensions.width as usize)
            .checked_mul(dimensions.height as usize)
            .filter(|count| *count <= MAX_IMAGE_PIXELS)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "image dimensions too large")
            })?;
        let mut buffer = vec![0; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut buffer)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let width = info.width as usize;
        let height = info.height as usize;
        let pixels = width
            .checked_mul(height)
            .filter(|count| *count <= MAX_IMAGE_PIXELS)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "image dimensions too large")
            })?;
        let rgba = to_rgba(&buffer[..info.buffer_size()], info.color_type, pixels)?;

        let mut dib = vec![0_u8; 124 + rgba.len()];
        put_u32(&mut dib, 0, 124);
        put_i32(&mut dib, 4, info.width as i32);
        put_i32(&mut dib, 8, -(info.height as i32));
        put_u16(&mut dib, 12, 1);
        put_u16(&mut dib, 14, 32);
        put_u32(&mut dib, 16, BI_BITFIELDS);
        put_u32(&mut dib, 20, rgba.len() as u32);
        put_u32(&mut dib, 40, 0x00ff_0000);
        put_u32(&mut dib, 44, 0x0000_ff00);
        put_u32(&mut dib, 48, 0x0000_00ff);
        put_u32(&mut dib, 52, 0xff00_0000);
        put_u32(&mut dib, 56, 0x7352_4742);
        for (source, target) in rgba.chunks_exact(4).zip(dib[124..].chunks_exact_mut(4)) {
            target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
        }
        Ok(dib)
    }

    fn to_rgba(bytes: &[u8], color: ColorType, pixels: usize) -> io::Result<Vec<u8>> {
        let channels = match color {
            ColorType::Rgba => 4,
            ColorType::Rgb => 3,
            ColorType::GrayscaleAlpha => 2,
            ColorType::Grayscale => 1,
            ColorType::Indexed => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "indexed PNG was not expanded",
                ));
            }
        };
        if bytes.len() != pixels * channels {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid PNG buffer",
            ));
        }
        let mut rgba = Vec::with_capacity(pixels * 4);
        for pixel in bytes.chunks_exact(channels) {
            match color {
                ColorType::Rgba => rgba.extend_from_slice(pixel),
                ColorType::Rgb => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
                ColorType::GrayscaleAlpha => {
                    rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]])
                }
                ColorType::Grayscale => {
                    rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255])
                }
                ColorType::Indexed => unreachable!(),
            }
        }
        Ok(rgba)
    }

    fn dib_to_png(dib: &[u8]) -> io::Result<Vec<u8>> {
        if dib.len() < 40 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "DIB header too short",
            ));
        }
        let header_size = read_u32(dib, 0)? as usize;
        let width = read_i32(dib, 4)?;
        let signed_height = read_i32(dib, 8)?;
        let bit_count = read_u16(dib, 14)?;
        let compression = read_u32(dib, 16)?;
        if header_size < 40 || width <= 0 || signed_height == 0 || !matches!(bit_count, 24 | 32) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported DIB layout",
            ));
        }
        if !matches!(compression, BI_RGB | BI_BITFIELDS) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported DIB compression",
            ));
        }
        let width = width as usize;
        let height = signed_height.unsigned_abs() as usize;
        let pixels = width
            .checked_mul(height)
            .filter(|count| *count <= MAX_IMAGE_PIXELS)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "image dimensions too large")
            })?;
        let row_stride = (width * bit_count as usize).div_ceil(32) * 4;
        let mask_bytes = usize::from(header_size == 40 && compression == BI_BITFIELDS) * 12;
        let pixel_offset = header_size
            .checked_add(mask_bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid DIB offset"))?;
        pixel_offset
            .checked_add(row_stride * height)
            .filter(|required| *required <= dib.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated DIB"))?;
        let bytes_per_pixel = usize::from(bit_count / 8);
        let mut rgba = Vec::with_capacity(pixels * 4);
        let mut has_nonzero_alpha = false;
        for y in 0..height {
            let source_y = if signed_height > 0 { height - 1 - y } else { y };
            let row = pixel_offset + source_y * row_stride;
            for x in 0..width {
                let offset = row + x * bytes_per_pixel;
                let alpha = if bit_count == 32 {
                    dib[offset + 3]
                } else {
                    255
                };
                has_nonzero_alpha |= alpha != 0;
                rgba.extend_from_slice(&[dib[offset + 2], dib[offset + 1], dib[offset], alpha]);
            }
        }
        if bit_count == 32 && !has_nonzero_alpha {
            for alpha in rgba.iter_mut().skip(3).step_by(4) {
                *alpha = 255;
            }
        }
        encode_png(width as u32, height as u32, &rgba)
    }

    fn encode_png(width: u32, height: u32, rgba: &[u8]) -> io::Result<Vec<u8>> {
        let mut png = Vec::new();
        {
            let mut encoder = Encoder::new(&mut png, width, height);
            encoder.set_color(ColorType::Rgba);
            encoder.set_depth(BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            writer
                .write_image_data(rgba)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        }
        if png.len() > MAX_CLIPBOARD_IMAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encoded PNG exceeds the 10 MiB limit",
            ));
        }
        Ok(png)
    }

    fn valid_png(bytes: &[u8]) -> bool {
        !bytes.is_empty()
            && bytes.len() <= MAX_CLIPBOARD_IMAGE_BYTES
            && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
    }

    fn read_u16(bytes: &[u8], offset: usize) -> io::Result<u16> {
        bytes
            .get(offset..offset + 2)
            .and_then(|value| value.try_into().ok())
            .map(u16::from_le_bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated DIB"))
    }

    fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
        bytes
            .get(offset..offset + 4)
            .and_then(|value| value.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated DIB"))
    }

    fn read_i32(bytes: &[u8], offset: usize) -> io::Result<i32> {
        read_u32(bytes, offset).map(|value| value as i32)
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_i32(bytes: &mut [u8], offset: usize, value: i32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}

#[cfg(target_os = "windows")]
pub use windows_native::{read_content, write_content};

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("ClipSync currently supports only Windows and Linux");
