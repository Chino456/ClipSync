use std::{
    collections::VecDeque,
    fs,
    io::{self, BufRead, BufReader, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

const CONTROL_ADDRESS: &str = "127.0.0.1:24873";
const AUTO_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SAVED_PEERS: usize = 5;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub phase: String,
    pub peer_name: Option<String>,
    pub message: Option<String>,
}

impl ServiceStatus {
    pub fn listening() -> Self {
        Self {
            phase: "listening".to_owned(),
            peer_name: None,
            message: None,
        }
    }

    fn connecting(ip: Ipv4Addr, show_progress: bool) -> Self {
        Self {
            phase: "connecting".to_owned(),
            peer_name: None,
            message: show_progress.then(|| format!("正在连接 {ip}")),
        }
    }

    fn connected(peer_name: String) -> Self {
        Self {
            phase: "connected".to_owned(),
            peer_name: Some(peer_name),
            message: None,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ControlRequest {
    Status,
    Connect { ip: Ipv4Addr },
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedConfig {
    successful_peers: Vec<Ipv4Addr>,
}

struct ServiceManager {
    child: Option<Child>,
    status: Arc<Mutex<ServiceStatus>>,
    generation: Arc<AtomicU64>,
    config_path: PathBuf,
}

impl ServiceManager {
    fn new(config_path: PathBuf) -> Self {
        Self {
            child: None,
            status: Arc::new(Mutex::new(ServiceStatus::listening())),
            generation: Arc::new(AtomicU64::new(0)),
            config_path,
        }
    }

    fn start_listener(&mut self) -> io::Result<()> {
        self.start(None, false)
    }

    fn connect(&mut self, peer: Ipv4Addr) -> io::Result<()> {
        self.start(Some(peer), true)
    }

    fn connect_in_background(&mut self, peer: Ipv4Addr) -> io::Result<()> {
        self.start(Some(peer), false)
    }

    fn start(&mut self, peer: Option<Ipv4Addr>, show_progress: bool) -> io::Result<()> {
        self.stop();
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *self.status.lock().unwrap() = peer.map_or_else(ServiceStatus::listening, |ip| {
            ServiceStatus::connecting(ip, show_progress)
        });

        let mut command = Command::new(sync_executable()?);
        command
            .arg("sync")
            .arg(device_name())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(peer) = peer {
            command.arg("--peer").arg(peer.to_string());
        }
        hide_process(&mut command);

        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("无法读取 ClipSync 服务输出"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("无法读取 ClipSync 服务错误输出"))?;
        let status = Arc::clone(&self.status);
        let active_generation = Arc::clone(&self.generation);
        let config_path = self.config_path.clone();
        thread::spawn(move || {
            let mut connection_ip = peer;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(ip) = parse_incoming_ip(&line) {
                    connection_ip = Some(ip);
                    continue;
                }
                let Some(peer_name) = parse_transport_peer(&line) else {
                    continue;
                };
                if active_generation.load(Ordering::Acquire) != generation {
                    continue;
                }
                *status.lock().unwrap() = ServiceStatus::connected(peer_name.to_owned());
                if let Some(ip) = connection_ip {
                    let _ = remember_successful_peer(&config_path, ip);
                }
            }
        });

        let status = Arc::clone(&self.status);
        let active_generation = Arc::clone(&self.generation);
        let waiting_status = peer.map_or_else(ServiceStatus::listening, |ip| {
            ServiceStatus::connecting(ip, show_progress)
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if line.starts_with("Connection ended:")
                    && active_generation.load(Ordering::Acquire) == generation
                {
                    *status.lock().unwrap() = waiting_status.clone();
                }
            }
        });

        self.child = Some(child);
        Ok(())
    }

    fn status(&mut self) -> ServiceStatus {
        if let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(Some(_)))
        {
            self.child = None;
            *self.status.lock().unwrap() = ServiceStatus::listening();
        }
        self.status.lock().unwrap().clone()
    }

    fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let Some(mut child) = self.child.take() else {
            return;
        };
        terminate_child(&mut child);
    }
}

impl Drop for ServiceManager {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn run() -> io::Result<()> {
    let listener = match TcpListener::bind(CONTROL_ADDRESS) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => return Ok(()),
        Err(error) => return Err(error),
    };
    listener.set_nonblocking(true)?;

    let config_path = config_file()?;
    let mut saved_peers = VecDeque::from(load_config(&config_path).successful_peers);
    let mut manager = ServiceManager::new(config_path);
    let mut auto_deadline = start_next_saved_peer(&mut manager, &mut saved_peers);

    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if handle_control(stream, &mut manager).unwrap_or(false) {
                    saved_peers.clear();
                    auto_deadline = None;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }

        if manager.status().phase == "connected" {
            saved_peers.clear();
            auto_deadline = None;
        } else if auto_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            auto_deadline = start_next_saved_peer(&mut manager, &mut saved_peers);
        }

        if manager.child.is_none() {
            let _ = manager.start_listener();
        }
        thread::sleep(Duration::from_millis(80));
    }
}

