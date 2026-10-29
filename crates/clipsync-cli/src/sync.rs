use std::{
    env,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread,
    time::Duration,
};

use clipsync_core::{MAX_CLIPBOARD_TEXT_BYTES, SyncMessage};
use uuid::Uuid;

use crate::clipboard;
#[cfg(target_os = "linux")]
use crate::gnome_extension;

const DEFAULT_SYNC_PORT: u16 = 24_872;
const MAX_FRAME_BYTES: usize = MAX_CLIPBOARD_TEXT_BYTES + 4_096;
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(350);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug)]
struct Config {
    device_name: String,
    peer: Option<SocketAddr>,
    listen_port: u16,
}

pub fn run(args: impl IntoIterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args(args)?;
    let hello = SyncMessage::hello(Uuid::new_v4(), &config.device_name, env::consts::OS)?;
    let (outgoing_sender, outgoing_receiver) = mpsc::sync_channel(1);
    let (incoming_sender, incoming_receiver) = mpsc::sync_channel(1);
    let (local_sender, local_receiver) = mpsc::sync_channel(4);
    let remote_write = Arc::new(Mutex::new(None));

    #[cfg(target_os = "linux")]
    let _gnome_bridge = start_linux_clipboard_monitor(local_sender)?;
    #[cfg(target_os = "windows")]
    thread::Builder::new()
        .name("clipsync-clipboard-watch".to_owned())
        .spawn(move || clipboard_watch_loop(local_sender))?;

    let event_remote_write = Arc::clone(&remote_write);
    thread::Builder::new()
        .name("clipsync-clipboard-events".to_owned())
        .spawn(move || clipboard_event_loop(local_receiver, outgoing_sender, event_remote_write))?;
    thread::Builder::new()
        .name("clipsync-clipboard-write".to_owned())
        .spawn(move || clipboard_write_loop(incoming_receiver, remote_write))?;

    println!("ClipSync text synchronization (protocol v1)");
    println!("Local device: {}", config.device_name);
    println!("Warning: phase-2 transport is not encrypted; use only on a trusted network.");

    if let Some(peer) = config.peer {
        println!("Connecting to {peer}; press Ctrl+C to stop.");
        client_loop(peer, &hello, &outgoing_receiver, &incoming_sender)
    } else {
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), config.listen_port);
        let listener = TcpListener::bind(address)?;
        println!("Listening on TCP {address}; press Ctrl+C to stop.");
        server_loop(listener, &hello, &outgoing_receiver, &incoming_sender)
    }
}

fn clipboard_watch_loop(local_updates: SyncSender<String>) {
    let mut last_observed = match clipboard::read_text() {
        Ok(text) => text,
        Err(error) => {
            eprintln!("Clipboard is unavailable: {error}");
            return;
        }
    };

    loop {
        match clipboard::read_text() {
            Ok(Some(text)) if last_observed.as_ref() != Some(&text) => {
                last_observed = Some(text.clone());
                match local_updates.try_send(text) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(_)) => {
                        eprintln!("! Clipboard event queue is busy; update dropped")
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => return,
                }
            }
            Ok(text) => last_observed = text,
            Err(error) => eprintln!("! Could not read clipboard: {error}"),
        }
        thread::sleep(CLIPBOARD_POLL_INTERVAL);
    }
}

