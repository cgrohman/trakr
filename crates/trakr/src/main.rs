//! trakr: launch monitor bridge for golf simulators.
//!
//! This CLI is a thin, AXI-conventioned client over the `trakr` daemon's HTTP
//! API (see `trakr-daemon` and `docs/api.md`). Everything it does, an agent
//! can do directly against that API — the CLI exists for humans and scripts
//! that would rather not speak HTTP.
mod toon;

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;
use tracing_subscriber::EnvFilter;

const DEFAULT_DAEMON: &str = "http://127.0.0.1:7011";

#[derive(Parser)]
#[command(
    name = "trakr",
    version,
    about = "Launch monitor bridge for golf simulators",
    disable_help_subcommand = true
)]
struct Cli {
    /// Daemon base URL.
    #[arg(long, global = true, env = "TRAKR_DAEMON", default_value = DEFAULT_DAEMON)]
    daemon: String,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon in the foreground: owns the launch monitor session and
    /// serves the HTTP + SSE API that this CLI, an agent, or the Tauri UI drive.
    #[command(
        after_help = "Examples:\n  trakr serve                                    # sim on this machine, default port 921\n  trakr serve --sim-host 192.168.1.50 --sim-port 921\n  RUST_LOG=debug trakr serve                     # verbose wire-level logs on stderr"
    )]
    Serve {
        /// Address the HTTP + SSE API listens on.
        #[arg(long, default_value = "127.0.0.1:7011")]
        bind: SocketAddr,
        /// Simulator to forward shots to (Muni Golf Sim / GSPro Open Connect).
        #[arg(long, default_value = "127.0.0.1")]
        sim_host: String,
        /// Simulator's Open Connect port.
        #[arg(long, default_value_t = 921)]
        sim_port: u16,
    },
    /// Discover launch monitors on the network.
    #[command(
        after_help = "Examples:\n  trakr devices\n  trakr devices --broadcast 192.168.7.255        # if the global broadcast doesn't reach your subnet\n  trakr devices --window-ms 6000                 # wait longer on a slow network"
    )]
    Devices {
        /// How long to wait for replies.
        #[arg(long, default_value_t = 3000)]
        window_ms: u64,
        /// Directed broadcast address, e.g. 192.168.7.255 for a /22. Repeatable.
        #[arg(long = "broadcast")]
        broadcast: Vec<String>,
    },
    /// Connect to a launch monitor. With no flags, discovers and connects to
    /// the first one found.
    #[command(
        after_help = "Examples:\n  trakr connect                                        # discover and connect to the first box found\n  trakr connect --name SKYTRAK_C47F51902EE3 --address 192.168.4.61\n  trakr connect --hand left"
    )]
    Connect {
        /// Box name from `trakr devices`. Requires --address.
        #[arg(long, requires = "address")]
        name: Option<String>,
        /// Box IPv4 address from `trakr devices`. Requires --name.
        #[arg(long, requires = "name")]
        address: Option<String>,
        /// Player handedness to configure on connect.
        #[arg(long, value_parser = ["right", "left"])]
        hand: Option<String>,
    },
    /// Show the current session's device and status.
    Status,
    /// Arm the connected launch monitor (activates lasers/cameras).
    Arm,
    /// Disarm the connected launch monitor.
    Disarm,
    /// Set shot mode.
    Mode {
        #[arg(value_parser = ["normal", "putting"])]
        mode: String,
    },
    /// Set player handedness.
    Hand {
        #[arg(value_parser = ["right", "left"])]
        hand: String,
    },
    /// Disconnect the current session.
    Disconnect,
    /// Stream session events (status, shots, errors) as they happen.
    #[command(
        after_help = "Examples:\n  trakr events                                    # human-readable TOON lines\n  trakr events --json | jq .                      # pipe raw JSON to another tool"
    )]
    Events {
        /// Print raw JSON instead of TOON.
        #[arg(long)]
        json: bool,
    },
    /// Send one synthetic shot straight to a simulator, bypassing the daemon.
    /// Useful for proving a simulator link works before hardware is involved.
    #[command(allow_negative_numbers = true)]
    TestShot {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 921)]
        port: u16,
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
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let exit_code = match cli.cmd {
        None => home(&client, &cli.daemon).await?,
        Some(Cmd::Serve {
            bind,
            sim_host,
            sim_port,
        }) => {
            let sim = trakr_openconnect::Config {
                host: sim_host,
                port: sim_port,
                ..Default::default()
            };
            trakr_daemon::serve(bind, sim).await?;
            0
        }
        Some(Cmd::Devices {
            window_ms,
            broadcast,
        }) => devices(&client, &cli.daemon, window_ms, broadcast).await?,
        Some(Cmd::Connect {
            name,
            address,
            hand,
        }) => connect(&client, &cli.daemon, name, address, hand).await?,
        Some(Cmd::Status) => status(&client, &cli.daemon).await?,
        Some(Cmd::Arm) => simple_post(&client, &cli.daemon, "/v1/session/arm", None).await?,
        Some(Cmd::Disarm) => simple_post(&client, &cli.daemon, "/v1/session/disarm", None).await?,
        Some(Cmd::Mode { mode }) => {
            simple_post(
                &client,
                &cli.daemon,
                "/v1/session/mode",
                Some(serde_json::json!({ "mode": mode })),
            )
            .await?
        }
        Some(Cmd::Hand { hand }) => {
            simple_post(
                &client,
                &cli.daemon,
                "/v1/session/hand",
                Some(serde_json::json!({ "hand": hand })),
            )
            .await?
        }
        Some(Cmd::Disconnect) => disconnect(&client, &cli.daemon).await?,
        Some(Cmd::Events { json }) => events(&client, &cli.daemon, json).await?,
        Some(Cmd::TestShot {
            host,
            port,
            speed,
            vla,
            hla,
            spin,
            axis,
        }) => {
            test_shot(host, port, speed, vla, hla, spin, axis).await?;
            0
        }
    };
    std::process::exit(exit_code);
}

