use anyhow::Result;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{Instrument, debug, error, info, info_span};
use tracing_subscriber::{EnvFilter, filter::LevelFilter};
use wtransport::{Endpoint, Identity, ServerConfig, endpoint::IncomingSession};
use protocol::Packet;

type AudioMessage = Vec<u8>;

fn init_logging() {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_target(true)
        .with_level(true)
        .with_env_filter(env_filter)
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();

    let config = ServerConfig::builder()
        .with_bind_default(4433)
        .with_identity(Identity::self_signed(["localhost"]).unwrap())
        .keep_alive_interval(Some(Duration::from_secs(3)))
        .build();

    let server = Endpoint::server(config)?;
    
    let (tx, _rx) = broadcast::channel::<AudioMessage>(100);

    info!("Server ready!");

    for id in 0.. {
        let incoming_session = server.accept().await;
        let tx = tx.clone();
        let rx = tx.subscribe();
        tokio::spawn(
            handle_connection(id, incoming_session, tx, rx)
                .instrument(info_span!("Connection", id))
        );
    }

    Ok(())
}

async fn handle_connection(
    client_id: usize,
    incoming_session: IncomingSession,
    tx: broadcast::Sender<AudioMessage>,
    mut rx: broadcast::Receiver<AudioMessage>,
) {
    let result = handle_connection_impl(client_id, incoming_session, tx, &mut rx).await;
    
    info!("Client {} disconnected", client_id);
    
    if let Err(e) = result {
        error!("Connection error: {:?}", e);
    }
}

async fn handle_connection_impl(
    client_id: usize,
    incoming_session: IncomingSession,
    tx: broadcast::Sender<AudioMessage>,
    rx: &mut broadcast::Receiver<AudioMessage>,
) -> Result<()> {
    info!("Waiting for session request...");

    let session_request = incoming_session.await?;

    info!(
        "New session: Authority: '{}', Path: '{}'",
        session_request.authority(),
        session_request.path()
    );

    let connection = session_request.accept().await?;

    info!("Client {} connected", client_id);

    loop {
        tokio::select! {
            dgram = connection.receive_datagram() => {
                let dgram = dgram?;
                let payload = dgram.payload();
                
                match Packet::from_bytes(&payload) {
                    Ok(_packet) => {
                        debug!("Received packet from client {}", client_id);
                        
                        let _ = tx.send(payload.to_vec());
                    }
                    Err(e) => {
                        error!("Failed to parse packet from client {}: {}", client_id, e);
                    }
                }
            }
            msg = rx.recv() => {
                match msg {
                    Ok(data) => {
                        match connection.send_datagram(&data) {
                            Ok(_) => {
                                debug!("Forwarded packet to client {}", client_id);
                            }
                            Err(e) => {
                                error!("Failed to forward to client {}: {}", client_id, e);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        error!("Client {} lagged, skipped {} messages", client_id, skipped);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Broadcast channel closed");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}