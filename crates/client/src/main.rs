use device_query::{DeviceQuery, DeviceState, Keycode};
use tokio::sync::mpsc;
use tracing::{debug, info, level_filters::LevelFilter};
use tracing_subscriber::EnvFilter;
use wtransport::{ClientConfig, Endpoint};
use crate::{audio_backend::AudioBackendBuilder, coder::{Encoder, Decoder, OpusDecoder, OpusEncoder}, constants::BITRATE};
use protocol::{Packet, AudioPacket, PacketType};

mod coder;
mod constants;
mod errors;
mod audio_backend;

async fn keyboard_input_listener(tx: mpsc::Sender<bool>, ptt_key: Keycode) {
    let device_state = DeviceState::new();
    let mut recording = false;

    loop {
        let keys = device_state.get_keys();

        if keys.contains(&ptt_key) && !recording {
            recording = true;
            info!("recording");
            let res = tx.send(true).await;
            if let Err(e) = res {
                info!("Error sending recording signal: {}", e);
            }
        } else if !keys.contains(&ptt_key) && recording {
            recording = false;
            info!("stopped recording");
            let res = tx.send(false).await;
            if let Err(e) = res {
                info!("Error sending recording signal: {}", e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
}

fn init_logging() {
    let level = LevelFilter::DEBUG;

    let env_filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_target(true)
        .with_level(true)
        .with_env_filter(env_filter)
        .init();
}

#[tokio::main]
async fn main() {
    init_logging();

    let config = ClientConfig::builder()
        .with_bind_default()
        .with_no_cert_validation()
        .build();

    let connection = Endpoint::client(config)
        .unwrap()
        .connect("https://[::1]:4433")
        .await
        .unwrap();

    let (_audio_backend, tx_audio_playback, mut rx_audio_recording, tx_recording_signal) =
        AudioBackendBuilder::build();

    // TODO: move to config
    let ptt_key = Keycode::Delete;

    #[cfg(windows)]
    {
        tokio::spawn(async move {
            keyboard_input_listener(tx_recording_signal, ptt_key).await;
        });
    }

    // UNIX DeviceState doesn't implement Send + Sync
    #[cfg(unix)]
    {
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(keyboard_input_listener(tx_recording_signal, ptt_key));
        });
    }

    let encoder = OpusEncoder::new(BITRATE).expect("Error creating encoder");
    // TODO: create a decoder per client
    let mut decoder = OpusDecoder::new().expect("Error creating decoder");

    loop {
        tokio::select! {
            datagram = connection.receive_datagram() => {
                let datagram = match datagram {
                    Ok(datagram) => datagram,
                    Err(e) => {
                        info!("Error receiving datagram: {}. Connection closed", e);
                        break;
                    }
                };

                // try to deserialize the packet
                let packet = match Packet::from_bytes(&datagram.payload()) {
                    Ok(packet) => packet,
                    Err(e) => {
                        info!("Error deserializing packet: {}", e);
                        continue;
                    }
                };

                // Handle different packet types
                if let Some(packet_type) = packet.packet_type {
                    match packet_type {
                        PacketType::Audio(audio_packet) => {
                            let decoded_frame = match decoder.decode(audio_packet.frame) {
                                Ok(frame) => frame,
                                Err(e) => {
                                    info!("Error decoding frame: {}", e);
                                    continue;
                                }
                            };

                            match tx_audio_playback.send(decoded_frame).await {
                                Ok(_) => (),
                                Err(e) => {
                                    info!("Error sending frame to playback: {}", e);
                                    continue;
                                }
                            }

                            debug!("playing audio");
                        }
                    }
                } else {
                    debug!("Received empty packet");
                }
            },
            frame = rx_audio_recording.recv() => {
                let frame = match frame {
                    Some(frame) => frame,
                    None => {
                        info!("Error receiving frame");
                        continue;
                    }
                };

                // TODO: stop hardcoding the mic sample rate
                let encoded_frame = match encoder.encode(frame, 44100) {
                    Ok(frame) => frame,
                    Err(e) => {
                        info!("Error encoding frame: {}", e);
                        continue;
                    }
                };

                let packet = Packet {
                    packet_type: Some(PacketType::Audio(AudioPacket {
                        index: 1,
                        frame: encoded_frame,
                    })),
                };

                let packet_bytes = match packet.to_bytes() {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        info!("Error serializing packet: {}", e);
                        continue;
                    }
                };

                connection.send_datagram(packet_bytes).unwrap();
            }
        }
    }
}