fn clipboard_event_loop(
    local_updates: Receiver<String>,
    outgoing: SyncSender<SyncMessage>,
    remote_write: Arc<Mutex<Option<String>>>,
) {
    while let Ok(text) = local_updates.recv() {
        let suppress = remote_write.lock().unwrap().take();
        if suppress.as_ref() == Some(&text) {
            continue;
        }
        match SyncMessage::clipboard_text(text) {
            Ok(message) => match outgoing.try_send(message) {
                Ok(()) => println!("-> Local clipboard changed"),
                Err(mpsc::TrySendError::Full(_)) => {
                    eprintln!("! Connection is busy; clipboard change dropped")
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return,
            },
            Err(error) => eprintln!("! Clipboard change was not sent: {error}"),
        }
    }
}

#[cfg(target_os = "linux")]
fn start_linux_clipboard_monitor(
    local_updates: SyncSender<String>,
) -> Result<Option<zbus::blocking::Connection>, Box<dyn std::error::Error>> {
    if is_gnome_wayland() {
        if !gnome_extension::is_enabled() {
            return Err(
                "GNOME Wayland requires the ClipSync Shell extension. Run `clipsync setup-gnome`, log out and back in, then run `gnome-extensions enable clipsync@local`."
                    .into(),
            );
        }
        let bridge = ClipboardBridge { local_updates };
        let connection = zbus::blocking::connection::Builder::session()?
            .name("org.clipsync.Daemon")?
            .serve_at("/org/clipsync/Daemon", bridge)?
            .build()?;
        println!("Clipboard monitor: GNOME Shell event bridge (no polling)");
        Ok(Some(connection))
    } else if env::var_os("WAYLAND_DISPLAY").is_some() {
        thread::Builder::new()
            .name("clipsync-wayland-watch".to_owned())
            .spawn(move || {
                if let Err(error) = clipboard::watch_wayland_text(local_updates) {
                    eprintln!("Wayland clipboard monitor stopped: {error}");
                }
            })?;
        println!("Clipboard monitor: persistent Wayland event watcher");
        Ok(None)
    } else {
        thread::Builder::new()
            .name("clipsync-clipboard-watch".to_owned())
            .spawn(move || clipboard_watch_loop(local_updates))?;
        println!("Clipboard monitor: platform polling fallback");
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
fn is_gnome_wayland() -> bool {
    if env::var_os("WAYLAND_DISPLAY").is_none() {
        return false;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .any(|desktop| desktop.eq_ignore_ascii_case("gnome"))
}

#[cfg(target_os = "linux")]
struct ClipboardBridge {
    local_updates: SyncSender<String>,
}

#[cfg(target_os = "linux")]
#[zbus::interface(name = "org.clipsync.Daemon")]
impl ClipboardBridge {
    fn clipboard_text(&self, text: String) {
        if text.is_empty() || text.len() > MAX_CLIPBOARD_TEXT_BYTES {
            return;
        }
        match self.local_updates.try_send(text) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                eprintln!("! GNOME clipboard event queue is busy; update dropped")
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {}
        }
    }
}

fn clipboard_write_loop(incoming: Receiver<String>, remote_write: Arc<Mutex<Option<String>>>) {
    while let Ok(text) = incoming.recv() {
        *remote_write.lock().unwrap() = Some(text.clone());
        match clipboard::write_text(&text) {
            Ok(()) => println!("<- Applied remote text to the clipboard"),
            Err(error) => {
                remote_write.lock().unwrap().take();
                eprintln!("Could not write remote clipboard: {error}");
            }
        }
    }
}

fn client_loop(
    peer: SocketAddr,
    hello: &SyncMessage,
    outgoing: &Receiver<SyncMessage>,
    incoming: &SyncSender<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        match TcpStream::connect_timeout(&peer, Duration::from_secs(3)) {
            Ok(stream) => {
                println!("Connected to {peer}");
                if let Err(error) = handle_connection(stream, hello, outgoing, incoming) {
                    eprintln!("Connection ended: {error}");
                }
            }
            Err(error) => eprintln!("Waiting for {peer}: {error}"),
        }
        thread::sleep(RECONNECT_INTERVAL);
    }
}

fn server_loop(
    listener: TcpListener,
    hello: &SyncMessage,
    outgoing: &Receiver<SyncMessage>,
    incoming: &SyncSender<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        let (stream, address) = listener.accept()?;
        println!("Connection from {address}");
        if let Err(error) = handle_connection(stream, hello, outgoing, incoming) {
            eprintln!("Connection ended: {error}");
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    hello: &SyncMessage,
    outgoing: &Receiver<SyncMessage>,
    incoming: &SyncSender<String>,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    write_frame(&mut stream, hello)?;

    let disconnected = Arc::new(AtomicBool::new(false));
    let reader_disconnected = Arc::clone(&disconnected);
    let mut reader = stream.try_clone()?;
    let incoming = incoming.clone();
    let reader_thread = thread::spawn(move || {
        let result = read_messages(&mut reader, &incoming);
        if let Err(error) = &result {
            eprintln!("Reader stopped: {error}");
        }
        reader_disconnected.store(true, Ordering::Release);
        result
    });

    while !disconnected.load(Ordering::Acquire) {
        match outgoing.recv_timeout(Duration::from_millis(200)) {
            Ok(message) => {
                write_frame(&mut stream, &message)?;
                println!("=> Sent clipboard update to peer");
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = reader_thread.join();
    Ok(())
}

fn read_messages(stream: &mut TcpStream, incoming: &SyncSender<String>) -> io::Result<()> {
    let mut received_hello = false;
    loop {
        match read_frame(stream)? {
            SyncMessage::Hello {
                device_name,
                platform,
                ..
            } => {
                if received_hello {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "duplicate hello",
                    ));
                }
                received_hello = true;
                println!("Transport peer: {device_name} [{platform}]");
            }
            SyncMessage::ClipboardText { text, .. } => {
                if !received_hello {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "clipboard message received before hello",
                    ));
                }
                match incoming.try_send(text) {
                    Ok(()) => println!("<= Received clipboard update from peer"),
                    Err(mpsc::TrySendError::Full(_)) => {
                        eprintln!("! Incoming clipboard queue is busy; update dropped")
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => return Ok(()),
                }
            }
        }
    }
}

fn write_frame(writer: &mut impl Write, message: &SyncMessage) -> io::Result<()> {
    let payload = message
        .encode()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let length = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

fn read_frame(reader: &mut impl Read) -> io::Result<SyncMessage> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid sync frame length",
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    SyncMessage::decode(&payload).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut args = args.into_iter();
    let device_name = args.next().ok_or_else(usage)?;
    let mut peer = None;
    let mut listen_port = DEFAULT_SYNC_PORT;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--peer" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--peer requires an IPv4 address".to_owned())?;
                peer = Some(parse_peer(&value)?);
            }
            "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--port requires a port number".to_owned())?;
                listen_port = value
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(|| format!("invalid port `{value}`"))?;
            }
            unknown => return Err(format!("unknown sync option `{unknown}`\n{}", usage())),
        }
    }
    Ok(Config {
        device_name,
        peer,
        listen_port,
    })
}

