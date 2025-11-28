use anyhow::Result;
use cpal::{
    BufferSize, Sample, StreamConfig,
    traits::{DeviceTrait, HostTrait},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

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

        if device.supports_input() && device_name == target_input_device {
            println!("Supports input!");
            let device_config = device.default_input_config()?;

            let timeout = None;
            let input_format = device_config.sample_format();
            let mut stream_config: StreamConfig = device_config.into();

            dbg!(&stream_config);

            stream_config.channels = 1;
            // stream_config.sample_rate = cpal::SampleRate(44100);
            stream_config.buffer_size = BufferSize::Fixed(480);

            let sample_count = Arc::new(AtomicU64::new(0));
            let sample_count_clone = Arc::clone(&sample_count);

            let stream = match input_format {
                cpal::SampleFormat::I8 => device.build_input_stream(
                    &stream_config,
                    move |data, _| handle_input_data::<i8>(data, Arc::clone(&sample_count_clone)),
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    &stream_config,
                    move |data, _| handle_input_data::<i16>(data, Arc::clone(&sample_count_clone)),
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::I32 => device.build_input_stream(
                    &stream_config,
                    move |data, _| handle_input_data::<i32>(data, Arc::clone(&sample_count_clone)),
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &stream_config,
                    move |data, _| handle_input_data::<f32>(data, Arc::clone(&sample_count_clone)),
                    |_| {},
                    timeout,
                ),
                _ => panic!("oh no"),
            }?;

            let input_stream = InputStream {
                device_name: device_name.clone(),
                num_channels: stream_config.channels as usize,
                stream,
                total_samples: sample_count,
                last_samples: 0,
            };

            input_streams.push(input_stream);
        }

        if device.supports_output() && device_name == target_output_device {
            println!("Supports output!");
            let device_config = device.default_output_config()?;

            let output_format = device_config.sample_format();
            let mut stream_config: StreamConfig = device_config.into();
            dbg!(&stream_config);

            stream_config.buffer_size = BufferSize::Fixed(480);

            let timeout = None;
            let sample_count = Arc::new(AtomicU64::new(0));
            let sample_count_clone = Arc::clone(&sample_count);

            let stream = match output_format {
                cpal::SampleFormat::I8 => device.build_output_stream(
                    &stream_config,
                    move |data, _| handle_output_data::<i8>(data, Arc::clone(&sample_count_clone)),
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::I16 => device.build_output_stream(
                    &stream_config,
                    move |data, _| handle_output_data::<i16>(data, Arc::clone(&sample_count_clone)),
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::I32 => device.build_output_stream(
                    &stream_config,
                    move |data, _| handle_output_data::<i32>(data, Arc::clone(&sample_count_clone)),
                    |_| {},
                    timeout,
                ),
                cpal::SampleFormat::F32 => device.build_output_stream(
                    &stream_config,
                    move |data, _| handle_output_data::<f32>(data, Arc::clone(&sample_count_clone)),
                    |_| {},
                    timeout,
                ),
                _ => panic!("oh no"),
            }?;

            let output_stream = OutputStream {
                device_name: device_name.clone(),
                num_channels: stream_config.channels as usize,
                stream,
                total_samples: sample_count,
                last_samples: 0,
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
        let now = std::time::Instant::now();

        while now.elapsed() < Duration::from_secs(10) {
            for stream in &mut self.input_streams {
                let total_samples = stream.total_samples.load(Ordering::Relaxed);
                let diff = total_samples - stream.last_samples;
                println!("Stream '{}' samples: {} (+{})", stream.device_name, total_samples, diff);

                stream.last_samples = total_samples;
            }

            for stream in &mut self.output_streams {
                let total_samples = stream.total_samples.load(Ordering::Relaxed);
                let diff = total_samples - stream.last_samples;
                println!("Stream '{}' samples: {} (+{})", stream.device_name, total_samples, diff);

                stream.last_samples = total_samples;
            }

            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

pub struct InputStream {
    device_name: String,
    num_channels: usize,
    stream: cpal::Stream,
    total_samples: Arc<AtomicU64>,
    last_samples: u64,
}

pub struct OutputStream {
    device_name: String,
    num_channels: usize,
    stream: cpal::Stream,
    total_samples: Arc<AtomicU64>,
    last_samples: u64,
}

fn handle_input_data<T>(input: &[T], sample_count: Arc<AtomicU64>)
where
    T: Sample,
    f32: cpal::FromSample<T>,
{
    // dbg!(input.len());
    sample_count.fetch_add(input.len() as u64, Ordering::Relaxed);

    for sample in input {
        let _float_sample = sample.to_sample::<f32>();
    }
}

fn handle_output_data<T>(output: &mut [T], sample_count: Arc<AtomicU64>)
where
    T: Sample + cpal::FromSample<f32>,
{
    // dbg!(output.len());
    sample_count.fetch_add(output.len() as u64, Ordering::Relaxed);

    for sample in output {
        *sample = 0.0.to_sample();
    }
}
