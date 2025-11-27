use anyhow::Result;
use cpal::{
    BufferSize, Sample, SampleRate, StreamConfig,
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

    let mut streams = vec![];

    for device in devices {
        let device_name = device.name()?;
        dbg!(&device_name);

        if device.supports_input() {
            println!("Supports input!");
            let device_config = device.default_input_config()?;

            let timeout = None;
            let input_format = device_config.sample_format();
            let mut stream_config: StreamConfig = device_config.into();

            dbg!(&stream_config);

            stream_config.channels = 1;
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

            let input_stream = AudioInputStream {
                device_name,
                stream,
                total_samples: sample_count,
                last_samples: 0,
            };

            streams.push(input_stream);
        }

        if device.supports_output() {
            println!("Supports output!");
        }
    }

    let now = std::time::Instant::now();

    while now.elapsed() < Duration::from_secs(10) {
        for stream in &mut streams {
            let total_samples = stream.total_samples.load(Ordering::Relaxed);
            let diff = total_samples - stream.last_samples;
            println!(
                "Stream '{}' samples: {} (+{})",
                stream.device_name,
                stream.total_samples.load(Ordering::Relaxed),
                diff
            );

            stream.last_samples = total_samples;
        }

        std::thread::sleep(Duration::from_millis(600));
    }

    Ok(())
}

pub struct AudioInputStream {
    device_name: String,
    stream: cpal::Stream,
    total_samples: Arc<AtomicU64>,
    last_samples: u64,
}

fn handle_input_data<T>(input: &[T], sample_count: Arc<AtomicU64>)
where
    T: Sample,
    f32: cpal::FromSample<T>,
{
    dbg!(input.len());
    // let thing = AtomicU64::new(0);
    sample_count.fetch_add(input.len() as u64, Ordering::Relaxed);

    for sample in input {
        let _float_sample = sample.to_sample::<f32>();
    }
}
