//! trakr: launch monitor bridge. Drivers in, GSPro Open Connect out.
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::sync::{broadcast, mpsc};
use tracing_subscriber::EnvFilter;
use trakr_core::{BallData, Confidence, Event, Shot, EVENT_BUS_CAPACITY};

#[derive(Parser)]
#[command(name = "trakr", version, about = "Launch monitor bridge for golf simulators")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Send one synthetic shot to a simulator over Open Connect to prove the
    /// link works end to end (Muni, GSPro, OpenGolfAPI).
    TestShot {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 921)]
        port: u16,
        /// Ball speed in mph.
        #[arg(long, default_value_t = 150.0)]
        speed: f32,
        #[arg(long, default_value_t = 12.5)]
        vla: f32,
        #[arg(long, default_value_t = 1.0)]
        hla: f32,
        #[arg(long, default_value_t = 2800.0)]
        spin: f32,
        #[arg(long, default_value_t = -3.0)]
        axis: f32,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    match Cli::parse().cmd {
        Cmd::TestShot { host, port, speed, vla, hla, spin, axis } => {
            let cfg = trakr_openconnect::Config { host, port, heartbeat: Duration::from_millis(500), ..Default::default() };
            let (tx, rx) = broadcast::channel::<Event>(EVENT_BUS_CAPACITY);
            let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
            let out = tokio::spawn(trakr_openconnect::run(cfg, rx, cmd_tx));
            let log_cmds = tokio::spawn(async move {
                while let Some(c) = cmd_rx.recv().await {
                    tracing::info!(?c, "sim asked for");
                }
            });
            tokio::time::sleep(Duration::from_millis(1500)).await;
            tx.send(Event::Ready)?;
            tokio::time::sleep(Duration::from_millis(500)).await;
            let shot = Shot {
                sequence: 1,
                ball: BallData {
                    speed_mps: speed / 2.236_936,
                    speed_conf: Confidence::CERTAIN,
                    launch_angle_deg: vla,
                    launch_angle_conf: Confidence::CERTAIN,
                    horizontal_angle_deg: hla,
                    horizontal_angle_conf: Confidence::CERTAIN,
                    total_spin_rpm: Some(spin),
                    spin_axis_deg: Some(axis),
                    back_spin_rpm: None,
                    side_spin_rpm: None,
                    spin_conf: Confidence::CERTAIN,
                },
                club: None,
                flight: None,
                valid: true,
            };
            tracing::info!("sending test shot");
            tx.send(Event::Shot(shot))?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            drop(tx);
            let _ = tokio::time::timeout(Duration::from_secs(2), out).await;
            log_cmds.abort();
            Ok(())
        }
    }
}
