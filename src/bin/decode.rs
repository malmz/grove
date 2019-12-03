
use anyhow::{anyhow, Result};
use std::fs::File;
use std::io::Read;
use std::io::Write;
use audiopus::coder::Decoder;
use audiopus::{
    SampleRate,
    Channels,
};
use cpal::Sample;

fn sample_to_bytes<T: Sample>(val: T) -> [u8; 4] {
    val.to_f32().to_bits().to_ne_bytes()
}

fn main() -> Result<()> {
    let mut file = File::open("encoded.opus")?;
    let mut out_file = File::create("raw.pcm")?;

    let mut buf = Vec::new();
    let mut count_buf = [0u8; 8];
    file.read_to_end(&mut buf)?;
    let mut buf = &buf[..];

    let mut output = vec![0f32; 2880 * 2];
    let mut decoder: Decoder = Decoder::new(SampleRate::Hz48000, Channels::Stereo)?;

    while !buf.is_empty() {
        for (c, b) in count_buf.iter_mut().zip(buf) {
            *c = *b;
        }
        buf = buf.split_at(8).1;
        let count = usize::from_ne_bytes(count_buf);
        let frame = &buf[..count];
        let n = decoder.decode_float(Some(frame), &mut output, false).expect("Failed to decode");
        buf = buf.split_at(count).1;
        let data: Vec<u8> = &output[..n].iter().map(|s| sample_to_bytes(*s)).foldcargo.collect();
        out_file.write_all(&data)?;
    }
    Ok(())
}