use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const SYNC_PROTOCOL_VERSION: u16 = 2;
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 10 * 1024 * 1024;
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
    ClipboardImage {
        event_id: Uuid,
        mime_type: String,
        #[serde(with = "base64_bytes")]
        png: Vec<u8>,
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
    #[error("clipboard image must be a PNG no larger than 10 MiB")]
    ClipboardImageInvalid,
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

    pub fn clipboard_image_png(png: Vec<u8>) -> Result<Self, SyncProtocolError> {
        let message = Self::ClipboardImage {
            event_id: Uuid::new_v4(),
            mime_type: "image/png".to_owned(),
            png,
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
            Self::ClipboardImage { mime_type, png, .. } => {
                if mime_type != "image/png"
                    || png.is_empty()
                    || png.len() > MAX_CLIPBOARD_IMAGE_BYTES
                    || !png.starts_with(b"\x89PNG\r\n\x1a\n")
                {
                    return Err(SyncProtocolError::ClipboardImageInvalid);
                }
            }
        }
        Ok(())
    }
}

mod base64_bytes {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(encoded).map_err(D::Error::custom)
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

    #[test]
    fn png_clipboard_round_trip() {
        let png = b"\x89PNG\r\n\x1a\nexample".to_vec();
        let original = SyncMessage::clipboard_image_png(png).unwrap();
        assert_eq!(
            SyncMessage::decode(&original.encode().unwrap()).unwrap(),
            original
        );
    }

    #[test]
    fn rejects_non_png_and_oversized_images() {
        assert_eq!(
            SyncMessage::clipboard_image_png(b"not a png".to_vec()),
            Err(SyncProtocolError::ClipboardImageInvalid)
        );
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.resize(MAX_CLIPBOARD_IMAGE_BYTES + 1, 0);
        assert_eq!(
            SyncMessage::clipboard_image_png(png),
            Err(SyncProtocolError::ClipboardImageInvalid)
        );
    }
}
