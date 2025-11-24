use thiserror::Error;

#[derive(Error, Debug)]
pub enum EncoderErrors {
    #[error("Error creating encoder: {0}")]
    Initialization(String),
    #[error("Error setting bitrate: {0}")]
    Bitrate(String),
    #[error("Error encoding audio: {0}")]
    Encoding(String),
}

#[derive(Error, Debug)]
pub enum DecoderErrors {
    #[error("Error creating decoder: {0}")]
    Initialization(String),
    #[error("Packet format error: {0}")]
    PacketFormat(String),
    #[error("Error decoding audio: {0}")]
    Decoding(String),
}
    #[error("Error decoding audio: {0}")]
    DecodingError(String),
}