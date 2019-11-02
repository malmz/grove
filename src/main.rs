use tokio::prelude::*;
use cpal::traits::{DeviceTrait, EventLoopTrait, HostTrait};
use failure::ResultExt;
use bytes::BufMut;
use std::sync::{
    Arc,
    Mutex,
};
use tokio::io::AsyncWriteExt;
use anyhow::Result;
use anyhow::anyhow;

fn create_host() -> cpal::Host {
    cpal::default_host() 
}

fn create_device(host: &cpal::Host) -> Result<cpal::Device> { 
    let device = host.default_input_device().ok_or(anyhow!("Failed to get default device"))?;
    println!("Default input device: {}", device.name().compat()?);
    Ok(device)
}

fn get_best_format(device: &cpal::Device) -> Result<cpal::SampleFormat> {
    let mut supported_formats: Vec<cpal::SupportedFormat> = device.supported_input_formats()?.collect();
    supported_formats.sort_by(compare_format);
    for format in &supported_formats {
        println!("Formats: {:?}", format);
    }
    let supported_format = supported_formats.pop().ok_or(anyhow!("No format supported"))?;
    let sample_rate = highest_supported_rate(supported_format.max_sample_rate, supported_format.min_sample_rate)?;
    let mut format = supported_format.with_max_sample_rate();
    format.sample_rate = sample_rate;
    println!("Chosen format: {:?}", format);
    Ok(format)
}

fn init_event_loop(host: &cpal::Host, device: &cpal::Device, format: &cpal::Format) -> Result<(cpal::EventLoop, cpal::StreamId)> {
    let event_loop = host.event_loop();
    let stream_id = event_loop.build_input_stream(&device, &format).compat()?;
    event_loop.play_stream(stream_id).compat()?;
}

fn init_cpal() -> Result<>

