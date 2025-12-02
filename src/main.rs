use anyhow::Result;
use cpal::{
    BufferSize, InputCallbackInfo, OutputCallbackInfo, Sample, StreamConfig,
    traits::{DeviceTrait, HostTrait},
};
use ringbuf::{
    CachingCons, CachingProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};
use rubato::{
    Resampler, SincFixedOut, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const CPAL_BUFFER_SIZE: usize = 480;

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
            stream_config.buffer_size = BufferSize::Fixed(CPAL_BUFFER_SIZE as u32);

            let frame_count = Arc::new(AtomicU64::new(0));
            let num_channels = stream_config.channels as usize;

            let ring_buf = HeapRb::new((CPAL_BUFFER_SIZE * num_channels) * 4);
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

            let sample_rate = stream_config.sample_rate.0 as usize;

            let resample_ratio = 48_000.0f64 / sample_rate as f64;
            let max_relative_resample_ratio = 1.1;
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                oversampling_factor: 128,
                interpolation: SincInterpolationType::Cubic,
                window: WindowFunction::Blackman,
            };
            let chunk_size = 480;

            let resampler = SincFixedOut::new(
                resample_ratio,
                max_relative_resample_ratio,
                params,
                chunk_size,
                num_channels,
            )
            .expect("Should be able to construct the resampler");

            let input_stream = InputStream {
                device_name: device_name.clone(),
                num_channels: stream_config.channels as usize,
                sample_rate: stream_config.sample_rate.0 as usize,
                stream,
                ring_rx,
                resampler: Some(InputResampler::new(resampler)),
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
            stream_config.buffer_size = BufferSize::Fixed(CPAL_BUFFER_SIZE as u32);

            let timeout = None;
            let frame_count = Arc::new(AtomicU64::new(0));
            let num_channels = stream_config.channels as usize;

            let ring_buf = HeapRb::new((CPAL_BUFFER_SIZE * num_channels) * 4);
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

            let first_input_stream = &mut self.input_streams[0];

            if let Some(resampler) = &mut first_input_stream.resampler {
                let input_frames_needed = resampler.resampler.input_frames_next();

                // If our input ring buffer has enough samples for the resampler, then resample into the output buffer.
                if first_input_stream.ring_rx.occupied_len() >= input_frames_needed {
                    let _num = first_input_stream
                        .ring_rx
                        .pop_slice(&mut resampler.input_buffer[0][..input_frames_needed]);

                    if let Err(e) = resampler.resampler.process_into_buffer(
                        &resampler.input_buffer,
                        &mut resampler.output_buffer,
                        None,
                    ) {
                        println!("Resampler error: {e}");
                    } else {
                        for sample in &resampler.output_buffer[0][..480] {
                            let _ = self.output_streams[0].ring_tx.try_push(*sample);
                            let _ = self.output_streams[0].ring_tx.try_push(*sample);
                        }
                    }
                }
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
    resampler: Option<InputResampler>,
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

pub struct InputResampler {
    resampler: SincFixedOut<f32>,
    input_buffer: Vec<Vec<f32>>,
    output_buffer: Vec<Vec<f32>>,
}

impl InputResampler {
    pub fn new(resampler: SincFixedOut<f32>) -> Self {
        let filled = true;
        let input_buffer = resampler.input_buffer_allocate(filled);
        let output_buffer = resampler.output_buffer_allocate(filled);

        dbg!(input_buffer[0].capacity());
        dbg!(output_buffer[0].capacity());

        Self { resampler, input_buffer, output_buffer }
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
        let mut did_underrun = false;
        // dbg!(output.len());
        self.frame_count.fetch_add((output.len() / self.num_channels) as u64, Ordering::Relaxed);

        for sample in output {
            if let Some(popped) = self.ring_rx.try_pop() {
                *sample = popped.to_sample();
            } else {
                did_underrun = true;
                *sample = 0.0.to_sample();
            }
        }

        if did_underrun {
            println!("underrun");
        }
    }
}

fn deinterleave_into<T: AsMut<[f32]>>(data: &[f32], num_channels: usize, output: &mut [T]) {
    assert!(data.len().is_multiple_of(num_channels));
    assert_eq!(output.len(), num_channels);
    for buf in output.iter_mut() {
        assert_eq!(buf.as_mut().len(), data.len() / num_channels);
    }

    for (channel_index, channel) in output.iter_mut().enumerate() {
        for (sample, multi_sample) in
            channel.as_mut().iter_mut().zip(data.chunks_exact(num_channels))
        {
            *sample = multi_sample[channel_index];
        }
    }
}
