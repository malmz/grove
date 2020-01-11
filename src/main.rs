mod async_spsc;
// mod cpal_stuff;
mod audio;
mod encoder;
mod decoder;

use anyhow::bail;
use anyhow::Result;


use tokio::io;
use tokio::task;
use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args: pico_args::Arguments = pico_args::Arguments::from_env();
    let (speaker, mic) = audio::init()?;
    if args.contains("-e") {
        task::spawn(encoder::run(mic));
    } else if args.contains("-d") {
        task::spawn(decoder::run(speaker));
    } else {
        bail!("Need to provide a flag")
    }
    io::stdin().read_u8().await?;
    Ok(())
}