fn main() -> Result<()>{
        // Use the default host for working with audio devices.
    let host = cpal::default_host();

    // Setup the default input device and stream with the default input format.
    let device = host.default_input_device().expect("Failed to get default input device");
    println!("Default input device: {}", device.name().compat()?);
    let mut f: Vec<_> = device.supported_input_formats().unwrap().collect();
    f.sort_by(compare_format);
    for format in &f {
        println!("Formats: {:?}", format);
    } 

    let format = f.pop().unwrap();
    let sr = highest_supported_rate(format.max_sample_rate, format.min_sample_rate).unwrap();
    let mut format = format.with_max_sample_rate();
    format.sample_rate = sr;

    // let format = device.default_input_format().expect("Failed to get default input format");
    println!("Chosen input format: {:?}", format);
    let event_loop = host.event_loop();
    let stream_id = event_loop.build_input_stream(&device, &format).compat()?;
    event_loop.play_stream(stream_id).compat()?;



    // A flag to indicate that recording is in progress.
    println!("Begin recording...");
    let recording = Arc::new(std::sync::atomic::AtomicBool::new(true));


    let (mut tx, mut rx) = tokio::sync::mpsc::channel(5);

    let recording_2 = recording.clone();

    let rt = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let buf = Arc::new(Mutex::new(vec![0; 1024 * 8]));
    let rt_2 = rt.clone();
    std::thread::spawn(move || {        
        event_loop.run(move |id, event| {
            let data = match event {
                Ok(data) => data,
                Err(err) => {
                    eprintln!("an error occurred on stream {:?}: {}", id, err);
                    return;
                }
            };

            // If we're done recording, return early.
            if !recording_2.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            // Otherwise write to the wav writer.
            match data {
                cpal::StreamData::Input { buffer: cpal::UnknownTypeInputBuffer::U16(buffer) } => {
                    
                    let buf: Vec<i16> = buffer.iter().map(cpal::Sample::to_i16).collect();

                    let n = unsafe {
                        let n = encoder.encode(&buf, outbuf.bytes_mut()).unwrap();
                        outbuf.advance_mut(n);
                        n
                    };

                    rt_2.block_on(tx.send(outbuf.split_to(n))).unwrap();
                },
                cpal::StreamData::Input { buffer: cpal::UnknownTypeInputBuffer::I16(buffer) } => {
                    let n = unsafe {
                        let n = encoder.encode(&buffer, outbuf.bytes_mut()).unwrap();
                        outbuf.advance_mut(n);
                        n
                    };

                    rt_2.block_on(tx.send(outbuf.split_to(n))).unwrap();
                },
                cpal::StreamData::Input { buffer: cpal::UnknownTypeInputBuffer::F32(buffer) } => {
                    dbg!(&buffer.len());
                    let n = unsafe {
                        let n = encoder.encode_float(&buffer, outbuf.bytes_mut()).unwrap();
                        outbuf.advance_mut(n);
                        n
                    };

                    rt_2.block_on(tx.send(outbuf.split_to(n))).unwrap();
                },
                _ => (),
            }
        });
    });
    let res: Result<(), std::io::Error> = rt.block_on(async {
        use tokio::fs;
        let mut file = fs::File::create("out.opus").await?;
        while let Some(data) = rx.recv().await {
            file.write_all(&data).await?;
        }
        Ok(())
    });
    // Let recording go for roughly three seconds.
    std::thread::sleep(std::time::Duration::from_secs(3));
    recording.store(false, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

fn encoder_from_spec(format: &cpal::Format) -> audiopus::coder::Encoder {
    use audiopus::SampleRate;
    use audiopus::Channels;
    use audiopus::Application;

    let rate = match format.sample_rate.0 {
        8000 => SampleRate::Hz8000,
        12000 => SampleRate::Hz12000,
        16000 => SampleRate::Hz16000,        
        24000 => SampleRate::Hz24000,
        48000 => SampleRate::Hz48000,
        e => panic!("Unsupported rate: {}", e),
    };
    
    let chan = match format.channels {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        e => panic!("Too many channels: {}", e),
    };

    audiopus::coder::Encoder::new(
        rate,
        chan,
        Application::Voip,
    ).expect("failed to create opus encoder")
}

fn compare_format(a: &cpal::SupportedFormat, b: &cpal::SupportedFormat) -> std::cmp::Ordering {
    use std::cmp::Ordering::Equal;
    use cpal::SampleFormat::{F32, I16, U16};

    // Stereo
    let cmp = (a.channels == 2).cmp(&(b.channels == 2));
    if cmp != Equal { return cmp }

    // Mono
    let cmp = (a.channels == 1).cmp(&(b.channels == 1));
    if cmp != Equal { return cmp }

    // Channels
    let cmp = a.channels.cmp(&b.channels);
    if cmp != Equal { return cmp }

    let cmp = (a.data_type == F32).cmp(&(b.data_type == F32));
    if cmp != Equal { return cmp }

    let cmp = (a.data_type == I16).cmp(&(b.data_type == I16));
    if cmp != Equal { return cmp }

    let cmp = (a.data_type == U16).cmp(&(a.data_type == U16));
    if cmp != Equal { return cmp }

    compare_rate((a.max_sample_rate, a.min_sample_rate), (b.max_sample_rate, b.min_sample_rate))
}

fn compare_rate(a: (cpal::SampleRate, cpal::SampleRate), b: (cpal::SampleRate, cpal::SampleRate)) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let ah = highest_supported_rate(a.0, a.1);
    let bh = highest_supported_rate(b.0, b.1);

    match (ah, bh) {
        (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (Some(am), Some(bm)) => am.0.cmp(&bm.0),
    }
}

fn highest_supported_rate(max: cpal::SampleRate, min: cpal::SampleRate) -> Result<cpal::SampleRate> {
    match (max.0, min.0) {
        (max, min) if max >= 48000 && min <= 48000 => Some(cpal::SampleRate(48000)),
        (max, min) if max >= 24000 && min <= 24000 => Some(cpal::SampleRate(24000)),
        (max, min) if max >= 16000 && min <= 16000 => Some(cpal::SampleRate(16000)),
        (max, min) if max >= 12000 && min <= 12000 => Some(cpal::SampleRate(12000)),
        (max, min) if max >= 8000 && min <= 8000 => Some(cpal::SampleRate(8000)),
        _ => anyhow!("No sample rate supported"),
    }
}