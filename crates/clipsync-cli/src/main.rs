use std::{
    collections::HashMap,
    env, io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    process::ExitCode,
    str::FromStr,
    time::{Duration, Instant},
};

use clipsync_core::{DiscoveryMessage, DiscoveryMessageKind};
use socket2::{Domain, Protocol, Socket, Type};
use uuid::Uuid;

mod clipboard;
mod gnome_extension;
mod sync;

const MULTICAST_ADDRESS: Ipv4Addr = Ipv4Addr::new(239, 255, 67, 83);
const DISCOVERY_PORT: u16 = 24_871;
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3);
const PEER_EXPIRY: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct Peer {
    message: DiscoveryMessage,
    address: SocketAddr,
    last_seen: Instant,
}

#[derive(Debug, PartialEq, Eq)]
struct Config {
    device_name: String,
    direct_peers: Vec<SocketAddrV4>,
    multicast_enabled: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ClipSync failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    if env::args().nth(1).as_deref() == Some("__clipboard-frame") {
        return clipboard::run_frame_helper();
    }
    if env::args().nth(1).as_deref() == Some("setup-gnome") {
        return gnome_extension::install();
    }
    if env::args().nth(1).as_deref() == Some("sync") {
        return sync::run(env::args().skip(2));
    }

    let config = parse_args(env::args().skip(1))?;
    let device_id = Uuid::new_v4();
    let local = DiscoveryMessage::new(
        DiscoveryMessageKind::Announce,
        device_id,
        config.device_name,
        env::consts::OS,
        DISCOVERY_PORT,
    )?;
    let socket = discovery_socket(config.multicast_enabled)?;
    let multicast_target = SocketAddrV4::new(MULTICAST_ADDRESS, DISCOVERY_PORT);
    let mut peers: HashMap<Uuid, Peer> = HashMap::new();
    let mut last_announcement = Instant::now() - ANNOUNCE_INTERVAL;
    let mut buffer = [0_u8; 1_500];

    println!("ClipSync LAN discovery (protocol v1)");
    println!("Local device: {} ({device_id})", local.device_name);
    println!("Listening on UDP 0.0.0.0:{DISCOVERY_PORT}");
    if config.multicast_enabled {
        println!("Multicast discovery: {multicast_target}");
    } else {
        println!("Multicast discovery: disabled");
    }
    for target in &config.direct_peers {
        println!("Direct peer: {target}");
    }
    println!("Press Ctrl+C to stop.");

    loop {
        if last_announcement.elapsed() >= ANNOUNCE_INTERVAL {
            let packet = local.encode()?;
            if config.multicast_enabled {
                send_probe(&socket, &packet, multicast_target);
            }
            for target in &config.direct_peers {
                send_probe(&socket, &packet, *target);
            }
            last_announcement = Instant::now();
        }

        match socket.recv_from(&mut buffer) {
            Ok((size, source)) => {
                let Ok(message) = DiscoveryMessage::decode(&buffer[..size]) else {
                    continue;
                };
                if message.device_id == device_id {
                    continue;
                }

                let is_new = !peers.contains_key(&message.device_id);
                let peer_id = message.device_id;
                let peer_name = message.device_name.clone();
                let peer_platform = message.platform.clone();
                let kind = message.kind;
                peers.insert(
                    peer_id,
                    Peer {
                        message,
                        address: source,
                        last_seen: Instant::now(),
                    },
                );

                if is_new {
                    println!(
                        "+ Found {peer_name} [{peer_platform}] at {} (id {peer_id})",
                        source.ip()
                    );
                }

                if kind == DiscoveryMessageKind::Announce {
                    // Reply directly even when the request arrived via multicast. This both
                    // reduces LAN chatter and makes a one-sided --peer configuration work.
                    send_probe(&socket, &local.response_to().encode()?, source);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }

        let now = Instant::now();
        peers.retain(|peer_id, peer| {
            let alive = now.duration_since(peer.last_seen) <= PEER_EXPIRY;
            if !alive {
                println!(
                    "- Lost {} at {} (id {peer_id})",
                    peer.message.device_name,
                    peer.address.ip()
                );
            }
            alive
        });
    }
}

fn send_probe(socket: &UdpSocket, packet: &[u8], target: impl Into<SocketAddr>) {
    let target = target.into();
    if let Err(error) = socket.send_to(packet, target) {
        eprintln!("! Could not probe {target}: {error}");
    }
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(usage());
    };
    if command != "discover" {
        return Err(format!("unknown command `{command}`\n{}", usage()));
    }
    let Some(device_name) = args.next() else {
        return Err(usage());
    };

    let mut direct_peers = Vec::new();
    let mut multicast_enabled = true;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--peer" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--peer requires an IPv4 address".to_owned())?;
                direct_peers.push(parse_direct_peer(&value)?);
            }
            "--no-multicast" => multicast_enabled = false,
            unknown => return Err(format!("unknown option `{unknown}`\n{}", usage())),
        }
    }
    Ok(Config {
        device_name,
        direct_peers,
        multicast_enabled,
    })
}

fn parse_direct_peer(value: &str) -> Result<SocketAddrV4, String> {
    if let Ok(address) = SocketAddrV4::from_str(value) {
        return Ok(address);
    }
    let ip = Ipv4Addr::from_str(value)
        .map_err(|_| format!("invalid peer `{value}`; expected IPv4 or IPv4:port"))?;
    Ok(SocketAddrV4::new(ip, DISCOVERY_PORT))
}

fn usage() -> String {
    "usage:\n  clipsync discover <device-name> [--peer <IPv4[:port]>]... [--no-multicast]\n  clipsync sync <device-name> [--peer <IPv4[:port]>] [--port <port>]\n  clipsync setup-gnome".to_owned()
}

fn discovery_socket(multicast_enabled: bool) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_multicast_ttl_v4(1)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT).into())?;
    if multicast_enabled {
        socket.join_multicast_v4(&MULTICAST_ADDRESS, &Ipv4Addr::UNSPECIFIED)?;
    }

    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_default_multicast_mode() {
        let config = parse_args(args(&["discover", "Laptop"])).unwrap();
        assert_eq!(
            config,
            Config {
                device_name: "Laptop".to_owned(),
                direct_peers: vec![],
                multicast_enabled: true,
            }
        );
    }

    #[test]
    fn parses_multiple_direct_peers_and_default_port() {
        let config = parse_args(args(&[
            "discover",
            "Laptop",
            "--peer",
            "10.20.30.40",
            "--peer",
            "10.20.30.41:25000",
            "--no-multicast",
        ]))
        .unwrap();
        assert_eq!(
            config.direct_peers,
            vec![
                SocketAddrV4::new(Ipv4Addr::new(10, 20, 30, 40), DISCOVERY_PORT),
                SocketAddrV4::new(Ipv4Addr::new(10, 20, 30, 41), 25_000),
            ]
        );
        assert!(!config.multicast_enabled);
    }

    #[test]
    fn rejects_invalid_peer() {
        let error = parse_args(args(&["discover", "Laptop", "--peer", "not-an-ip"])).unwrap_err();
        assert!(error.contains("invalid peer"));
    }

    #[test]
    fn direct_listen_only_mode_is_valid() {
        let config = parse_args(args(&["discover", "Laptop", "--no-multicast"])).unwrap();
        assert!(!config.multicast_enabled);
        assert!(config.direct_peers.is_empty());
    }
}
