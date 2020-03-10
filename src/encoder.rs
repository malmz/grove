
use anyhow::{Result, Context, bail};
use tokio::task;
use tokio::io::AsyncWriteExt;
use crate::audio::Mic;
use tokio::net::udp;
use byteorder::{NetworkEndian, ByteOrder};

const LATENCY: usize = 960;
//const LATENCY: usize = 120;

pub async fn run_network(mut mic: Mic, mut socket: udp::SendHalf) -> Result<()> {
    let encoder = encoder_from_format(mic.format())?;
    let mut frame = vec![0f32; LATENCY * mic.format().channels as usize];
    let mut out_buf = vec![0u8; 4000 + 2];
    
    mic.play()?;

    let mut seq = 0;

    loop {
        seq += 1;
        for slot in frame.iter_mut() {
            *slot = mic.recv().await?;
        }

        let n = task::block_in_place(|| {
            encoder.encode_float(&frame, &mut out_buf[2..]).context("Failed to encode")
        })?;

        NetworkEndian::write_u16(&mut out_buf[..2], seq);
        socket.send(&out_buf[..n]).await?;
    }
}

pub async fn run(mut mic: Mic) -> Result<()> {
    let encoder = encoder_from_format(mic.format())?;
    let mut frame = vec![0f32; 960 * mic.format().channels as usize];
    let mut out_buf = vec![0u8; 4000];

    let mut file = tokio::fs::File::create("encoded.opus").await?;

    mic.play()?;

    loop {
        for slot in frame.iter_mut() {
            *slot = mic.recv().await?;
        }

        let n = task::block_in_place(|| {
            encoder.encode_float(&frame, &mut out_buf).context("Failed to encode")
        })?;

        file.write_u64(n as u64).await?;
        file.write_all(&out_buf[..n]).await?;
    }
}

fn encoder_from_format(format: &cpal::Format) -> Result<audiopus::coder::Encoder> {
    use audiopus::SampleRate;
    use audiopus::Channels;
    use audiopus::Application;

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

    Ok(audiopus::coder::Encoder::new(
        rate,
        chan,
        Application::Voip,
    ).context("failed to create opus encoder")?)
}