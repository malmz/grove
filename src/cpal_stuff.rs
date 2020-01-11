/* use anyhow::Result;
use anyhow::Context as _;
use cpal::{
    EventLoop,
    Host,
    Format,
    Sample,
    SampleRate,
    default_host,
    Device,
    SupportedFormat,
    StreamData,
    StreamId,
    UnknownTypeInputBuffer,
    UnknownTypeOutputBuffer,
    SampleFormat,
    traits::{
        HostTrait,
        DeviceTrait,
        EventLoopTrait,
    },
};
use std::thread::JoinHandle;
use std::sync::Arc;
use crate::StopSignal;
use crate::async_spsc::{Producer, Consumer};
//use ringbuf::{Producer, Consumer};
use std::task::{Context, Poll};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy)]
enum Direction {
    In,
    Out,
}

pub struct AudioController {
    host: Host,
    input_device: Device,
    input_format: Format,
    output_device: Device, 
    output_format: Format,
    engine: Arc<InnerEngine>,
}

struct InnerEngine {
    input_stream: Mutex<Option<(StreamId, Producer<f32>)>>,
    output_stream: Mutex<Option<(StreamId, Consumer<f32>)>>,
    event_loop: EventLoop,
}

fn audio_callback(engine: &InnerEngine, id: StreamId, data: cpal::StreamData) {
    match data {
        StreamData::Input { buffer } => {
            let tx = match engine.input_stream.lock().unwrap() {
                Some(val) => val,
                None => eprintln!("Inputstream still running")
            }
        },
    }
    match data {
        StreamData::Output { buffer: UnknownTypeOutputBuffer::U16(buffer) } => {

        }
        StreamData::Input { buffer: UnknownTypeInputBuffer::U16(buffer) } => {
            for sample in buffer.iter().map(Sample::to_f32) {
                tx.push(sample).ok();
            }
            tx.notify();
        },
        StreamData::Input { buffer: UnknownTypeInputBuffer::I16(buffer) } => {
            for sample in buffer.iter().map(Sample::to_f32) {
                tx.push(sample).ok();
            }
            tx.notify();
        },
        StreamData::Input { buffer: UnknownTypeInputBuffer::F32(buffer) } => {
            for sample in buffer.iter() {
                tx.push(*sample).ok();
            }
            tx.notify();
        },
        _ => (),
    }
}

impl AudioController {
    pub fn new() -> Result<Self> {
        let host = default_host();
        let input_device = create_input_device(&host)?;
        let input_format = get_best_format(&input_device, Direction::In)?;
        let output_device = create_output_device(&host)?;
        let output_format = get_best_format(&output_device, Direction::Out)?;
        
        let engine = Arc::new(InnerEngine {
            input_stream: Mutex::new(None),
            output_stream: Mutex::new(None),
            event_loop: host.event_loop(),
        });
        std::thread::spawn({
            let engine = engine.clone();
            move || {
                engine.event_loop.run(|id, stream| {
                    let data = match stream {
                        Ok(data) => data,
                        Err(err) => {
                            eprintln!("an error occurred on stream {:?}: {}", id, err);
                            return;
                        }
                    };
                    audio_callback(&engine, id, data);
                })
            }
        });

        Ok(Self {
            input_device,
            input_format,
            output_device,
            output_format,
            engine: engine.clone(),
            host,
        })
    }

    pub fn output_sink() {

    }
    pub fn input_stream(&mut self) -> Result<InputStream> {
        unimplemented!();
    }

    pub fn close_input_stream(&mut self, input_stream: InputStream) -> Result<()> {
        let (stream, _) = self.engine.input_stream.lock().unwrap().take().context("No input stream open")?;
        self.engine.event_loop.destroy_stream(stream);
        Ok(())
    }

    pub fn close_output_stream(&mut self, output_stream: OutputStream) -> Result<()> {
        let (stream, _) = self.engine.output_stream.lock().unwrap().take().context("No output stream open")?;
        self.engine.event_loop.destroy_stream(stream);
        Ok(())
    }
}

fn create_host() -> Host {
    default_host() 
}

fn create_input_device(host: &Host) -> Result<Device> { 
    let device = host.default_input_device().context("Failed to get default input device")?;
    let input_device_name = device.name()?;
    dbg!(input_device_name);
    Ok(device)
}

fn create_output_device(host: &Host) -> Result<Device> {
    let device = host.default_output_device().context("Failed to get default output device")?;
    let output_device_name = device.name()?;
    dbg!(output_device_name);
    Ok(device)
} 


fn get_best_format(device: &Device, direction: Direction) -> Result<Format> {
    
    let supported_format = match direction {
        Direction::In => {
            let mut supported_format: Vec<_> = device.supported_input_formats()?.collect();
            supported_format.sort_by(compare_format);
            supported_format.pop().context("No format supported")?
        },
        Direction::Out => {
            let mut supported_format: Vec<_> = device.supported_output_formats()?.collect();
            supported_format.sort_by(compare_format);
            supported_format.pop().context("No format supported")?
        }
    };

    let sample_rate = highest_supported_rate(supported_format.max_sample_rate, supported_format.min_sample_rate).context("No sample rate supported")?;
    let mut format = supported_format.with_max_sample_rate();
    format.sample_rate = sample_rate;
    println!("Chosen format: {:?}", format);
    Ok(format)
}

fn init_event_loop(host: &Host, device: &Device, format: &Format) -> Result<(EventLoop, StreamId)> {
    let event_loop = host.event_loop();
    let stream_id = event_loop.build_input_stream(&device, &format)?;
    event_loop.play_stream(stream_id.clone())?;
    Ok((event_loop, stream_id))
}

pub fn init_cpal() -> Result<(Host, Device, Format, EventLoop, StreamId)> {
    let host = create_host();
    let device = create_input_device(&host)?;
    let format = get_best_format(&device, Direction::In)?;
    let (event_loop, stream_id) = init_event_loop(&host, &device, &format)?;
    Ok((host, device, format, event_loop, stream_id))
}

pub fn sample_to_bytes<T: Sample>(val: T) -> [u8; 4] {
    val.to_f32().to_bits().to_ne_bytes()
}

pub fn largest_fram_size(len: usize) -> usize {
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

pub(crate) fn launch_cpal_thread(stop: StopSignal, event_loop: Arc<EventLoop>) -> (crate::async_spsc::Consumer<f32>, JoinHandle<()>) {
    let (mut tx, rx) = crate::async_spsc::new::<f32>(2880 * 4);
    let handle = std::thread::spawn(move || {
        event_loop.run(move |id, event| {
            let data = match event {
                Ok(data) => data,
                Err(err) => {
                    panic!("an error occurred on stream {:?}: {}", id, err);
                }
            };
            if stop.stopped() {
                tx.notify();
                println!("Audio: Stopping");
                panic!("Akwardly closing cpal thread");
                //return;
            }
            match data {
                StreamData::Output { buffer: UnknownTypeOutputBuffer::U16(buffer) } => {

                }
                StreamData::Input { buffer: UnknownTypeInputBuffer::U16(buffer) } => {
                    for sample in buffer.iter().map(Sample::to_f32) {
                        tx.push(sample).ok();
                    }
                    tx.notify();
                },
                StreamData::Input { buffer: UnknownTypeInputBuffer::I16(buffer) } => {
                    for sample in buffer.iter().map(Sample::to_f32) {
                        tx.push(sample).ok();
                    }
                    tx.notify();
                },
                StreamData::Input { buffer: UnknownTypeInputBuffer::F32(buffer) } => {
                    for sample in buffer.iter() {
                        tx.push(*sample).ok();
                    }
                    tx.notify();
                },
                _ => (),
            }
        })
    });
    (rx, handle)
}

fn compare_format(a: &SupportedFormat, b: &SupportedFormat) -> std::cmp::Ordering {
    use std::cmp::Ordering::Equal;
    use SampleFormat::{F32, I16, U16};

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

fn compare_rate(a: (SampleRate, SampleRate), b: (SampleRate, SampleRate)) -> std::cmp::Ordering {
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

fn highest_supported_rate(max: SampleRate, min: SampleRate) -> Option
<SampleRate> {
    match (max.0, min.0) {
        (max, min) if max >= 48000 && min <= 48000 => Some(SampleRate(48000)),
        (max, min) if max >= 24000 && min <= 24000 => Some(SampleRate(24000)),
        (max, min) if max >= 16000 && min <= 16000 => Some(SampleRate(16000)),
        (max, min) if max >= 12000 && min <= 12000 => Some(SampleRate(12000)),
        (max, min) if max >= 8000 && min <= 8000 => Some(SampleRate(8000)),
        _ => None,
    }
}
*/