fn bin_path() -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .map(|p| {
            if let Ok(home) = std::env::var("HOME") {
                p.replacen(&home, "~", 1)
            } else {
                p
            }
        })
        .unwrap_or_else(|| "trakr".into())
}

async fn home(client: &reqwest::Client, daemon: &str) -> Result<i32> {
    println!("bin: {}", bin_path());
    println!("description: Launch monitor bridge for golf simulators");
    match client.get(format!("{daemon}/v1/session")).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: Value = resp.json().await?;
            print_session(&body);
            print!(
                "{}",
                toon::help(&[
                    "Run `trakr events` to watch shots as they happen".into(),
                    "Run `trakr disconnect` to end the session".into()
                ])
            );
        }
        Ok(resp) if resp.status() == 404 => {
            println!("session: none");
            print!(
                "{}",
                toon::help(&[
                    "Run `trakr devices` to find a launch monitor".into(),
                    "Run `trakr connect --name <name> --address <ip>` to connect".into(),
                ])
            );
        }
        _ => {
            println!("daemon: not reachable at {daemon}");
            print!(
                "{}",
                toon::help(&["Run `trakr serve` to start the daemon".into()])
            );
        }
    }
    Ok(0)
}

async fn devices(
    client: &reqwest::Client,
    daemon: &str,
    window_ms: u64,
    broadcast: Vec<String>,
) -> Result<i32> {
    let mut url = reqwest::Url::parse(&format!("{daemon}/v1/devices"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("window_ms", &window_ms.to_string());
        if !broadcast.is_empty() {
            q.append_pair("broadcast", &broadcast.join(","));
        }
    }
    let resp = client
        .get(url)
        .send()
        .await
        .context("could not reach the trakr daemon")?;
    if !resp.status().is_success() {
        return print_error_response(resp).await;
    }
    let body: Value = resp.json().await?;
    let devices = body["devices"].as_array().cloned().unwrap_or_default();
    if devices.is_empty() {
        println!("devices: 0 found in {window_ms}ms");
        print!(
            "{}",
            toon::help(&["Make sure the unit is powered on and connected to this network".into()])
        );
        return Ok(0);
    }
    let rows: Vec<Vec<String>> = devices
        .iter()
        .map(|d| {
            vec![
                d["name"].as_str().unwrap_or("").into(),
                d["address"].as_str().unwrap_or("").into(),
                d["direct_mode"].as_bool().unwrap_or(false).to_string(),
            ]
        })
        .collect();
    print!(
        "{}",
        toon::table("devices", &["name", "address", "direct_mode"], &rows)
    );
    print!(
        "{}",
        toon::help(&["Run `trakr connect --name <name> --address <address>` to connect".into()])
    );
    Ok(0)
}

async fn connect(
    client: &reqwest::Client,
    daemon: &str,
    name: Option<String>,
    address: Option<String>,
    hand: Option<String>,
) -> Result<i32> {
    let mut body = serde_json::Map::new();
    if let Some(n) = name {
        body.insert("name".into(), Value::String(n));
    }
    if let Some(a) = address {
        body.insert("address".into(), Value::String(a));
    }
    if let Some(h) = hand {
        body.insert("right_handed".into(), Value::Bool(h == "right"));
    }
    let resp = client
        .post(format!("{daemon}/v1/session"))
        .json(&Value::Object(body))
        .send()
        .await
        .context("could not reach the trakr daemon")?;
    if !resp.status().is_success() {
        return print_error_response(resp).await;
    }
    let already: Value = resp.json().await?;
    if already["already_connected"].as_bool() == Some(true) {
        println!("session: already connected to this device (no-op)");
    } else {
        println!("session: connecting");
        print!(
            "{}",
            toon::help(&[
                "Run `trakr status` to check progress".into(),
                "Run `trakr events` to stream progress live".into()
            ])
        );
    }
    Ok(0)
}

async fn status(client: &reqwest::Client, daemon: &str) -> Result<i32> {
    let resp = client
        .get(format!("{daemon}/v1/session"))
        .send()
        .await
        .context("could not reach the trakr daemon")?;
    if !resp.status().is_success() {
        return print_error_response(resp).await;
    }
    let body: Value = resp.json().await?;
    print_session(&body);
    Ok(0)
}

fn print_session(body: &Value) {
    let device = &body["device"];
    let status = &body["status"];
    print!(
        "{}",
        toon::record(
            "session",
            &[
                ("device", device["name"].as_str().unwrap_or("-").to_string()),
                (
                    "address",
                    device["address"].as_str().unwrap_or("-").to_string()
                ),
                (
                    "firmware",
                    device["firmware"].as_str().unwrap_or("-").to_string()
                ),
                (
                    "armed",
                    status["armed"].as_bool().unwrap_or(false).to_string()
                ),
                (
                    "battery_pct",
                    status["battery_pct"]
                        .as_f64()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into())
                ),
                (
                    "mode",
                    status["shot_mode"].as_str().unwrap_or("-").to_string()
                ),
                (
                    "hand",
                    status["handedness"].as_str().unwrap_or("-").to_string()
                ),
            ],
        )
    );
    if body["last_shot"].is_object() {
        print!(
            "{}",
            toon::help(&["Run `trakr events --json` to see full shot data as it arrives".into()])
        );
    }
}

