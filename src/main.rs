use anyhow::Result;
use cpal::{
    BufferSize, InputCallbackInfo, OutputCallbackInfo, Sample, StreamConfig, StreamError,
    traits::{DeviceTrait, HostTrait},
};
use ringbuf::{
    CachingCons, CachingProd, HeapRb,
    consumer::PopIter,
    traits::{Consumer, Observer, Producer, Split},
};
use rubato::{
    Resampler, SincFixedIn, SincFixedOut, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const CPAL_BUFFER_SIZE: usize = 128;
const USER_BUFFER_SIZE: usize = 480;
const ERROR_BUFFER_SIZE: usize = 10;

type RingBufferTx<T> = CachingProd<Arc<HeapRb<T>>>;
type RingBufferRx<T> = CachingCons<Arc<HeapRb<T>>>;

fn main() -> Result<()> {
    let host = cpal::default_host();
    let devices = host.devices()?;

    let mut input_streams = vec![];
    let mut output_streams = vec![];

    let mut user_input_buffers = vec![];
    let mut input_buffer_index_start = 0;
    let mut user_output_buffers = vec![];
    let mut output_buffer_index_start = 0;

    let target_input_device = ["Blue Snowball"];
    let target_output_device = ["Mac mini Speakers"];

    for device in devices {
        let device_name = device.name()?;
        dbg!(&device_name);
        dbg!(device.supports_input());
        dbg!(device.supports_output());

        if device.supports_input() && target_input_device.contains(&device_name.as_str()) {
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

            let sample_ring_buf = HeapRb::new((USER_BUFFER_SIZE * num_channels) * 4);
            let (sample_tx, sample_rx) = sample_ring_buf.split();

            let error_ring_buf = HeapRb::new(ERROR_BUFFER_SIZE);
            let (mut error_tx, error_rx) = error_ring_buf.split();

            let mut stream_callback = InputStreamCallback {
                num_channels,
                sample_tx,
                frame_count: Arc::clone(&frame_count),
            };

            let stream = match input_format {
                cpal::SampleFormat::I8 => device.build_input_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i8>(data, callback_info);
                    },
                    move |err| {
                        error_tx.try_push(err).expect("Error ring buffer shouldn't be full");
                    },
                    timeout,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i16>(data, callback_info);
                    },
                    move |err| {
                        error_tx.try_push(err).expect("Error ring buffer shouldn't be full");
                    },
                    timeout,
                ),
                cpal::SampleFormat::I32 => device.build_input_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<i32>(data, callback_info);
                    },
                    move |err| {
                        error_tx.try_push(err).expect("Error ring buffer shouldn't be full");
                    },
                    timeout,
                ),
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<f32>(data, callback_info);
                    },
                    move |err| {
                        error_tx.try_push(err).expect("Error ring buffer shouldn't be full");
                    },
                    timeout,
                ),
                _ => panic!("oh no"),
            }?;

            let resampler =
                build_input_resampler(stream_config.sample_rate.0 as usize, num_channels);

            for _ in 0..num_channels {
                user_input_buffers.push([0.0f32; USER_BUFFER_SIZE]);
            }

            let input_stream = InputStream {
                device_name: device_name.clone(),
                num_channels: stream_config.channels as usize,
                sample_rate: stream_config.sample_rate.0 as usize,
                stream,
                sample_rx,
                error_rx,
                resampler: InputResampler::new(resampler),
                total_frames: frame_count,
                last_frames: 0,
                has_errored: false,
                input_buffer_index_start,
                measured_sample_rate: stream_config.sample_rate.0 as f64,
                last_sample_rate_calc_time: Instant::now(),
                is_leader: false,
            };

            input_buffer_index_start += stream_config.channels as usize;

            input_streams.push(input_stream);
        }

        if device.supports_output() && target_output_device.contains(&device_name.as_str()) {
            let device_config = device.default_output_config()?;

            let output_format = device_config.sample_format();
            let mut stream_config: StreamConfig = device_config.into();
            dbg!(&stream_config);

            stream_config.channels = 2;
            stream_config.buffer_size = BufferSize::Fixed(CPAL_BUFFER_SIZE as u32);

            let timeout = None;
            let frame_count = Arc::new(AtomicU64::new(0));
            let num_channels = stream_config.channels as usize;

            let ring_buf = HeapRb::new((USER_BUFFER_SIZE * num_channels) * 4);
            let (sample_tx, sample_rx) = ring_buf.split();

            let mut stream_callback = OutputStreamCallback {
                num_channels,
                sample_rx,
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
                        stream_callback.process::<i32>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::F32 => device.build_output_stream(
                    &stream_config,
                    move |data, callback_info| {
                        stream_callback.process::<f32>(data, callback_info);
                    },
                    |_| {},
                    timeout,
                ),
                _ => panic!("oh no"),
            }?;

            let resampler =
                build_output_resampler(stream_config.sample_rate.0 as usize, num_channels);

            for _ in 0..num_channels {
                user_output_buffers.push([0.0f32; USER_BUFFER_SIZE]);
            }

            let output_stream = OutputStream {
                device_name: device_name.clone(),
                num_channels: stream_config.channels as usize,
                sample_rate: stream_config.sample_rate.0 as usize,
                stream,
                sample_tx,
                resampler: OutputResampler::new(resampler),
                total_frames: frame_count,
                last_frames: 0,
                output_buffer_index_start,
                measured_sample_rate: stream_config.sample_rate.0 as f64,
                last_sample_rate_calc_time: Instant::now(),
                is_leader: false,
            };

            output_buffer_index_start += stream_config.channels as usize;

            output_streams.push(output_stream);
        }
    }

    input_streams[0].is_leader = true;

    let mut audio_system =
        AudioSystem { input_streams, output_streams, user_input_buffers, user_output_buffers };

    audio_system.run();

    Ok(())
}

