use anyhow::{Context, Result};
use bp7::{
    Bundle, CreationTimestamp, EndpointID,
    bundle::BundleBuilder,
    canonical,
    eid::EndpointIdError,
    flags::{BlockControlFlags, BundleControlFlags},
    primary::PrimaryBlockBuilder,
};
use log::{debug, info, warn};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};
use tokio_util::sync::CancellationToken;

use crate::core::application_agent::SimpleApplicationAgent;
use crate::core::helpers::is_valid_service_name;
use crate::core::processing;
use crate::dtnd::rec_messages::*;
use crate::{CONFIG, DTNCORE};
use crate::{core::application_agent::ApplicationAgent, dtnd::rec_messages};

struct SockGuard(std::path::PathBuf);
impl Drop for SockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub async fn serve_rec_agent(shutdown: CancellationToken) -> Result<()> {
    let sock_guard = SockGuard(CONFIG.lock().rec_socket_path.to_path_buf());

    // cleanup stale socket on startup
    if sock_guard.0.exists() {
        let _ = tokio::fs::remove_file(&sock_guard.0).await;
    }
    if let Some(dir) = sock_guard.0.parent() {
        tokio::fs::create_dir_all(dir).await.ok();
    }

    let listener = UnixListener::bind(&sock_guard.0)
        .with_context(|| format!("bind {}", sock_guard.0.display()))?;
    info!("REC Agent listening at {}", sock_guard.0.display());

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("REC Agent shutdown");
                break;
            }
            accept_res = listener.accept() => {
                let (stream, _addr) = match accept_res {
                    Ok(x) => x,
                    Err(e) => { warn!("unix accept error: {e}"); continue; }
                };
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream).await {
                        warn!("unix conn error: {e:?}");
                    }
                });
            }
        }
    }
    Ok(())
}

