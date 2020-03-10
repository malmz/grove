
mod async_spsc;
// mod cpal_stuff;
mod audio;
mod encoder;
mod decoder;

use anyhow::bail;
use anyhow::Result;

use futures::future;
use futures::select;
use futures::future::Either;
use futures::pin_mut;
use futures::FutureExt;

use tokio::task;
use tokio::net;

#[tokio::main]
async fn main() -> Result<()> {
    run_networked().await
}

async fn run_file() -> Result<()> {
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

    match future::select(handle, ctrl_c).await {
        Either::Left((handle_res, _)) => { handle_res.unwrap()? },
        Either::Right((ctrl_c_res, _)) => { ctrl_c_res? },
    }
    Ok(())
}

#[allow(clippy::unnecessary_mut_passed)]
async fn run_networked() -> Result<()> {
    let mut args = pico_args::Arguments::from_env();
    let addr: String = args.opt_value_from_str("--addr")?.unwrap_or_else(|| "0.0.0.0:1337".into());
    
    
    let socket = net::UdpSocket::bind("127.0.0.1").await?;
    socket.connect(addr).await?;
    let (recv, send) = socket.split();
    let (speaker, mic) = audio::init()?;
    
    let mut ehandle = task::spawn(encoder::run_network(mic, send)).fuse();
    let mut dhandle = task::spawn(decoder::run_network(speaker, recv)).fuse();
    let stop = tokio::signal::ctrl_c().fuse();

    pin_mut!(stop);

    select! {
        res = ehandle => res.unwrap()?,
        res = dhandle => res.unwrap()?,
        res = stop => res?,
    }
    Ok(())
}
