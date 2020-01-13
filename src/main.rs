mod async_spsc;
// mod cpal_stuff;
mod audio;
mod encoder;
mod decoder;

use anyhow::bail;
use anyhow::Result;

use futures::future::select;
use futures::future::Either;
use futures::pin_mut;

use tokio::task;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args: pico_args::Arguments = pico_args::Arguments::from_env();
    let (speaker, mic) = audio::init()?;
    let handle = if args.contains("-e") {
        task::spawn(encoder::run(mic))
    } else if args.contains("-d") {
        task::spawn(decoder::run(speaker))
    } else {
        bail!("Need to provide a flag")
    };

    let ctrl_c = tokio::signal::ctrl_c();
    pin_mut!(ctrl_c);

    match select(handle, ctrl_c).await {
        Either::Left((handle_res, _)) => { handle_res.unwrap()? },
        Either::Right((ctrl_c_res, _)) => { ctrl_c_res? },
    }
    Ok(())
}