async fn handle_connection(mut stream: UnixStream) -> Result<()> {
    loop {
        let buf = read_framed(&mut stream).await?;
        let msg: Message = rmp_serde::from_slice(&buf).context("decoding message header")?;

        match msg.message_type {
            MessageType::Register => {
                let req: Register = rmp_serde::from_slice(&buf).context("decoding Register")?;
                let resp = handle_register(req).await;
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
            MessageType::Fetch => {
                let req: Fetch = rmp_serde::from_slice(&buf).context("decoding Fetch")?;
                let resp = handle_fetch(req).await;
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
            MessageType::BundleCreate => {
                let req: BundleCreate =
                    rmp_serde::from_slice(&buf).context("decoding BundleCreate")?;
                let resp = handle_bundle_create(req).await;
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
            _ => {
                // Unknown/unsupported type
                let resp = Reply {
                    message: Message {
                        message_type: MessageType::Reply,
                    },
                    success: false,
                    error: format!("unsupported message type: {:?}", msg.message_type),
                };
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
        }
    }
}

async fn handle_register(req: Register) -> Reply {
    let mut resp = Reply {
        message: Message {
            message_type: MessageType::Reply,
        },
        success: true,
        error: String::new(),
    };

    let eid = match resolve_eid(&req.endpoint_id) {
        Ok(e) => e,
        Err(e) => {
            resp.success = false;
            resp.error = format!("invalid endpoint id: {e}");
            return resp;
        }
    };

    (*DTNCORE.lock()).register_application_agent(SimpleApplicationAgent::with(eid.clone()).into());
    debug!("REC Agent: registered {eid}");
    resp
}

async fn handle_fetch(req: Fetch) -> FetchReply {
    let mut resp = FetchReply {
        reply: Reply {
            message: Message {
                message_type: MessageType::FetchReply,
            },
            success: true,
            error: String::new(),
        },
        bundles: Vec::new(),
    };

    let eid = match resolve_eid(&req.endpoint_id) {
        Ok(e) => e,
        Err(_) => {
            resp.reply.success = false;
            resp.reply.error = "invalid endpoint id".to_string();
            return resp;
        }
    };

    if let Some(aa) = (*DTNCORE.lock()).get_endpoint_mut(&eid) {
        let mut drained: Vec<Bundle> = Vec::new();
        while let Some(b) = aa.pop() {
            drained.push(b);
        }

        resp.bundles = drained.iter().filter_map(transform_bundle).collect();
    } else {
        resp.reply.success = false;
        resp.reply.error = "unknown endpoint".to_string();
    }
    resp
}

fn transform_bundle(b: &Bundle) -> Option<BundleData> {
    let payload = b.payload().map(|p| p.to_vec()).unwrap_or_default();
    let source = format!("{}", b.primary.source);
    let destination = format!("{}", b.primary.destination);

    let submitter = b
        .extension_block_by_type(rec_messages::REC_JOB_QUERY_BLOCK)
        .map(|pb| RecJobQuery::from_canonical_block(pb).map(|rjq| rjq.submitter))
        .unwrap_or_default();

    if let Some(submitter) = submitter {
        return Some(BundleData {
            bundle_type: BundleType::JobsQuery,
            source,
            destination,
            payload,
            submitter: Some(submitter),
        });
    }
    None
}

async fn handle_bundle_create(req: BundleCreate) -> Reply {
    let mut resp = Reply {
        message: Message {
            message_type: MessageType::Reply,
        },
        success: true,
        error: String::new(),
    };

    let dst = match resolve_eid(&req.bundle.destination) {
        Ok(d) => d,
        Err(e) => {
            resp.success = false;
            resp.error = format!("invalid destination: {e}");
            return resp;
        }
    };

    let src = match resolve_eid(&req.bundle.source) {
        Ok(e) => e,
        Err(e) => {
            resp.success = false;
            resp.error = format!("invalid source: {e}");
            return resp;
        }
    };

    // construct extension block
    let job_query = if let Some(submitter) = &req.bundle.submitter {
        rec_messages::RecJobQuery {
            submitter: submitter.clone(),
        }
        .to_canonical_block(3, BlockControlFlags::empty())
    } else {
        resp.success = false;
        resp.error = "missing submitter".to_string();
        return resp;
    };

    let creation_timestamp = CreationTimestamp::now();
    let lifetime = Duration::from_secs(60 * 60); // 1 hour
    let report_to = src.clone();
    let b_flags = BundleControlFlags::BUNDLE_MUST_NOT_FRAGMENTED;
    let blk_flags = BlockControlFlags::from_bits_truncate(0);

    let pblock = PrimaryBlockBuilder::default()
        .bundle_control_flags(b_flags.bits())
        .destination(dst)
        .source(src)
        .report_to(report_to)
        .creation_timestamp(creation_timestamp)
        .lifetime(lifetime)
        .build()
        .unwrap();

    let mut bundle = BundleBuilder::default()
        .primary(pblock)
        .canonicals(vec![
            canonical::new_payload_block(blk_flags, req.bundle.payload),
            canonical::new_hop_count_block(2, BlockControlFlags::empty(), 32),
            job_query,
        ])
        .build()
        .unwrap();

    bundle.set_crc(bp7::crc::CRC_NO);

    processing::send_bundle(bundle).await;
    resp
}

fn resolve_eid(path: &str) -> std::result::Result<EndpointID, EndpointIdError> {
    let s = path.trim();
    if is_valid_service_name(s) {
        let host_eid = CONFIG.lock().host_eid.clone();
        host_eid.new_endpoint(s)
    } else {
        EndpointID::try_from(s)
    }
}

pub async fn read_framed(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let len = usize::try_from(stream.read_u64().await.context("reading length")?)?;
    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .await
        .context("reading payload")?;
    Ok(buf)
}

pub async fn write_framed(stream: &mut UnixStream, buf: &[u8]) -> Result<()> {
    stream
        .write_u64(buf.len() as u64)
        .await
        .context("writing length")?;
    stream.write_all(buf).await.context("writing payload")?;
    stream.flush().await.context("flush")
}