pub struct AudioSystem {
    input_streams: Vec<InputStream>,
    output_streams: Vec<OutputStream>,
    // An aggregate of all input audio channels after sample conversion and resampling.
    user_input_buffers: Vec<[f32; USER_BUFFER_SIZE]>,
    // An aggregate of all output audio channels in f32, 48kHz format, before sample
    // conversion and resampling.
    user_output_buffers: Vec<[f32; USER_BUFFER_SIZE]>,
}

impl AudioSystem {
    pub fn run(&mut self) {
        let now = Instant::now();
        let mut last_rate_recalculate = now;

        dbg!(self.user_input_buffers.len());
        dbg!(self.user_output_buffers.len());

        while now.elapsed() < Duration::from_secs(100) {
            for stream in &mut self.input_streams {
                if let Some(err) = stream.error_rx.try_pop() {
                    println!("Stream {} got error: {err:?}", stream.device_name);
                    stream.has_errored = true;
                    // TODO(bschwind) - Mark the stream as disconnected, zero its buffers,
                    //                  and try to recreate it.
                }
            }

            if last_rate_recalculate.elapsed() >= Duration::from_secs(4) {
                for stream in &mut self.input_streams {
                    stream.recalculate_sample_rate();
                }

                for stream in &mut self.output_streams {
                    stream.recalculate_sample_rate();
                }

                last_rate_recalculate = Instant::now();
            }

            let mut should_output = false;

            for input_stream in &mut self.input_streams {
                let input_frames_needed = input_stream.resampler.resampler.input_frames_next();

                // If our input ring buffer has enough samples for the resampler, then resample into the output buffer.
                // TODO(bschwind) - It would be better to clock this on a driver device instead of depending on ring buffer fill.
                if !input_stream.has_errored
                    && input_stream.sample_rx.occupied_len()
                        >= input_frames_needed * input_stream.num_channels
                {
                    should_output = true;
                    deinterleave_from_ring_buf(
                        input_stream.sample_rx.pop_iter(),
                        input_stream.num_channels,
                        input_frames_needed,
                        &mut input_stream.resampler.input_buffer,
                    );

                    let user_input_buffer = &mut self.user_input_buffers[input_stream
                        .input_buffer_index_start
                        ..(input_stream.input_buffer_index_start + input_stream.num_channels)];

                    match input_stream.resampler.resampler.process_into_buffer(
                        &input_stream.resampler.input_buffer,
                        user_input_buffer,
                        None,
                    ) {
                        Ok((_input_frames, _output_frames)) => {
                            // user_input_buffer is now ready for this stream
                            // TODO(bschwind) - Assert `input_frames` is the same as `input_frames_needed`.
                        },
                        Err(e) => {
                            println!("Resampler error: {e}");
                        },
                    }
                } else if input_stream.has_errored {
                }
            }

            if should_output {
                big_mix(&self.user_input_buffers, &mut self.user_output_buffers);

                // Resample the output
                for output_stream in &mut self.output_streams {
                    let user_output_buffer = &mut self.user_output_buffers[output_stream
                        .output_buffer_index_start
                        ..(output_stream.output_buffer_index_start + output_stream.num_channels)];

                    match output_stream.resampler.resampler.process_into_buffer(
                        user_output_buffer,
                        &mut output_stream.resampler.output_buffer,
                        None,
                    ) {
                        Ok((_input_frames, output_frames)) => {
                            // Interleave the output channels for this device into the output stream's ring buffer.
                            for sample_idx in 0..output_frames {
                                for out_channel in &output_stream.resampler.output_buffer {
                                    let _ =
                                        output_stream.sample_tx.try_push(out_channel[sample_idx]);
                                }
                            }
                        },
                        Err(e) => {
                            println!("Resampler error: {e}");
                        },
                    }
                }
            }

            // TODO(bschwind) - Find a way to drive this loop in a more efficient manner without using sleep.
            //                  Maybe CondVars, or OS-specific wakeup events like eventfd or kqueue?
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

// Given audio on all input channels, write the resulting audio to the output channels.
// For now it does a simple loopback.
fn big_mix(inputs: &[[f32; USER_BUFFER_SIZE]], outputs: &mut [[f32; USER_BUFFER_SIZE]]) {
    for input_channel in inputs {
        for output_channel in &mut *outputs {
            output_channel.copy_from_slice(input_channel);
        }
    }
}

pub struct InputStream {
    device_name: String,
    num_channels: usize,
    sample_rate: usize,
    stream: cpal::Stream,
    sample_rx: RingBufferRx<f32>,
    error_rx: RingBufferRx<StreamError>,
    resampler: InputResampler,
    total_frames: Arc<AtomicU64>,
    last_frames: u64,
    has_errored: bool,
    input_buffer_index_start: usize,
    measured_sample_rate: f64,
    last_sample_rate_calc_time: Instant,
    is_leader: bool,
}

impl InputStream {
    fn recalculate_sample_rate(&mut self) {
        let total_frames = self.total_frames.load(Ordering::Relaxed);
        let diff = total_frames - self.last_frames;
        println!("Stream '{}' frames: {} (+{})", self.device_name, total_frames, diff);

        let new_measured_sample_rate =
            diff as f64 / self.last_sample_rate_calc_time.elapsed().as_secs_f64();
        self.measured_sample_rate = (new_measured_sample_rate + self.measured_sample_rate) * 0.5;
        dbg!(self.measured_sample_rate);

        self.last_frames = total_frames;
        self.last_sample_rate_calc_time = Instant::now();
    }
}

pub struct InputStreamCallback {
    num_channels: usize,
    sample_tx: RingBufferTx<f32>,
    frame_count: Arc<AtomicU64>,
}

impl InputStreamCallback {
    pub fn process<T>(&mut self, input: &[T], _callback_info: &InputCallbackInfo)
    where
        T: Sample,
        f32: cpal::FromSample<T>,
    {
        let mut did_overrun = false;
        // dbg!(input.len());
        self.frame_count.fetch_add((input.len() / self.num_channels) as u64, Ordering::Relaxed);

        for sample in input {
            let float_sample = sample.to_sample::<f32>();
            if let Err(_e) = self.sample_tx.try_push(float_sample) {
                // println!("Error on input stream: {e:?}");
                did_overrun = true;
            }
        }

        if did_overrun {
            // println!("overrun");
        }
    }
}

pub struct InputResampler {
    resampler: SincFixedOut<f32>,
    input_buffer: Vec<Vec<f32>>,
}

impl InputResampler {
    pub fn new(resampler: SincFixedOut<f32>) -> Self {
        let filled = true;
        let input_buffer = resampler.input_buffer_allocate(filled);

        Self { resampler, input_buffer }
    }
}

pub struct OutputResampler {
    resampler: SincFixedIn<f32>,
    output_buffer: Vec<Vec<f32>>,
}

impl OutputResampler {
    pub fn new(resampler: SincFixedIn<f32>) -> Self {
        let filled = true;
        let output_buffer = resampler.output_buffer_allocate(filled);

        Self { resampler, output_buffer }
    }
}

pub struct OutputStream {
    device_name: String,
    num_channels: usize,
    sample_rate: usize,
    stream: cpal::Stream,
    sample_tx: RingBufferTx<f32>,
    resampler: OutputResampler,
    total_frames: Arc<AtomicU64>,
    last_frames: u64,
    output_buffer_index_start: usize,
    measured_sample_rate: f64,
    last_sample_rate_calc_time: Instant,
    is_leader: bool,
}

impl OutputStream {
    fn recalculate_sample_rate(&mut self) {
        let total_frames = self.total_frames.load(Ordering::Relaxed);
        let diff = total_frames - self.last_frames;
        println!("Stream '{}' frames: {} (+{})", self.device_name, total_frames, diff);

        let new_measured_sample_rate =
            diff as f64 / self.last_sample_rate_calc_time.elapsed().as_secs_f64();
        self.measured_sample_rate = (new_measured_sample_rate + self.measured_sample_rate) * 0.5;
        dbg!(self.measured_sample_rate);

        self.last_frames = total_frames;
        self.last_sample_rate_calc_time = Instant::now();
    }
}

pub struct OutputStreamCallback {
    num_channels: usize,
    sample_rx: RingBufferRx<f32>,
    frame_count: Arc<AtomicU64>,
}

impl OutputStreamCallback {
    pub fn process<T>(&mut self, output: &mut [T], _callback_info: &OutputCallbackInfo)
    where
        T: Sample + cpal::FromSample<f32>,
    {
        let mut did_underrun = false;
        self.frame_count.fetch_add((output.len() / self.num_channels) as u64, Ordering::Relaxed);

        for sample in output {
            if let Some(popped) = self.sample_rx.try_pop() {
                *sample = popped.to_sample();
            } else {
                did_underrun = true;
                *sample = 0.0.to_sample();
            }
        }

        if did_underrun {
            // println!("underrun");
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

fn deinterleave_from_ring_buf<T: AsMut<[f32]>>(
    mut pop_iter: PopIter<RingBufferRx<f32>>,
    num_channels: usize,
    frames_needed: usize,
    output: &mut [T],
) {
    // assert!(pop_iter.len().is_multiple_of(num_channels));
    assert_eq!(output.len(), num_channels);
    // for buf in output.iter_mut() {
    //     assert_eq!(buf.as_mut().len(), pop_iter.len() / num_channels);
    // }

    for i in 0..frames_needed {
        for channel in output.iter_mut() {
            channel.as_mut()[i] = pop_iter.next().expect("PopIter should have enough elements");
        }
    }

    pop_iter.commit();
}

fn build_input_resampler(sample_rate: usize, num_channels: usize) -> SincFixedOut<f32> {
    let resample_ratio = 48_000.0f64 / sample_rate as f64;
    let max_relative_resample_ratio = 1.1;
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        oversampling_factor: 128,
        interpolation: SincInterpolationType::Cubic,
        window: WindowFunction::Blackman,
    };
    let chunk_size = USER_BUFFER_SIZE;

    SincFixedOut::new(resample_ratio, max_relative_resample_ratio, params, chunk_size, num_channels)
        .expect("Should be able to construct the resampler")
}

fn build_output_resampler(sample_rate: usize, num_channels: usize) -> SincFixedIn<f32> {
    let resample_ratio = sample_rate as f64 / 48_000.0f64;
    let max_relative_resample_ratio = 1.1;
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        oversampling_factor: 128,
        interpolation: SincInterpolationType::Cubic,
        window: WindowFunction::Blackman,
    };
    let chunk_size = USER_BUFFER_SIZE;

    SincFixedIn::new(resample_ratio, max_relative_resample_ratio, params, chunk_size, num_channels)
        .expect("Should be able to construct the resampler")
}
