use cpal::platform::Device;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, Stream};
use ringbuf::{
    HeapRb, SharedRb,
    storage::Heap,
    traits::{Consumer, Producer, Split},
    wrap::CachingCons,
};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use tracing::{error, info};

use crate::coder::{AudioFrame, resample};
use crate::errors::{PlayErrors, RecordingErrors};

pub struct AudioBackendBuilder;
impl AudioBackendBuilder {
    pub fn build() -> (
        AudioBackend,
        mpsc::Sender<AudioFrame>,
        mpsc::Receiver<AudioFrame>,
        mpsc::Sender<bool>,
    ) {
        // TODO: tweak channel size
        let (tx_audio_playback, rx_audio_playback) = mpsc::channel::<AudioFrame>(20);
        let (tx_audio_recording, rx_audio_recording) = mpsc::channel::<AudioFrame>(20);
        let (tx_recording_signal, rx_recording_signal) = mpsc::channel::<bool>(20);

        let mut audio_backend = AudioBackend {
            playback_thread_cancel_token: None,
            recording_thread_cancel_token: None,
        };

        audio_backend.init(rx_recording_signal, rx_audio_playback, tx_audio_recording);

        (
            audio_backend,
            tx_audio_playback,
            rx_audio_recording,
            tx_recording_signal,
        )
    }
}

pub struct AudioBackend {
    playback_thread_cancel_token: Option<CancellationToken>,
    recording_thread_cancel_token: Option<CancellationToken>,
}

// TODO!: do proper error handling and propagation
impl AudioBackend {
    pub fn builder() -> AudioBackendBuilder {
        AudioBackendBuilder
    }

    pub fn init(
        &mut self,
        rx_recording_signal: mpsc::Receiver<bool>,
        rx_audio_playback: mpsc::Receiver<AudioFrame>,
        tx_audio_recording: mpsc::Sender<AudioFrame>,
    ) {
        let recording_cancellation_token = CancellationToken::new();

        thread::spawn(move || {
            let recording_device = cpal::default_host()
                .default_input_device()
                .expect("No input device found");

            info!("Starting recording thread");
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(Self::start_recording_thread(
                    recording_cancellation_token,
                    tx_audio_recording,
                    recording_device,
                    rx_recording_signal,
                ));
        });

        let playback_cancellation_token = CancellationToken::new();

        thread::spawn(move || {
            info!("Starting playback thread");
            let playback_device = cpal::default_host()
                .default_output_device()
                .expect("No output device found");

            // run the future and block the thread
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(Self::start_playback_thread(
                    playback_cancellation_token,
                    rx_audio_playback,
                    playback_device,
                ));
        });
    }


    async fn start_playback_thread(
        cancellation_token: CancellationToken,
        audio_playback_rx: mpsc::Receiver<AudioFrame>,
        playback_device: Device,
    ) {
        match Self::playback_thread(audio_playback_rx, playback_device, cancellation_token)
            .instrument(tracing::info_span!("playback_thread"))
            .await
        {
            Ok(_) => info!("Playback thread exited"),
            Err(e) => error!("Error in playback thread: {:?}", e),
        }
    }


