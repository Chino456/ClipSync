//! Platform-independent types shared by every ClipSync frontend and daemon.

mod discovery;

pub use discovery::{
    DiscoveryMessage, DiscoveryMessageKind, DiscoveryProtocolError, PROTOCOL_MAGIC,
    PROTOCOL_VERSION,
};
