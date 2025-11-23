use thiserror::Error;

#[derive(Error, Debug)]
pub enum EncoderErrors {
    #[error("Error creating encoder: {0}")]
    InitializationError(String),
    #[error("Error setting bitrate: {0}")]
    BitrateError(String),
    #[error("Error encoding audio: {0}")]
    EncodingError(String),
}

#[derive(Error, Debug)]
pub enum DecoderErrors {
    #[error("Error creating decoder: {0}")]
    InitializationError(String),
    #[error("Packet format error: {0}")]
    PacketFormatError(String),
    #[error("Error decoding audio: {0}")]
    DecodingError(String),
}