async fn disconnect(client: &reqwest::Client, daemon: &str) -> Result<i32> {
    let resp = client
        .delete(format!("{daemon}/v1/session"))
        .send()
        .await
        .context("could not reach the trakr daemon")?;
    let body: Value = resp.json().await?;
    if body["note"].as_str().is_some() {
        println!("session: already disconnected (no-op)");
    } else {
        println!("session: disconnected");
    }
    Ok(0)
}

async fn simple_post(
    client: &reqwest::Client,
    daemon: &str,
    path: &str,
    body: Option<Value>,
) -> Result<i32> {
    let mut req = client.post(format!("{daemon}{path}"));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req
        .send()
        .await
        .context("could not reach the trakr daemon")?;
    if !resp.status().is_success() {
        return print_error_response(resp).await;
    }
    println!("ok: command accepted");
    print!(
        "{}",
        toon::help(&["Run `trakr status` to see the resulting state".into()])
    );
    Ok(0)
}

async fn print_error_response(resp: reqwest::Response) -> Result<i32> {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or_default();
    let error = body["error"].as_str().unwrap_or("request_failed");
    let help = body["help"]
        .as_str()
        .unwrap_or("check `trakr status` and the daemon logs");
    print!("{}", toon::error(&format!("{error} ({status})"), help));
    Ok(1)
}

