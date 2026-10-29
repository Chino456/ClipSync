use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_MAGIC: &str = "clipsync";
pub const PROTOCOL_VERSION: u16 = 1;
const MAX_DEVICE_NAME_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryMessageKind {
    Announce,
    Response,
}

/// The small, non-sensitive datagram visible to other devices on the LAN.
///
/// Discovery proves only reachability. It never makes a peer trusted; pairing
/// and encrypted clipboard messages use a separate protocol in a later phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryMessage {
    pub magic: String,
    pub version: u16,
    pub kind: DiscoveryMessageKind,
    pub device_id: Uuid,
    pub device_name: String,
    pub platform: String,
    pub service_port: u16,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DiscoveryProtocolError {
    #[error("invalid discovery JSON: {0}")]
    InvalidJson(String),
    #[error("not a ClipSync packet")]
    InvalidMagic,
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("device name must contain 1 to {MAX_DEVICE_NAME_BYTES} UTF-8 bytes")]
    InvalidDeviceName,
    #[error("platform must not be empty")]
    InvalidPlatform,
    #[error("service port must not be zero")]
    InvalidServicePort,
    #[error("discovery packet is too large")]
    PacketTooLarge,
}

impl DiscoveryMessage {
    pub fn new(
        kind: DiscoveryMessageKind,
        device_id: Uuid,
        device_name: impl Into<String>,
        platform: impl Into<String>,
        service_port: u16,
    ) -> Result<Self, DiscoveryProtocolError> {
        let message = Self {
            magic: PROTOCOL_MAGIC.to_owned(),
            version: PROTOCOL_VERSION,
            kind,
            device_id,
            device_name: device_name.into(),
            platform: platform.into(),
            service_port,
        };
        message.validate()?;
        Ok(message)
    }

    pub fn response_to(&self) -> Self {
        let mut response = self.clone();
        response.kind = DiscoveryMessageKind::Response;
        response
    }

    pub fn encode(&self) -> Result<Vec<u8>, DiscoveryProtocolError> {
        self.validate()?;
        let encoded = serde_json::to_vec(self)
            .map_err(|error| DiscoveryProtocolError::InvalidJson(error.to_string()))?;
        if encoded.len() > 1_200 {
            return Err(DiscoveryProtocolError::PacketTooLarge);
        }
        Ok(encoded)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DiscoveryProtocolError> {
        if bytes.len() > 1_200 {
            return Err(DiscoveryProtocolError::PacketTooLarge);
        }
        let message: Self = serde_json::from_slice(bytes)
            .map_err(|error| DiscoveryProtocolError::InvalidJson(error.to_string()))?;
        message.validate()?;
        Ok(message)
    }

    fn validate(&self) -> Result<(), DiscoveryProtocolError> {
        if self.magic != PROTOCOL_MAGIC {
            return Err(DiscoveryProtocolError::InvalidMagic);
        }
        if self.version != PROTOCOL_VERSION {
            return Err(DiscoveryProtocolError::UnsupportedVersion(self.version));
        }
        if self.device_name.is_empty() || self.device_name.len() > MAX_DEVICE_NAME_BYTES {
            return Err(DiscoveryProtocolError::InvalidDeviceName);
        }
        if self.platform.trim().is_empty() {
            return Err(DiscoveryProtocolError::InvalidPlatform);
        }
        if self.service_port == 0 {
            return Err(DiscoveryProtocolError::InvalidServicePort);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(kind: DiscoveryMessageKind) -> DiscoveryMessage {
        DiscoveryMessage::new(kind, Uuid::nil(), "Desk PC", "linux", 24_871).unwrap()
    }

    #[test]
    fn round_trip_is_stable() {
        let original = message(DiscoveryMessageKind::Announce);
        let decoded = DiscoveryMessage::decode(&original.encode().unwrap()).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn rejects_foreign_packets() {
        let packet = br#"{"magic":"other","version":1,"kind":"announce","device_id":"00000000-0000-0000-0000-000000000000","device_name":"PC","platform":"linux","service_port":24871}"#;
        assert_eq!(
            DiscoveryMessage::decode(packet),
            Err(DiscoveryProtocolError::InvalidMagic)
        );
    }

    #[test]
    fn rejects_future_protocol_version() {
        let packet = br#"{"magic":"clipsync","version":2,"kind":"announce","device_id":"00000000-0000-0000-0000-000000000000","device_name":"PC","platform":"linux","service_port":24871}"#;
        assert_eq!(
            DiscoveryMessage::decode(packet),
            Err(DiscoveryProtocolError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn response_does_not_change_identity() {
        let announcement = message(DiscoveryMessageKind::Announce);
        let response = announcement.response_to();
        assert_eq!(response.kind, DiscoveryMessageKind::Response);
        assert_eq!(response.device_id, announcement.device_id);
    }
}
