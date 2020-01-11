
use anyhow::{Result, Context, bail};
use crate::audio::Speaker;

use tokio::prelude::*;
use tokio::task;

pub async fn run(mut speaker: Speaker) -> Result<()> {

    let mut decoder = decoder_from_format(speaker.format())?;
    let mut sample_buf = vec![0f32; 960 * 2];
    let mut encoded_buf = vec![0u8; 4000];

    let mut file = tokio::fs::File::open("encoded.opus").await?;

    speaker.play();

    loop {
        let n = file.read_u64().await? as usize;
        file.read_exact(&mut encoded_buf[..n]).await?;
        let n = task::block_in_place(|| {
            decoder.decode_float(Some(&encoded_buf[..n]), &mut sample_buf, false)
        })?;

        for sample in &sample_buf[..n] {
            speaker.send(*sample).await?;
        }
    }
}

fn decoder_from_format(format: &cpal::Format) -> Result<audiopus::coder::Decoder> {
    use audiopus::SampleRate;
    use audiopus::Channels;

    let rate = match format.sample_rate.0 {
        8000 => SampleRate::Hz8000,
        12000 => SampleRate::Hz12000,
        16000 => SampleRate::Hz16000,        
        24000 => SampleRate::Hz24000,
        48000 => SampleRate::Hz48000,
        e => bail!("Unsupported rate: {}", e),
    };

    let chan = match format.channels {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        e => bail!("Too many channels: {}", e),
    };

    Ok(audiopus::coder::Decoder::new(rate, chan).context("failed to create opus decoder")?)
}