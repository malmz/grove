
use anyhow::{anyhow, Result};
use std::fs::File;
use std::io::Read;
use audiopus::coder::Decoder;
use audiopus::{
    SampleRate,
    Channels,
};

fn main() -> Result<()> {
    let mut file = File::open("encoded.opus")?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let mut count_buf = [0u8; 8];
    for (c, b) in count_buf.iter_mut().zip(&buf) {
        *c = *b;
    }
    let (_, buf) = buf.split_at(8);
    let count = usize::from_ne_bytes(count_buf);
    let frame = &buf[..count];
    let decoder = Decoder::new(SampleRate::Hz48000, Channels::Stereo)?;
    let output = vec![0, 2880 * 2];
    decoder.decode_float()
    Ok(())
}