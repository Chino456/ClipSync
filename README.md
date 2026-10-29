# ClipSync

[简体中文](README.zh-CN.md) | English

ClipSync is an incremental prototype for trusted clipboard synchronization
between Windows and Linux machines on the same LAN.

## Phase 1: LAN discovery

The current executable only discovers peers. It does **not** read, transmit, or
write clipboard data yet, and discovery does not imply trust.

Build and run it on two machines connected to the same LAN:

```bash
cargo run --release -- discover "Linux laptop"
```

```powershell
cargo run --release -- discover "Windows desktop"
```

Each side should print a `+ Found ...` line within a few seconds. Stop one side;
the other should print `- Lost ...` after about 10 seconds.

The prototype uses IPv4 UDP multicast group `239.255.67.83` and port `24871`.
On Windows, allow the executable on **Private networks** if the firewall asks.
Guest Wi-Fi and AP/client isolation can prevent devices from seeing each other.

### Direct IP fallback

If the network blocks multicast, point either device at the other device's IPv4
address. The other side can keep using the normal command because it replies
directly to the source address:

```bash
cargo run --release -- discover "Linux laptop" --peer 10.20.30.41
```

Multiple known addresses and a non-default port are supported:

```bash
cargo run --release -- discover "Linux laptop" \
  --peer 10.20.30.41 \
  --peer 10.20.30.42:24871
```

To use only unicast without joining the multicast group, start one device in
listen-only mode:

```bash
cargo run --release -- discover "Linux laptop" --no-multicast
```

Then point the other device at it:

```powershell
cargo run --release -- discover "Windows desktop" `
  --peer 10.20.30.41 --no-multicast
```

Find the address with `ipconfig` on Windows or `ip -4 addr` on Linux. Direct IP
helps when only multicast is filtered. It cannot bypass AP/client isolation that
blocks all traffic between wireless clients. A quick reachability check is
`ping <peer-ip>`; however, some networks block ICMP while still allowing the UDP
application port, so the ClipSync probe is the final test.

## Phase 2: text clipboard synchronization (MVP)

The MVP synchronizes UTF-8 plain text up to 1 MiB over one bidirectional TCP
connection. It does not upload clipboard content that existed before startup.

On the receiving/listening device:

```bash
cargo run --release -- sync "Linux laptop"
```

On the device that initiates the connection:

```powershell
cargo run --release -- sync "Windows desktop" --peer 10.20.30.41
```

Both directions are active after the connection is established. Copy text on
either device, then paste normally with `Ctrl+V` on the other device. TCP port
`24872` must be allowed by the listening device's firewall.

### Single-button GUI

`cargo build --release` now also builds `clipsync-gui` (`clipsync-gui.exe` on
Windows). Keep it beside the `clipsync` executable because it starts the CLI
transport in the background.

Start a listening Linux device with:

```bash
target/release/clipsync-gui "Linux laptop"
```

Connect from Windows by IP with:

```powershell
target\release\clipsync-gui.exe "Windows desktop" --peer 10.20.30.41
```

The window has one button: click it once to start synchronization and again to
stop it. Closing the window also stops the transport and clipboard watcher.
All existing `sync` arguments, including `--port`, can be passed to the GUI.
Linux uses the system GTK 3 runtime; Windows uses a native Win32 window.

Linux Wayland requires `wl-clipboard`. Linux X11 requires `xclip` or `xsel`.
Windows uses the native Unicode clipboard API and has no extra dependency.

GNOME Wayland also requires ClipSync's event bridge extension:

```bash
cargo run --release -- setup-gnome
# Log out and back in once, then:
gnome-extensions enable clipsync@local
```

The extension observes clipboard ownership inside GNOME Shell and forwards text
to the local ClipSync process over the session D-Bus. It never accesses the
network. Other compatible Wayland compositors use one persistent
`wl-paste --watch` process; X11 retains the `xclip`/`xsel` fallback.

> **Security notice:** this milestone deliberately keeps pairing and encrypted
> transport separate. Clipboard traffic is currently unencrypted. Use it only
> between devices you control on a network you trust. Authenticated pairing and
> encryption remain the next milestone.

## Planned milestones

1. **LAN discovery (done):** multicast and direct-IP probes, peer presence,
   protocol validation, self-filtering, and expiry.
2. **Text clipboard MVP (current):** bidirectional TCP text synchronization,
   native Windows clipboard, Linux Wayland/X11 adapters, limits, and loop
   suppression.
3. **Pairing and trust:** persistent device identity, explicit two-device
   confirmation, key exchange, and an encrypted authenticated channel.
4. **Native event adapters:** Win32 event listener and compositor-specific Linux
   event integrations to replace the MVP polling adapter.
5. **Rich content and product shell:** images/files, tray UI, device management,
   autostart, packaging, and diagnostics.

## Development

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
