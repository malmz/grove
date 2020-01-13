use anyhow::{Result, Context};
use crate::async_spsc::{Producer, Consumer};
use cpal::traits::{DeviceTrait, EventLoopTrait, HostTrait};
use cpal::{Device, Format, SupportedFormat, SampleRate, SampleFormat};
use std::sync::Arc;

enum Direction {
    In, 
    Out,
}

struct Bufs {
    sbuf: Consumer<f32>,
    mbuf: Producer<f32>,
}

struct Engine {
    event_loop: cpal::EventLoop,
}

pub struct Speaker {
    id: cpal::StreamId,
    format: cpal::Format,
    buf: Producer<f32>,
    engine: Arc<Engine>,
}

impl Speaker {

    pub async fn send(&mut self, sample: f32) -> Result<(), crate::async_spsc::error::PushError<f32>> {
        self.buf.send(sample).await
    }

    pub fn play(&self) -> Result<(), cpal::PlayStreamError> {
        self.engine.event_loop.play_stream(self.id.clone())
    }

    pub fn pause(&self) -> Result<(), cpal::PauseStreamError> {
        self.engine.event_loop.pause_stream(self.id.clone())
    }

    pub fn format(&self) -> &cpal::Format {
        &self.format
    }
}

pub struct Mic {
    id: cpal::StreamId,
    format: cpal::Format,
    buf: Consumer<f32>,
    engine: Arc<Engine>,
}

impl Mic {

    pub async fn recv(&mut self) -> Result<f32, crate::async_spsc::error::PopError> {
        self.buf.recv().await
    }

    pub fn play(&self) -> Result<(), cpal::PlayStreamError> {
        self.engine.event_loop.play_stream(self.id.clone())
    }

    pub fn pause(&self) -> Result<(), cpal::PauseStreamError> {
        self.engine.event_loop.pause_stream(self.id.clone())
    }

    pub fn format(&self) -> &cpal::Format {
        &self.format
    }
}

const BUF_LEN: usize = 2880 * 2;

pub fn init() -> Result<(Speaker, Mic)> {
    let host = cpal::default_host();
    let input_device = host.default_input_device().context("Failed to get defaut input device")?;
    let output_device = host.default_output_device().context("Failed to get defaut output device")?;
    let input_format = get_best_format(&input_device, Direction::In).context("No supported input format")?;
    let output_format = get_best_format(&output_device, Direction::Out).context("No supported output format")?;

    let event_loop = host.event_loop();

    let input_stream_id = event_loop.build_input_stream(&input_device, &input_format)?;
    let output_stream_id = event_loop.build_output_stream(&output_device, &output_format)?;



    let (stx, srx) = crate::async_spsc::new(BUF_LEN);
    let (mtx, mrx) = crate::async_spsc::new(BUF_LEN);

    let engine = Arc::new(Engine {
        event_loop,
    });

    std::thread::spawn({
        let engine = engine.clone();
        let mut bufs = Bufs {
            sbuf: srx,
            mbuf: mtx,
        };
        move || {
            engine.event_loop.run(|id, stream| {
                let data = match stream {
                    Ok(data) => data,
                    Err(err) => {
                        eprintln!("an error occurred on stream {:?}: {}", id, err);
                        return;
                    }
                };
                audio_callback(id, data, &mut bufs);
            });
        }
    });
    Ok((
        Speaker {
            id: output_stream_id,
            format: output_format,
            buf: stx,
            engine: engine.clone(),
        },
        Mic {
            id: input_stream_id,
            format: input_format,
            buf: mrx,
            engine
        }
    ))
}

fn audio_callback(_id: cpal::StreamId, data: cpal::StreamData, bufs: &mut Bufs) {
    use cpal::StreamData;
    use cpal::{UnknownTypeOutputBuffer, UnknownTypeInputBuffer};
    use cpal::Sample;
    match data {
        StreamData::Output { buffer: UnknownTypeOutputBuffer::U16(mut buffer) } => {
            for slot in buffer.iter_mut() {
                *slot = match bufs.sbuf.pop() {
                    Ok(v) => v.to_u16(),
                    Err(_) => return, 
                }
            }
            bufs.sbuf.notify();
        },
        StreamData::Output { buffer: UnknownTypeOutputBuffer::I16(mut buffer) } => {
            for slot in buffer.iter_mut() {
                *slot = match bufs.sbuf.pop() {
                    Ok(v) => v.to_i16(),
                    Err(_) => return, 
                }
            }
            bufs.sbuf.notify();
        },
        StreamData::Output { buffer: UnknownTypeOutputBuffer::F32(mut buffer) } => {
            for slot in buffer.iter_mut() {
                *slot = match bufs.sbuf.pop() {
                    Ok(v) => v,
                    Err(_) => return, 
                }
            }
            bufs.sbuf.notify();
        },
        StreamData::Input { buffer: UnknownTypeInputBuffer::U16(buffer) } => {
            for sample in buffer.iter().map(Sample::to_f32) {
                bufs.mbuf.push(sample).ok();
            }
            bufs.mbuf.notify();
        },
        StreamData::Input { buffer: UnknownTypeInputBuffer::I16(buffer) } => {
            for sample in buffer.iter().map(Sample::to_f32) {
                bufs.mbuf.push(sample).ok();
            }
            bufs.mbuf.notify();
        },
        StreamData::Input { buffer: UnknownTypeInputBuffer::F32(buffer) } => {
            for sample in buffer.iter() {
                bufs.mbuf.push(*sample).ok();
            }
            bufs.mbuf.notify();
        },
    }
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
    dbg!(&format);
    Ok(format)
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