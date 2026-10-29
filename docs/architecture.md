# Architecture notes

## Product boundary

ClipSync is local-first: no account and no relay server are required. A device
first discovers another device, then the user explicitly pairs it. Clipboard
events are accepted only from a paired device over an authenticated encrypted
channel.

Discovery traffic is intentionally treated as untrusted input. A matching name,
IP address, or UUID is not proof of identity.

Discovery normally uses UDP multicast. A configured direct IPv4 address is also
probed periodically for managed networks that suppress multicast. An incoming
announcement is answered by UDP unicast to its source, so only one side needs a
manual address. Neither method can bypass network-level client isolation; a
future optional relay mode would be a separate deployment and trust boundary.

## Proposed process model

The long-running daemon will contain four replaceable layers:

- `platform`: observes and writes the operating-system clipboard.
- `core`: normalizes content, applies size/policy limits, and suppresses loops.
- `peers`: discovery, pairing, trust persistence, presence, and reconnects.
- `transport`: encrypted, authenticated clipboard event delivery.

A tray UI will call the daemon through local IPC rather than own clipboard or
network state itself. This keeps the eventual UI technology replaceable.

## Clipboard platform strategy

### Windows

Use a message-only Win32 window registered with `AddClipboardFormatListener`.
`WM_CLIPBOARDUPDATE` wakes the adapter, which reads supported formats after a
short retry if another process temporarily owns the clipboard. Writing a remote
item triggers the listener too, so the core must mark and hash the write before
calling the platform adapter.

### Linux X11

Observe the `CLIPBOARD` selection through XFixes ownership notifications and
request its offered MIME data. X11 clipboard data is owned by the source
process, so the daemon must keep serving data after a remote write.

### Linux Wayland

Wayland is compositor-dependent. On wlroots-compatible compositors,
`wl-paste --watch`/the data-control protocol can observe changes. GNOME Wayland
does not expose that protocol to ordinary clients; a GNOME Shell extension can
listen to `Meta.Selection` owner changes and pass bounded content to the daemon
over D-Bus, following the architecture demonstrated by Clipman. This is an
explicit platform integration, not something a generic Rust clipboard crate can
reliably hide.

## Loop and conflict control

Every clipboard event will carry an event UUID, origin device UUID, monotonic
per-origin sequence, MIME type, byte length, and content digest. A bounded cache
of recent event IDs/digests prevents this cycle:

`device A local copy -> device B remote write -> B listener -> device A`.

When devices copy concurrently, each device keeps its most recently observed
local/remote event. Phase 4 will define deterministic ordering and make the
conflict behavior visible in diagnostics rather than silently oscillating.

## Security decisions

- Never send clipboard content during discovery or pairing confirmation.
- Pairing is explicit on both devices and binds a stable public key, not an IP.
- Use an established authenticated-encryption protocol/library; do not invent
  cryptographic primitives.
- Apply payload limits before allocation and parsing.
- Default to text only initially, with a visible pause/disable control.
- Provide a deny-list for password managers and honor sensitive clipboard hints
  where the platform exposes them.