pub fn ensure_running() -> io::Result<()> {
    if send_request(&ControlRequest::Status).is_ok() {
        return Ok(());
    }

    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach_process(&mut command);
    command.spawn()?;

    for _ in 0..50 {
        thread::sleep(Duration::from_millis(100));
        if send_request(&ControlRequest::Status).is_ok() {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "ClipSync 后台服务启动超时",
    ))
}

pub fn send_request(request: &ControlRequest) -> io::Result<ServiceStatus> {
    let mut stream = TcpStream::connect_timeout(
        &CONTROL_ADDRESS.parse::<SocketAddr>().unwrap(),
        Duration::from_millis(400),
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    serde_json::to_writer(&mut stream, request).map_err(io::Error::other)?;
    stream.write_all(b"\n")?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    serde_json::from_str(&response).map_err(io::Error::other)
}

pub fn device_name() -> String {
    #[cfg(target_os = "windows")]
    let keys = ["USERNAME", "COMPUTERNAME"];
    #[cfg(not(target_os = "windows"))]
    let keys = ["USER", "LOGNAME"];

    let environment_name = keys.into_iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    });
    if let Some(name) = environment_name {
        return name;
    }

    #[cfg(not(target_os = "windows"))]
    if let Ok(output) = Command::new("id").arg("-un").output()
        && output.status.success()
        && let Ok(name) = String::from_utf8(output.stdout)
        && !name.trim().is_empty()
    {
        return name.trim().to_owned();
    }

    format!("ClipSync {}", std::env::consts::OS)
}

fn handle_control(mut stream: TcpStream, manager: &mut ServiceManager) -> io::Result<bool> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut request)?;
    let request: ControlRequest = serde_json::from_str(&request).map_err(io::Error::other)?;
    let manual_connect = match request {
        ControlRequest::Status => false,
        ControlRequest::Connect { ip } => {
            if manager.connect(ip).is_err() {
                let _ = manager.start_listener();
            }
            true
        }
    };
    serde_json::to_writer(&mut stream, &manager.status()).map_err(io::Error::other)?;
    stream.write_all(b"\n")?;
    Ok(manual_connect)
}

fn start_next_saved_peer(
    manager: &mut ServiceManager,
    peers: &mut VecDeque<Ipv4Addr>,
) -> Option<Instant> {
    if let Some(peer) = peers.pop_front() {
        if manager.connect_in_background(peer).is_ok() {
            return Some(Instant::now() + AUTO_CONNECT_TIMEOUT);
        }
        return start_next_saved_peer(manager, peers);
    }
    let _ = manager.start_listener();
    None
}

fn config_file() -> io::Result<PathBuf> {
    let directory = app_config_directory()?;
    fs::create_dir_all(&directory)?;
    Ok(directory.join("service.json"))
}

pub fn app_config_directory() -> io::Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    base.map(|path| path.join("clipsync"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定用户配置目录"))
}

fn load_config(path: &Path) -> PersistedConfig {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn remember_successful_peer(path: &Path, peer: Ipv4Addr) -> io::Result<()> {
    let mut config = load_config(path);
    config.successful_peers.retain(|saved| *saved != peer);
    config.successful_peers.insert(0, peer);
    config.successful_peers.truncate(MAX_SAVED_PEERS);
    fs::write(
        path,
        serde_json::to_vec_pretty(&config).map_err(io::Error::other)?,
    )
}

fn parse_incoming_ip(line: &str) -> Option<Ipv4Addr> {
    line.strip_prefix("Connection from ")?
        .parse::<SocketAddr>()
        .ok()
        .and_then(|address| match address {
            SocketAddr::V4(address) => Some(*address.ip()),
            SocketAddr::V6(_) => None,
        })
}

fn parse_transport_peer(line: &str) -> Option<&str> {
    line.strip_prefix("Transport peer: ")?
        .rsplit_once(" [")
        .map(|(device_name, _)| device_name)
}

fn sync_executable() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "windows")]
    let file_name = "clipsync.exe";
    #[cfg(not(target_os = "windows"))]
    let file_name = "clipsync";
    Ok(executable.with_file_name(file_name))
}

fn hide_process(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
}

fn detach_process(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0008);
    }
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        kill(-(child.id() as i32), SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
const SIGTERM: i32 = 15;

#[cfg(unix)]
#[link(name = "c")]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::{ServiceStatus, device_name, parse_incoming_ip, parse_transport_peer};

    #[test]
    fn linux_or_windows_user_name_is_never_empty() {
        assert!(!device_name().trim().is_empty());
    }

    #[test]
    fn parses_service_output_without_real_addresses() {
        assert_eq!(
            parse_incoming_ip("Connection from 192.0.2.10:24872"),
            Some("192.0.2.10".parse().unwrap())
        );
        assert_eq!(
            parse_transport_peer("Transport peer: example-user [linux]"),
            Some("example-user")
        );
    }

    #[test]
    fn background_connection_does_not_expose_progress_message() {
        let peer = "192.0.2.10".parse().unwrap();
        assert!(ServiceStatus::connecting(peer, false).message.is_none());
        assert!(ServiceStatus::connecting(peer, true).message.is_some());
    }
}
