use device_query::DeviceQuery;
use device_query::DeviceState;
use device_query::Keycode;
use tokio::sync::mpsc;
use tracing::level_filters::LevelFilter;
use tracing::{debug, info};
use tracing_subscriber::EnvFilter;
use wtransport::ClientConfig;
use wtransport::Endpoint;

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

    let (tx_recording_signal, mut rx_recording_signal) = mpsc::channel::<bool>(20);

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

    loop {
        tokio::select! {
            recvd = rx_recording_signal.recv() => {
                debug!("Signal Received: {:?}", recvd);
            }
        }
    }
}

