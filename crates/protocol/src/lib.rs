use prost::Message;

pub mod voip {
    include!(concat!(env!("OUT_DIR"), "/voip.rs"));
}

pub use voip::{AudioPacket, Packet};
pub use voip::packet::PacketType;

impl Packet {
    pub fn to_bytes(&self) -> Result<Vec<u8>, prost::EncodeError> {
        let mut buf = Vec::new();
        self.encode(&mut buf)?;
        Ok(buf)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, prost::DecodeError> {
        Packet::decode(data)
    }
}