    async fn playback_thread(
        mut audio_playback_rx: mpsc::Receiver<AudioFrame>,
        playback_device: Device,
        cancellation_token: CancellationToken,
    ) -> Result<(), PlayErrors> {
        let config = playback_device.default_output_config().unwrap();
        let channels = config.channels();
        let sample_rate = config.sample_rate().0;

        info!(
            "device_name={} ; config={:?}",
            playback_device.name().unwrap(),
            config
        );

        let err_fn = move |err| {
            error!("an error occurred on stream: {}", err);
        };

        // 60ms ring buffer = 3 audio frames = sample_rate / 1000 * 60ms samples
        let ring_size = 60 * (sample_rate / 1000) as usize;

        let ring = HeapRb::<f32>::new(ring_size * 3);
        let (mut producer, mut consumer) = ring.split();

        fn write_output_data<T>(
            output: &mut [T],
            channels: u16,

            consumer: &mut CachingCons<Arc<SharedRb<Heap<f32>>>>,
        ) where
            T: Sample + cpal::SizedSample + cpal::FromSample<f32>,
        {
            for playback_frame in output.chunks_mut(channels.into()) {
                // we are working with mono audio, so we output the same sample to all channels
                let ring_sample = consumer.try_pop().unwrap_or(0.0);
                playback_frame.iter_mut().for_each(|sample| {
                    *sample = T::from_sample(ring_sample);
                });
            }
        }

        let stream = match config.sample_format() {
            cpal::SampleFormat::I8 => playback_device.build_output_stream(
                &config.into(),
                move |data, _: &_| write_output_data::<i8>(data, channels, &mut consumer),
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => playback_device.build_output_stream(
                &config.into(),
                move |data, _: &_| write_output_data::<i16>(data, channels, &mut consumer),
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I32 => playback_device.build_output_stream(
                &config.into(),
                move |data, _: &_| write_output_data::<i32>(data, channels, &mut consumer),
                err_fn,
                None,
            )?,
            cpal::SampleFormat::F32 => playback_device.build_output_stream(
                &config.into(),
                move |data, _: &_| write_output_data::<f32>(data, channels, &mut consumer),
                err_fn,
                None,
            )?,
            sample_format => {
                return Err(PlayErrors::UnsupportedSampleFormat(format!(
                    "{sample_format}"
                )));
            }
        };

        stream.play()?;

        let mut buffer_frames: Vec<AudioFrame> = Vec::with_capacity(50);

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    info!("Playback thread cancelled");
                    // Cancellation requested, break the loop
                    break;
                }
                frame = audio_playback_rx.recv() => {
                    let frame = match frame {
                        Some(frame) => frame,
                        None => {
                            error!("Playback channel closed");
                            // channel closed, exit the loop
                            break;
                        }
                    };

                    let frame = match sample_rate {
                        48000 => frame,
                        sr => resample(frame, 48000i32, sr as i32)
                    };

                    info!("Received frame for playback, size: {}", frame.len());

                    buffer_frames.push(frame);
                    if buffer_frames.len() >= 5 {
                        producer.push_iter(buffer_frames.drain(..).flat_map(|frame| frame.to_vec()));
                    }
                }
            }
        }

