use cpal::traits::{DeviceTrait, EventLoopTrait, HostTrait};
use failure::ResultExt;
use bytes::BufMut;
use std::sync::{
    Arc,
};
use parking_lot::{Mutex, Condvar};
use anyhow::Result;
use anyhow::anyhow;
use cpal::Sample;
use byteorder::{ByteOrder, NativeEndian};
use std::io::Read;
use std::io::Write;
use crossbeam::queue::spsc;
use crossbeam::queue::{PushError, PopError};

fn create_host() -> cpal::Host {
    cpal::default_host() 
}

fn create_device(host: &cpal::Host) -> Result<cpal::Device> { 
    let device = host.default_input_device().ok_or_else(|| anyhow!("Failed to get default device"))?;
    println!("Default input device: {}", device.name().compat()?);
    Ok(device)
}

fn get_best_format(device: &cpal::Device) -> Result<cpal::Format> {
    let mut supported_formats: Vec<cpal::SupportedFormat> = device.supported_input_formats().compat()?.collect();
    supported_formats.sort_by(compare_format);
    /* for format in &supported_formats {
        println!("Formats: {:?}", format);
    } */
    let supported_format = supported_formats.pop().ok_or_else(|| anyhow!("No format supported"))?;
    let sample_rate = highest_supported_rate(supported_format.max_sample_rate, supported_format.min_sample_rate).ok_or_else(|| anyhow!("No sample rate supported"))?;
    let mut format = supported_format.with_max_sample_rate();
    format.sample_rate = sample_rate;
    println!("Chosen format: {:?}", format);
    Ok(format)
}

fn init_event_loop(host: &cpal::Host, device: &cpal::Device, format: &cpal::Format) -> Result<(cpal::EventLoop, cpal::StreamId)> {
    let event_loop = host.event_loop();
    let stream_id = event_loop.build_input_stream(&device, &format).compat()?;
    event_loop.play_stream(stream_id.clone()).compat()?;
    Ok((event_loop, stream_id))
}

fn init_cpal() -> Result<(cpal::Host, cpal::Device, cpal::Format, cpal::EventLoop, cpal::StreamId)> {
    let host = create_host();
    let device = create_device(&host)?;
    let format = get_best_format(&device)?;
    let (event_loop, stream_id) = init_event_loop(&host, &device, &format)?;
    Ok((host, device, format, event_loop, stream_id))
}

fn sample_to_bytes<T: Sample>(val: T) -> [u8; 4] {
    val.to_f32().to_bits().to_ne_bytes()
}

fn largest_fram_size(len: usize) -> usize {
    debug_assert!(len >= 120);
    match len {
        x if x >= 2880 => 2880,
        x if x >= 1920 => 1920,
        x if x >= 960 => 960,
        x if x >= 480 => 480,
        x if x >= 240 => 240,
        x if x >= 120 => 120,
        _ => unreachable!(),
    }
}

fn main() -> Result<()> {
    let (host, device, format, event_loop, stream_id) = init_cpal()?;
    let event_loop = Arc::new(event_loop);

    let (tx, rx) = spsc::new::<f32>(1024);
    let can_read = Arc::new((Mutex::new(false), Condvar::new()));

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sound_handle = {
        let format = format.clone();
        let stop = stop.clone();
        let event_loop = event_loop.clone();
        let can_read = can_read.clone();
        std::thread::spawn(move || {
            let &(ref lock, ref cvar) = &*can_read;
            event_loop.run(move |id, event| {
                let data = match event {
                    Ok(data) => data,
                    Err(err) => {
                        eprintln!("an error occurred on stream {:?}: {}", id, err);
                        return;
                    }
                };
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let mut read = lock.lock();
                    *read = true;
                    cvar.notify_one();
                    println!("Stopping");
                    panic!("Akwardly closing cpal thread");
                    //return;
                }
                match data {
                    cpal::StreamData::Input { buffer: cpal::UnknownTypeInputBuffer::U16(buffer) } => {
                        for sample in buffer.iter().map(Sample::to_f32) {
                            tx.push(sample).ok();
                        }
                    },
                    cpal::StreamData::Input { buffer: cpal::UnknownTypeInputBuffer::I16(buffer) } => {
                        for sample in buffer.iter().map(Sample::to_f32) {
                            tx.push(sample).ok();
                        }
                    },
                    cpal::StreamData::Input { buffer: cpal::UnknownTypeInputBuffer::F32(buffer) } => {
                        for sample in buffer.iter() {
                            tx.push(*sample).ok();
                        }
                    },
                    _ => (),
                }
                let mut read = lock.lock();
                *read = true;
                cvar.notify_one();
            })
        })
    };
    let stop2 = stop.clone();
    let encoder_handle = std::thread::spawn(move || {
        let encoder = encoder_from_spec(&format);
        let mut temp_buf = [0f32; 2880 * 2];
        let mut out_buf = vec![0u8; 4000];
        let mut file = std::fs::File::create("encoded.opus").unwrap();
        let &(ref lock, ref cvar) = &*can_read;
        let mut n = 0usize;
        'outer: loop {
            
            // Wait for audio to be avalible
            let mut read = lock.lock();
            if !*read {
                cvar.wait(&mut read);
            }
            *read = false;

            if stop2.load(std::sync::atomic::Ordering::Relaxed) { break; }

            for slot in temp_buf.iter_mut().skip(n) {
                if let Ok(val) = rx.pop() {
                    *slot = val;
                    n += 1;
                } else if n >= 120 * 2 {
                    break;
                } else {
                    continue 'outer;
                }
            }
            debug_assert!(n & 1 == 0);
            dbg!(n);
            let frame_size = largest_fram_size(n>>1);
            let frame = &temp_buf[..frame_size*2];
    
            let en = encoder.encode_float(&frame, &mut out_buf).expect("Failed to encode");
            file.write_all(&en.to_ne_bytes()).unwrap();
            file.write_all(&out_buf[..en]).unwrap();
            temp_buf.copy_within(frame_size*2..n, 0);
            n -= frame_size * 2;
        }
        file.flush().unwrap();
    });
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    println!("Trying to stop");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);   
    sound_handle.join().ok();
    println!("Stopped cpal thread");
    event_loop.destroy_stream(stream_id);    
    encoder_handle.join().expect("Error in encoder thread");
    println!("Clean shutdown");
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

fn highest_supported_rate(max: cpal::SampleRate, min: cpal::SampleRate) -> Option<cpal::SampleRate> {
    match (max.0, min.0) {
        (max, min) if max >= 48000 && min <= 48000 => Some(cpal::SampleRate(48000)),
        (max, min) if max >= 24000 && min <= 24000 => Some(cpal::SampleRate(24000)),
        (max, min) if max >= 16000 && min <= 16000 => Some(cpal::SampleRate(16000)),
        (max, min) if max >= 12000 && min <= 12000 => Some(cpal::SampleRate(12000)),
        (max, min) if max >= 8000 && min <= 8000 => Some(cpal::SampleRate(8000)),
        _ => None,
    }
}