use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use dtn7::dtnd::{
    unix::{read_framed, write_framed},
    unix_messages::*,
};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use tokio::net::UnixStream;

#[derive(Parser, Debug)]
#[command(name = "dtnclient", author, version, about = "Unix Domain Socket Agent client for dtn7-rs dtnd", long_about = None)]
struct Cli {
    #[arg(
        short = 's',
        long = "socket",
        value_name = "PATH",
        default_value = "/tmp/dtnd.socket"
    )]
    socket: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Register a local endpoint with the daemon
    Register { endpoint: String },
    /// Unregister a local endpoint
    Unregister { endpoint: String },
    /// Send a UTF-8 message to a destination endpoint
    Send {
        recv_endpoint: String,
        message: String,
    },
    /// Receive & drain all pending bundles for an endpoint
    Receive { endpoint: String },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Register { endpoint } => do_register(&cli.socket, &endpoint).await,
        Commands::Unregister { endpoint } => do_unregister(&cli.socket, &endpoint).await,
        Commands::Send {
            recv_endpoint,
            message,
        } => do_send(&cli.socket, &recv_endpoint, message.into_bytes()).await,
        Commands::Receive { endpoint } => do_receive(&cli.socket, &endpoint).await,
    }
}

async fn do_register(sock: &Path, endpoint: &str) -> Result<()> {
    let req = RegisterUnregisterMessage {
        message: Message {
            message_type: MessageType::RegisterEID,
        },
        endpoint_id: endpoint.to_string(),
    };
    let resp: GeneralResponse = roundtrip(sock, &req).await?;
    if resp.success {
        println!("ok");
        Ok(())
    } else {
        anyhow::bail!(resp.error)
    }
}

async fn do_unregister(sock: &Path, endpoint: &str) -> Result<()> {
    let req = RegisterUnregisterMessage {
        message: Message {
            message_type: MessageType::UnregisterEID,
        },
        endpoint_id: endpoint.to_string(),
    };
    let resp: GeneralResponse = roundtrip(sock, &req).await?;
    if resp.success {
        println!("ok");
        Ok(())
    } else {
        anyhow::bail!(resp.error)
    }
}

async fn do_send(sock: &Path, dst: &str, payload: Vec<u8>) -> Result<()> {
    let req = BundleCreateMessage {
        message: Message {
            message_type: MessageType::BundleCreate,
        },
        destination_id: dst.to_string(),
        payload,
        source_id: None,
        creation_timestamp: None,
        lifetime: None,
        report_to: None,
        bundle_flags: None,
        block_flags: None,
    };
    let resp: GeneralResponse = roundtrip(sock, &req).await?;
    if resp.success {
        println!("ok");
        Ok(())
    } else {
        anyhow::bail!(resp.error)
    }
}

async fn do_receive(sock: &Path, endpoint: &str) -> Result<()> {
    let req = GetAllBundlesMessage {
        message: Message {
            message_type: MessageType::GetAllBundles,
        },
        mailbox: endpoint.to_string(),
        new: true,
        remove: true, // drain queue by default
    };
    let resp: GetAllBundlesResponse = roundtrip(sock, &req).await?;
    if !resp.general_response.success {
        anyhow::bail!(resp.general_response.error);
    }
    if resp.bundles.is_empty() {
        println!("(no messages)");
        return Ok(());
    }

    for b in resp.bundles {
        // Try UTF-8 first, then show hex preview if binary
        match std::str::from_utf8(&b.payload) {
            Ok(s) => println!("{} <- {}: {}", b.destination_id, b.source_id, s),
            Err(_) => {
                let mut hex = String::new();
                for (i, byte) in b.payload.iter().take(64).enumerate() {
                    let _ = write!(hex, "{:02x}", byte);
                    if i % 2 == 1 {
                        hex.push(' ');
                    }
                }
                if b.payload.len() > 64 {
                    hex.push_str("...");
                }
                println!(
                    "{} <- {}: <{} bytes, hex: {}>",
                    b.destination_id,
                    b.source_id,
                    b.payload.len(),
                    hex
                );
            }
        }
    }
    Ok(())
}

async fn roundtrip<TReq: Serialize, TResp: for<'de> Deserialize<'de>>(
    sock: &Path,
    req: &TReq,
) -> Result<TResp> {
    let mut stream = UnixStream::connect(sock)
        .await
        .with_context(|| format!("connect {}", sock.display()))?;
    let buf = rmp_serde::to_vec_named(req).context("encode msgpack")?;
    write_framed(&mut stream, &buf)
        .await
        .context("write framed request")?;
    let resp_buf = read_framed(&mut stream)
        .await
        .context("read framed response")?;

    let resp: TResp = rmp_serde::from_slice(&resp_buf).context("decode msgpack response")?;
    Ok(resp)
}
