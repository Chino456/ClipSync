#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    io::{self, BufRead, BufReader},
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use serde::Serialize;
use tauri::{Manager, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    device_name: String,
    local_ip: Ipv4Addr,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionStatus {
    phase: &'static str,
    peer_name: Option<String>,
    message: Option<String>,
}

impl ConnectionStatus {
    fn listening() -> Self {
        Self {
            phase: "listening",
            peer_name: None,
            message: None,
        }
    }

    fn connecting(ip: Ipv4Addr) -> Self {
        Self {
            phase: "connecting",
            peer_name: None,
            message: Some(format!("正在连接 {ip}")),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            phase: "error",
            peer_name: None,
            message: Some(message.into()),
        }
    }
}

struct SyncManager {
    child: Option<Child>,
    status: Arc<Mutex<ConnectionStatus>>,
    generation: Arc<AtomicU64>,
}

impl SyncManager {
    fn new() -> Self {
        Self {
            child: None,
            status: Arc::new(Mutex::new(ConnectionStatus::listening())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    fn start_listener(&mut self) -> io::Result<()> {
        self.start(None)
    }

    fn connect(&mut self, peer: Ipv4Addr) -> io::Result<()> {
        self.start(Some(peer))
    }

    fn start(&mut self, peer: Option<Ipv4Addr>) -> io::Result<()> {
        self.stop();
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *self.status.lock().unwrap() =
            peer.map_or_else(ConnectionStatus::listening, ConnectionStatus::connecting);

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

        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("无法读取 ClipSync 输出"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("无法读取 ClipSync 错误输出"))?;

        let status = Arc::clone(&self.status);
        let active_generation = Arc::clone(&self.generation);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some(peer_name) = parse_transport_peer(&line) else {
                    continue;
                };
                update_status(
                    &status,
                    &active_generation,
                    generation,
                    ConnectionStatus {
                        phase: "connected",
                        peer_name: Some(peer_name.to_owned()),
                        message: None,
                    },
                );
            }
        });

        let status = Arc::clone(&self.status);
        let active_generation = Arc::clone(&self.generation);
        let waiting_status =
            peer.map_or_else(ConnectionStatus::listening, ConnectionStatus::connecting);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if line.starts_with("ClipSync failed:") {
                    update_status(
                        &status,
                        &active_generation,
                        generation,
                        ConnectionStatus::error(line),
                    );
                } else if line.starts_with("Connection ended:") {
                    update_status(
                        &status,
                        &active_generation,
                        generation,
                        waiting_status.clone(),
                    );
                }
            }
        });

        self.child = Some(child);
        Ok(())
    }

    fn status(&mut self) -> ConnectionStatus {
        if let Some(child) = self.child.as_mut()
            && let Ok(Some(exit_status)) = child.try_wait()
        {
            self.child = None;
            let mut status = self.status.lock().unwrap();
            if status.phase != "error" {
                *status = ConnectionStatus::error(format!("同步进程已退出（状态：{exit_status}）"));
            }
        }
        self.status.lock().unwrap().clone()
    }

    fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let Some(mut child) = self.child.take() else {
            return;
        };
        #[cfg(unix)]
        unsafe {
            kill(-(child.id() as i32), SIGTERM);
        }
        #[cfg(not(unix))]
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for SyncManager {
    fn drop(&mut self) {
        self.stop();
    }
}

fn update_status(
    status: &Mutex<ConnectionStatus>,
    active_generation: &AtomicU64,
    generation: u64,
    update: ConnectionStatus,
) {
    if active_generation.load(Ordering::Acquire) == generation {
        *status.lock().unwrap() = update;
    }
}

#[tauri::command]
fn device_info() -> Result<DeviceInfo, String> {
    Ok(DeviceInfo {
        device_name: device_name(),
        local_ip: local_ipv4().map_err(|error| error.to_string())?,
    })
}

#[tauri::command]
fn connect_to_ip(ip: String, manager: State<'_, Mutex<SyncManager>>) -> Result<(), String> {
    let peer = parse_peer_ip(&ip)?;
    let mut manager = manager.lock().map_err(|_| "连接状态不可用".to_owned())?;
    if let Err(error) = manager.connect(peer) {
        let message = format!("启动连接失败：{error}");
        *manager.status.lock().unwrap() = ConnectionStatus::error(&message);
        return Err(message);
    }
    Ok(())
}

#[tauri::command]
fn connection_status(manager: State<'_, Mutex<SyncManager>>) -> Result<ConnectionStatus, String> {
    Ok(manager
        .lock()
        .map_err(|_| "连接状态不可用".to_owned())?
        .status())
}

fn parse_peer_ip(value: &str) -> Result<Ipv4Addr, String> {
    let ip = value
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| "请输入有效的 IPv4 地址，例如 10.29.182.191".to_owned())?;
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
        return Err("该 IPv4 地址不能作为连接目标".to_owned());
    }
    Ok(ip)
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
            "未找到可用于局域网连接的 IPv4 地址",
        )),
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

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let mut manager = SyncManager::new();
            if let Err(error) = manager.start_listener() {
                *manager.status.lock().unwrap() =
                    ConnectionStatus::error(format!("启动监听失败：{error}"));
            }
            app.manage(Mutex::new(manager));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            device_info,
            connect_to_ip,
            connection_status
        ])
        .run(tauri::generate_context!())
        .expect("failed to run ClipSync GUI");
}

#[cfg(test)]
mod tests {
    use super::{device_name, parse_peer_ip, parse_transport_peer};

    #[test]
    fn device_name_is_never_empty() {
        assert!(!device_name().trim().is_empty());
    }

    #[test]
    fn validates_peer_ipv4() {
        assert!(parse_peer_ip("10.29.182.191").is_ok());
        assert!(parse_peer_ip("not-an-ip").is_err());
        assert!(parse_peer_ip("224.0.0.1").is_err());
    }

    #[test]
    fn extracts_peer_device_name() {
        assert_eq!(
            parse_transport_peer("Transport peer: Linux laptop [linux]"),
            Some("Linux laptop")
        );
    }
}
