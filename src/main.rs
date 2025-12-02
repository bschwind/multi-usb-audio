use anyhow::Result;
use cpal::{
    BufferSize, InputCallbackInfo, OutputCallbackInfo, Sample, StreamConfig,
    traits::{DeviceTrait, HostTrait},
};
use ringbuf::{
    CachingCons, CachingProd, HeapRb,
    traits::{Consumer, Producer, Split},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

type RingBufferTx = CachingProd<Arc<HeapRb<f32>>>;
type RingBufferRx = CachingCons<Arc<HeapRb<f32>>>;

fn main() -> Result<()> {
    let host = cpal::default_host();
    let devices = host.devices()?;

    let mut input_streams = vec![];
    let mut output_streams = vec![];

    let target_input_device = "Blue Snowball";
    let target_output_device = "Mac mini Speakers";

    for device in devices {
        let device_name = device.name()?;
        dbg!(&device_name);
        dbg!(device.supports_input());
        dbg!(device.supports_output());

        if device.supports_input() && device_name == target_input_device {
            let device_config = device.default_input_config()?;

            let timeout = None;
            let input_format = device_config.sample_format();
            let mut stream_config: StreamConfig = device_config.into();

            dbg!(&stream_config);

            stream_config.channels = 1;
            // stream_config.sample_rate = cpal::SampleRate(44100);
            stream_config.buffer_size = BufferSize::Fixed(480);

            let frame_count = Arc::new(AtomicU64::new(0));
            let num_channels = stream_config.channels as usize;

            let ring_buf = HeapRb::new((480 * num_channels) * 4);
            let (ring_tx, ring_rx) = ring_buf.split();

            let mut stream_callback = InputStreamCallback {
                num_channels,
                ring_tx,
                frame_count: Arc::clone(&frame_count),
            };

            let stream = match input_format {
                cpal::SampleFormat::I8 => device.build_input_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i8>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i16>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::I32 => device.build_input_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i32>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<f32>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                _ => panic!("oh no"),
            }?;

            let input_stream = InputStream {
                device_name: device_name.clone(),
                num_channels: stream_config.channels as usize,
                sample_rate: stream_config.sample_rate.0 as usize,
                stream,
                ring_rx,
                total_frames: frame_count,
                last_frames: 0,
            };

            input_streams.push(input_stream);
        }

        if device.supports_output() && device_name == target_output_device {
            let device_config = device.default_output_config()?;

            let output_format = device_config.sample_format();
            let mut stream_config: StreamConfig = device_config.into();
            dbg!(&stream_config);

            stream_config.channels = 2;
            stream_config.buffer_size = BufferSize::Fixed(480);

            let timeout = None;
            let frame_count = Arc::new(AtomicU64::new(0));
            let num_channels = stream_config.channels as usize;

            let ring_buf = HeapRb::new((480 * num_channels) * 4);
            let (ring_tx, ring_rx) = ring_buf.split();

            let mut stream_callback = OutputStreamCallback {
                num_channels,
                ring_rx,
                frame_count: Arc::clone(&frame_count),
            };

            let stream = match output_format {
                cpal::SampleFormat::I8 => device.build_output_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i8>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::I16 => device.build_output_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i16>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::I32 => device.build_output_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i16>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::F32 => device.build_output_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i16>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                _ => panic!("oh no"),
            }?;

            let output_stream = OutputStream {
                device_name: device_name.clone(),
                num_channels: stream_config.channels as usize,
                sample_rate: stream_config.sample_rate.0 as usize,
                stream,
                ring_tx,
                total_frames: frame_count,
                last_frames: 0,
            };

            output_streams.push(output_stream);
        }
    }

    let mut audio_system = AudioSystem { input_streams, output_streams };

    audio_system.run();

    Ok(())
}

pub struct AudioSystem {
    input_streams: Vec<InputStream>,
    output_streams: Vec<OutputStream>,
}

impl AudioSystem {
    pub fn run(&mut self) {
        let now = Instant::now();
        let mut last_print = now;

        let mut input_slice = [0.0; 480];

        while now.elapsed() < Duration::from_secs(10) {
            if last_print.elapsed() >= Duration::from_secs(1) {
                for stream in &mut self.input_streams {
                    let total_frames = stream.total_frames.load(Ordering::Relaxed);
                    let diff = total_frames - stream.last_frames;
                    println!(
                        "Stream '{}' frames: {} (+{})",
                        stream.device_name, total_frames, diff
                    );

                    stream.last_frames = total_frames;
                }

                for stream in &mut self.output_streams {
                    let total_frames = stream.total_frames.load(Ordering::Relaxed);
                    let diff = total_frames - stream.last_frames;
                    println!(
                        "Stream '{}' frames: {} (+{})",
                        stream.device_name, total_frames, diff
                    );

                    stream.last_frames = total_frames;
                }

                last_print = Instant::now();
            }

            let num = self.input_streams[0].ring_rx.pop_slice(&mut input_slice);

            for sample in &input_slice[..num] {
                let _ = self.output_streams[0].ring_tx.try_push(*sample);
                let _ = self.output_streams[0].ring_tx.try_push(*sample);
            }

            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

pub struct InputStream {
    device_name: String,
    num_channels: usize,
    sample_rate: usize,
    stream: cpal::Stream,
    ring_rx: RingBufferRx,
    total_frames: Arc<AtomicU64>,
    last_frames: u64,
}

pub struct InputStreamCallback {
    num_channels: usize,
    ring_tx: RingBufferTx,
    frame_count: Arc<AtomicU64>,
}

impl InputStreamCallback {
    pub fn process<T>(&mut self, input: &[T], _callback_info: &InputCallbackInfo)
    where
        T: Sample,
        f32: cpal::FromSample<T>,
    {
        // dbg!(input.len());
        self.frame_count.fetch_add((input.len() / self.num_channels) as u64, Ordering::Relaxed);

        for sample in input {
            let float_sample = sample.to_sample::<f32>();
            if let Err(_e) = self.ring_tx.try_push(float_sample) {
                // println!("Error on input stream: {e:?}");
            }
        }
    }
}

pub struct OutputStream {
    device_name: String,
    num_channels: usize,
    sample_rate: usize,
    stream: cpal::Stream,
    ring_tx: RingBufferTx,
    total_frames: Arc<AtomicU64>,
    last_frames: u64,
}

pub struct OutputStreamCallback {
    num_channels: usize,
    ring_rx: RingBufferRx,
    frame_count: Arc<AtomicU64>,
}

impl OutputStreamCallback {
    pub fn process<T>(&mut self, output: &mut [T], _callback_info: &OutputCallbackInfo)
    where
        T: Sample + cpal::FromSample<f32>,
    {
        // dbg!(output.len());
        self.frame_count.fetch_add((output.len() / self.num_channels) as u64, Ordering::Relaxed);

        for sample in output {
            *sample = self.ring_rx.try_pop().unwrap_or(0.0).to_sample();
        }
    }
}
