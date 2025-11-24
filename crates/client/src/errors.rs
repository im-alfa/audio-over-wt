use cpal::{BuildStreamError, PlayStreamError};
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

#[derive(Error, Debug)]
pub enum RecordingErrors {
    #[error("Error building the stream: {0}")]
    ErrorBuildingStream(#[from] BuildStreamError),
    #[error("Unsupported sample format: {0}")]
    UnsupportedSampleFormat(String),
    #[error("Error playing the stream: {0}")]
    ErrorPlayingStream(#[from] PlayStreamError),
    #[error("Encoder error: {0}")]
    EncoderError(#[from] EncoderErrors),
}

#[derive(Error, Debug)]
pub enum PlayErrors {
    #[error("Error building the stream: {0}")]
    ErrorBuildingStream(#[from] BuildStreamError),
    #[error("Unsupported sample format: {0}")]
    UnsupportedSampleFormat(String),
    #[error("Error playing the stream: {0}")]
    ErrorPlayingStream(#[from] PlayStreamError),
    #[error("Error decoding audio: {0}")]
    ErrorDecodingAudio(#[from] DecoderErrors),
}