//! Platform-independent types shared by every ClipSync frontend and daemon.

mod discovery;
mod sync;

pub use discovery::{
    DiscoveryMessage, DiscoveryMessageKind, DiscoveryProtocolError, PROTOCOL_MAGIC,
    PROTOCOL_VERSION,
};
pub use sync::{
    MAX_CLIPBOARD_IMAGE_BYTES, MAX_CLIPBOARD_TEXT_BYTES, SYNC_PROTOCOL_VERSION, SyncMessage,
    SyncProtocolError,
};
