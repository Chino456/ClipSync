# ClipSync

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

## Planned milestones

1. **LAN discovery (current):** multicast and direct-IP probes, peer presence,
   protocol validation, self-filtering, and expiry.
2. **Pairing and trust:** persistent device identity, explicit two-device
   confirmation, key exchange, and an encrypted authenticated channel.
3. **Text clipboard adapters:** native Windows listener; X11 adapter; Wayland
   adapter with a separate GNOME Shell integration where data-control is absent.
4. **Automatic text sync:** ordering, payload limits, reconnects, echo-loop
   suppression, and sensitive-content controls.
5. **Rich content and product shell:** images/files, tray UI, device management,
   autostart, packaging, and diagnostics.

## Development

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
