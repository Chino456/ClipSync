#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod daemon;

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[cfg(target_os = "windows")]
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
use std::{fs, path::PathBuf};

use daemon::{ControlRequest, ServiceStatus};
use serde::Serialize;

static USER_STOPPED_SERVICE: AtomicBool = AtomicBool::new(false);
static SERVICE_CONTROL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    device_name: String,
    local_ip: Ipv4Addr,
}

#[tauri::command]
fn device_info() -> Result<DeviceInfo, String> {
    Ok(DeviceInfo {
        device_name: daemon::device_name(),
        local_ip: local_ipv4().map_err(|error| error.to_string())?,
    })
}

#[tauri::command]
fn connect_to_ip(ip: String) -> Result<ServiceStatus, String> {
    let peer = parse_peer_ip(&ip)?;
    let _control = SERVICE_CONTROL_LOCK.lock().unwrap();
    USER_STOPPED_SERVICE.store(false, Ordering::Release);
    daemon::ensure_running().map_err(|error| error.to_string())?;
    daemon::send_request(&ControlRequest::Connect { ip: peer })
        .map_err(|error| format!("无法连接后台服务：{error}"))
}

#[tauri::command]
fn connection_status() -> ServiceStatus {
    let _control = SERVICE_CONTROL_LOCK.lock().unwrap();
    if USER_STOPPED_SERVICE.load(Ordering::Acquire) {
        return ServiceStatus::stopped();
    }
    if daemon::ensure_running().is_err() {
        return ServiceStatus::listening();
    }
    daemon::send_request(&ControlRequest::Status).unwrap_or_else(|_| ServiceStatus::listening())
}

#[tauri::command]
fn stop_clipsync_processes() -> Result<ServiceStatus, String> {
    let _control = SERVICE_CONTROL_LOCK.lock().unwrap();
    USER_STOPPED_SERVICE.store(true, Ordering::Release);

    if daemon::send_request(&ControlRequest::Shutdown).is_err()
        && daemon::control_endpoint_is_open()
    {
        USER_STOPPED_SERVICE.store(false, Ordering::Release);
        return Err("无法通知 ClipSync 后台服务结束进程".to_owned());
    }

    for _ in 0..40 {
        thread::sleep(Duration::from_millis(50));
        if !daemon::control_endpoint_is_open() {
            return Ok(ServiceStatus::stopped());
        }
    }

    USER_STOPPED_SERVICE.store(false, Ordering::Release);
    Err("ClipSync 进程未能在预期时间内结束".to_owned())
}

#[tauri::command]
fn autostart_enabled() -> bool {
    autostart_is_enabled()
}

#[tauri::command]
fn set_autostart(enabled: bool) -> Result<bool, String> {
    configure_autostart(enabled).map_err(|error| error.to_string())?;
    Ok(autostart_is_enabled())
}

fn parse_peer_ip(value: &str) -> Result<Ipv4Addr, String> {
    let ip = value
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| "请输入有效的 IPv4 地址，例如 192.0.2.10".to_owned())?;
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
        return Err("该 IPv4 地址不能作为连接目标".to_owned());
    }
    Ok(ip)
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

#[cfg(target_os = "linux")]
fn autostart_file() -> io::Result<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定用户配置目录"))?;
    Ok(config_home.join("autostart").join("clipsync.desktop"))
}

#[cfg(target_os = "linux")]
fn autostart_is_enabled() -> bool {
    autostart_file().is_ok_and(|path| path.is_file())
}

#[cfg(target_os = "linux")]
fn configure_autostart(enabled: bool) -> io::Result<()> {
    let path = autostart_file()?;
    if !enabled {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
    }

    let executable = std::env::current_exe()?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("无效的自启动目录"))?;
    fs::create_dir_all(parent)?;
    let executable = executable.to_string_lossy().replace('"', "\\\"");
    fs::write(
        path,
        format!(
            "[Desktop Entry]\nType=Application\nName=ClipSync\nExec=\"{executable}\" --daemon\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        ),
    )
}

#[cfg(target_os = "windows")]
fn autostart_is_enabled() -> bool {
    let mut command = Command::new("reg.exe");
    command.args([
        "query",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
        "/v",
        "ClipSync",
    ]);
    hide_console(&mut command);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    command.status().is_ok_and(|status| status.success())
}

#[cfg(target_os = "windows")]
fn configure_autostart(enabled: bool) -> io::Result<()> {
    let mut command = Command::new("reg.exe");
    if enabled {
        let value = format!("\"{}\" --daemon", std::env::current_exe()?.display());
        command.args([
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "ClipSync",
            "/t",
            "REG_SZ",
            "/d",
            &value,
            "/f",
        ]);
    } else {
        command.args([
            "delete",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "ClipSync",
            "/f",
        ]);
    }
    hide_console(&mut command);
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() || (!enabled && !autostart_is_enabled()) {
        Ok(())
    } else {
        Err(io::Error::other("修改 Windows 开机自启失败"))
    }
}

#[cfg(target_os = "windows")]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--daemon") {
        let _ = daemon::run();
        return;
    }

    tauri::Builder::default()
        .setup(|_| {
            let _ = daemon::ensure_running();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            device_info,
            connect_to_ip,
            connection_status,
            stop_clipsync_processes,
            autostart_enabled,
            set_autostart
        ])
        .run(tauri::generate_context!())
        .expect("failed to run ClipSync GUI");
}

#[cfg(test)]
mod tests {
    use super::parse_peer_ip;

    #[test]
    fn validates_documentation_only_peer_addresses() {
        assert!(parse_peer_ip("192.0.2.10").is_ok());
        assert!(parse_peer_ip("not-an-ip").is_err());
        assert!(parse_peer_ip("224.0.0.1").is_err());
    }
}