fn parse_peer(value: &str) -> Result<SocketAddr, String> {
    if let Ok(address) = SocketAddr::from_str(value) {
        return Ok(address);
    }
    let ip = Ipv4Addr::from_str(value)
        .map_err(|_| format!("invalid peer `{value}`; expected IPv4 or IPv4:port"))?;
    Ok(SocketAddr::new(IpAddr::V4(ip), DEFAULT_SYNC_PORT))
}

fn usage() -> String {
    "usage: clipsync sync <device-name> [--peer <IPv4[:port]>] [--port <port>]".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_preserves_multiline_text() {
        let original = SyncMessage::clipboard_text("line 1\nline 2\n中文".to_owned()).unwrap();
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &original).unwrap();
        assert_eq!(read_frame(&mut bytes.as_slice()).unwrap(), original);
    }

    #[test]
    fn rejects_oversized_frame_before_allocating() {
        let bytes = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes();
        let error = read_frame(&mut bytes.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn parses_sync_peer_with_default_port() {
        let config = parse_args([
            "Desk".to_owned(),
            "--peer".to_owned(),
            "10.0.0.8".to_owned(),
        ])
        .unwrap();
        assert_eq!(config.peer, Some("10.0.0.8:24872".parse().unwrap()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires access to the desktop session D-Bus"]
    fn gnome_dbus_bridge_delivers_clipboard_text() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let service = format!("org.clipsync.Test{}", std::process::id());
        let _server = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name(service.as_str())
            .unwrap()
            .serve_at(
                "/org/clipsync/Daemon",
                ClipboardBridge {
                    local_updates: sender,
                },
            )
            .unwrap()
            .build()
            .unwrap();
        let client = zbus::blocking::Connection::session().unwrap();
        let proxy = zbus::blocking::Proxy::new(
            &client,
            service.as_str(),
            "/org/clipsync/Daemon",
            "org.clipsync.Daemon",
        )
        .unwrap();
        let _: () = proxy
            .call("ClipboardText", &("event-driven text",))
            .unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            "event-driven text"
        );
    }
}