async fn events(client: &reqwest::Client, daemon: &str, raw_json: bool) -> Result<i32> {
    use futures_util::StreamExt;
    let resp = client
        .get(format!("{daemon}/v1/events"))
        .send()
        .await
        .context("could not reach the trakr daemon")?;
    if !resp.status().is_success() {
        return print_error_response(resp).await;
    }
    eprintln!("streaming events, Ctrl-C to stop...");
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk?));
        while let Some(pos) = buf.find("\n\n") {
            let block: String = buf.drain(..pos + 2).collect();
            let mut event_type = "message".to_string();
            let mut data = String::new();
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event_type = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push_str(v.trim());
                }
            }
            if data.is_empty() || event_type == "keep-alive" {
                continue;
            }
            if raw_json {
                println!("{data}");
            } else if let Ok(v) = serde_json::from_str::<Value>(&data) {
                print_event_toon(&event_type, &v);
            }
        }
    }
    Ok(0)
}

fn print_event_toon(kind: &str, v: &Value) {
    match kind {
        "status" => print!(
            "{}",
            toon::record(
                "event.status",
                &[
                    (
                        "battery_pct",
                        v["battery_pct"]
                            .as_f64()
                            .map(|x| x.to_string())
                            .unwrap_or_else(|| "-".into())
                    ),
                    ("armed", v["armed"].as_bool().unwrap_or(false).to_string()),
                    (
                        "rssi",
                        v["rssi"]
                            .as_i64()
                            .map(|x| x.to_string())
                            .unwrap_or_else(|| "-".into())
                    ),
                ],
            )
        ),
        "shot" => println!(
            "event.shot: {}",
            serde_json::to_string(v).unwrap_or_default()
        ),
        "connected" => println!("event.connected: {}", v["name"].as_str().unwrap_or("-")),
        "disconnected" => println!(
            "event.disconnected: {}",
            v["reason"].as_str().unwrap_or("-")
        ),
        "error" => println!("event.error: {v}"),
        "misread" => println!("event.misread: {}", v["reason"].as_str().unwrap_or("-")),
        other => println!("event.{other}: {v}"),
    }
}

async fn test_shot(
    host: String,
    port: u16,
    speed: f32,
    vla: f32,
    hla: f32,
    spin: f32,
    axis: f32,
) -> Result<()> {
    use tokio::sync::{broadcast, mpsc};
    use trakr_core::{BallData, Confidence, Event, Shot, EVENT_BUS_CAPACITY};

    let cfg = trakr_openconnect::Config {
        host,
        port,
        heartbeat: Duration::from_millis(500),
        ..Default::default()
    };
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
    println!("sending test shot: {speed}mph vla={vla} hla={hla} spin={spin}rpm axis={axis}");
    tx.send(Event::Shot(shot))?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), out).await;
    log_cmds.abort();
    Ok(())
}