        info!("Playback thread exited");
        Ok(())
    }

    async fn start_recording_thread(
        cancellation_token: CancellationToken,
        audio_recording_tx: mpsc::Sender<AudioFrame>,
        recording_device: Device,
        rx_recording_signal: mpsc::Receiver<bool>,
    ) {
        match Self::recording_thread(
            audio_recording_tx,
            recording_device,
            cancellation_token,
            rx_recording_signal,
        )
        .instrument(tracing::info_span!("recording_thread"))
        .await
        {
            Ok(_) => info!("Recording thread exited gracefully"),
            Err(e) => error!("Error in recording thread: {:?}", e),
        }
    }

    async fn recording_thread(
        tx_audio_recording: mpsc::Sender<AudioFrame>,
        recording_device: Device,
        cancellation_token: CancellationToken,
        mut rx_recording_signal: mpsc::Receiver<bool>,
    ) -> Result<(), RecordingErrors> {
        let config = recording_device.default_input_config().unwrap();
        let channels = config.channels();
        let mic_sample_rate = config.sample_rate().0;

        info!(
            "device_name={} ; config={:?}",
            recording_device.name().unwrap(),
            config
        );

        struct AudioRecordStruct {
            samples: Vec<f32>,
        }

        let samples = AudioRecordStruct {
            samples: Vec::new(),
        };

        // TODO: replace with a ring buffer?
        type AudioRecordHandle = Arc<Mutex<Option<AudioRecordStruct>>>;
        let clip = Arc::new(Mutex::new(Some(samples)));

        let err_fn = move |err| {
            error!("an error occurred on stream: {}", err);
        };

        fn write_input_data<T>(
            input: &[T],
            sample_rate: i32,
            channels: u16,
            tx_audio_recording: &mpsc::Sender<AudioFrame>,
            writer: &AudioRecordHandle,
        ) where
            T: cpal::Sample,
            f32: cpal::FromSample<T>,
        {
            if let Ok(mut guard) = writer.try_lock()
                && let Some(record_struct) = guard.as_mut()
            {
                for frame in input.chunks(channels.into()) {
                    record_struct.samples.push(f32::from_sample(frame[0]));
                    // Every 20ms, create a new frame

                    // NB: Rationale for dividing using F32s:
                    // we want to capture exactly 20ms of audio. Some sample rates like 44100Hz are not multiples of 20ms, and
                    // when divide them by 1000, we are missing some precision which then results in a frame that is not exactly 20ms
                    if record_struct.samples.len()
                        == (20f32 * (sample_rate as f32 / 1000f32)) as usize
                    {
                        let new_frame = AudioFrame::from(record_struct.samples.clone());
                        match tx_audio_recording.blocking_send(new_frame) {
                            Ok(_) => {}
                            Err(e) => {
                                error!("Error sending audio frame: {:?}", e);
                            }
                        }
                        record_struct.samples.clear();
                    }
                }
            }
        }

        let mut stream: Option<Stream> = None;

        loop {
            tokio::select! {
                signal = rx_recording_signal.recv() => {
                    match signal {
                        Some(true) => {
                            // if it's none, create a new stream
                            if stream.is_none() {
                                let clip_2 = clip.clone();
                                let tx_audio_recording = tx_audio_recording.clone();

                                let new_stream = match &config.sample_format() {
                                            cpal::SampleFormat::I8 => recording_device.build_input_stream(
                                                &config.clone().into(),
                                                move |data, _: &_| write_input_data::<i8>(data, mic_sample_rate as i32, channels, &tx_audio_recording,  &clip_2),
                                                err_fn,
                                                None,
                                            )?,
                                            cpal::SampleFormat::I16 => recording_device.build_input_stream(
                                                &config.clone().into(),
                                                move |data, _: &_| write_input_data::<i16>(data, mic_sample_rate as i32, channels, &tx_audio_recording,  &clip_2),
                                                err_fn,
                                                None,
                                            )?,
                                            cpal::SampleFormat::I32 => recording_device.build_input_stream(
                                                &config.clone().into(),
                                                move |data, _: &_| write_input_data::<i32>(data, mic_sample_rate as i32, channels, &tx_audio_recording,  &clip_2),
                                                err_fn,
                                                None,
                                            )?,
                                            cpal::SampleFormat::F32 => recording_device.build_input_stream(
                                                &config.clone().into(),
                                                move |data, _: &_| write_input_data::<f32>(data, mic_sample_rate as i32, channels, &tx_audio_recording,  &clip_2),
                                                err_fn,
                                                None,
                                            )?,
                                            sample_format => {
                                                return Err(RecordingErrors::UnsupportedSampleFormat(format!(
                                                    "{sample_format}"
                                                )))
                                            }
                                        };
                                stream = Some(new_stream);
                                stream.as_mut().unwrap().play()?;
                                continue;
                            }

                            // if it exists, it may be paused. So play it
                            match stream.as_mut().unwrap().play() {
                                Ok(_) => {},
                                Err(e) => {
                                    error!("Error resuming stream: {:?}", e);
                                    // TODO: maybe try to create a new stream?
                                }
                            }
                        },
                        Some(false) => {
                            if let Some(unwrapped_stream) = stream.as_mut() {
                                match unwrapped_stream.pause() {
                                    Ok(_) => {},
                                    Err(e) => {
                                        error!("Error pausing stream: {:?}", e);
                                    }
                                }
                            }
                        }
                        None => {
                            // channel closed, exit the loop
                            error!("Recording thread channel closed");
                            break;
                        }
                    }
                },
                _ = cancellation_token.cancelled() => {
                    info!("Recording thread cancelled");
                    break;
                }
            }
        }

        info!("Recording thread exited");
        Ok(())
    }
}
