use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const SYNC_PROTOCOL_VERSION: u16 = 1;
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;
const MAX_DEVICE_NAME_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncMessage {
    Hello {
        version: u16,
        device_id: Uuid,
        device_name: String,
        platform: String,
    },
    ClipboardText {
        event_id: Uuid,
        text: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SyncProtocolError {
    #[error("invalid sync JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported sync protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("device name must contain 1 to {MAX_DEVICE_NAME_BYTES} UTF-8 bytes")]
    InvalidDeviceName,
    #[error("platform must not be empty")]
    InvalidPlatform,
    #[error("clipboard text exceeds the 1 MiB limit")]
    ClipboardTooLarge,
}

impl SyncMessage {
    pub fn hello(
        device_id: Uuid,
        device_name: impl Into<String>,
        platform: impl Into<String>,
    ) -> Result<Self, SyncProtocolError> {
        let message = Self::Hello {
            version: SYNC_PROTOCOL_VERSION,
            device_id,
            device_name: device_name.into(),
            platform: platform.into(),
        };
        message.validate()?;
        Ok(message)
    }

    pub fn clipboard_text(text: String) -> Result<Self, SyncProtocolError> {
        let message = Self::ClipboardText {
            event_id: Uuid::new_v4(),
            text,
        };
        message.validate()?;
        Ok(message)
    }

    pub fn encode(&self) -> Result<Vec<u8>, SyncProtocolError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| SyncProtocolError::InvalidJson(error.to_string()))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SyncProtocolError> {
        let message: Self = serde_json::from_slice(bytes)
            .map_err(|error| SyncProtocolError::InvalidJson(error.to_string()))?;
        message.validate()?;
        Ok(message)
    }

    fn validate(&self) -> Result<(), SyncProtocolError> {
        match self {
            Self::Hello {
                version,
                device_name,
                platform,
                ..
            } => {
                if *version != SYNC_PROTOCOL_VERSION {
                    return Err(SyncProtocolError::UnsupportedVersion(*version));
                }
                if device_name.is_empty() || device_name.len() > MAX_DEVICE_NAME_BYTES {
                    return Err(SyncProtocolError::InvalidDeviceName);
                }
                if platform.trim().is_empty() {
                    return Err(SyncProtocolError::InvalidPlatform);
                }
            }
            Self::ClipboardText { text, .. } => {
                if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
                    return Err(SyncProtocolError::ClipboardTooLarge);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_round_trip() {
        let original = SyncMessage::hello(Uuid::nil(), "Laptop", "linux").unwrap();
        assert_eq!(
            SyncMessage::decode(&original.encode().unwrap()).unwrap(),
            original
        );
    }

    #[test]
    fn clipboard_round_trip_preserves_newlines() {
        let original = SyncMessage::clipboard_text("first\nsecond\n你好".to_owned()).unwrap();
        assert_eq!(
            SyncMessage::decode(&original.encode().unwrap()).unwrap(),
            original
        );
    }

    #[test]
    fn rejects_oversized_clipboard() {
        let text = "x".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1);
        assert_eq!(
            SyncMessage::clipboard_text(text),
            Err(SyncProtocolError::ClipboardTooLarge)
        );
    }
}
