
use anyhow::{Result, Context, bail};
use crate::audio::Speaker;
use byteorder::NetworkEndian;
use byteorder::ByteOrder;

use tokio::prelude::*;
use tokio::task;
use tokio::net::udp;

pub async fn run_network(mut speaker: Speaker, mut socket: udp::RecvHalf) -> Result<()> {
    let mut decoder = decoder_from_format(speaker.format())?;
    let chans = speaker.format().channels as usize;
    let mut sample_buf = vec![0f32; 2880 * chans];
    let mut encoded_buf = vec![0u8; 4000 + 2];
    
    speaker.play()?;

    let mut last_seq = 0;

    loop {
        let n = socket.recv(&mut encoded_buf).await?;
        let msg = &encoded_buf[..n];

        let seq = NetworkEndian::read_u16(&msg[0..2]);

        if seq > last_seq {
            let diff = seq - last_seq;
            let mut msg = Some(&msg[2..]);
            for _ in 0..diff {
                let n = task::block_in_place(|| {
                    decoder.decode_float(msg.take(), &mut sample_buf, false)
                })?;

                for sample in &sample_buf[..n * chans] {
                    speaker.send(*sample).await?;
                }        
            }
            last_seq = seq;
        }
    }
}

#[allow(dead_code)]
pub async fn run(mut speaker: Speaker) -> Result<()> {

    let mut decoder = decoder_from_format(speaker.format())?;
    let mut sample_buf = vec![0f32; 960 * 2];
    let mut encoded_buf = vec![0u8; 4000];

    let mut file = tokio::fs::File::open("encoded.opus").await?;

    speaker.play()?;
    let chans = speaker.format().channels as usize;
    loop {
        let n = file.read_u64().await? as usize;
        match file.read_exact(&mut encoded_buf[..n]).await {
            Ok(_) => {},
            Err(e) if e.kind() == tokio::io::ErrorKind::UnexpectedEof => break,
            Err(e) => bail!(e),
        }
        let n = task::block_in_place(|| {
            decoder.decode_float(Some(&encoded_buf[..n]), &mut sample_buf, false)
        })?;

        for sample in &sample_buf[..n * chans] {
            speaker.send(*sample).await?;
        }
    }
    Ok(())
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