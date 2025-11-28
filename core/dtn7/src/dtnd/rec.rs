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

use crate::core::application_agent::{ApplicationAgent, SimpleApplicationAgent};
use crate::core::helpers::is_valid_service_name;
use crate::core::processing;
use crate::dtnd::rec_messages::*;
use crate::{CONFIG, DTNCORE};

// Multicast/broadcast addresses for REC
const REC_BROADCAST_ADDRESS: &str = "dtn://rec.all/~";
const REC_BROKER_MULTICAST_ADDRESS: &str = "dtn://rec.broker/~";
const REC_DATASTORE_MULTICAST_ADDRESS: &str = "dtn://rec.store/~";
const REC_EXECUTOR_MULTICAST_ADDRESS: &str = "dtn://rec.executor/~";
const REC_CLIENT_MULTICAST_ADDRESS: &str = "dtn://rec.client/~";

struct SockGuard(std::path::PathBuf);
impl Drop for SockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Get the multicast address for a given node type.
///
/// * `node_type` - The `NodeType`.
///
/// Returns `Some(&str)` with the multicast address, or `None` for `NodeType::None`.
fn get_multicast_address(node_type: NodeType) -> Option<&'static str> {
    match node_type {
        NodeType::Broker => Some(REC_BROKER_MULTICAST_ADDRESS),
        NodeType::Executor => Some(REC_EXECUTOR_MULTICAST_ADDRESS),
        NodeType::DataStore => Some(REC_DATASTORE_MULTICAST_ADDRESS),
        NodeType::Client => Some(REC_CLIENT_MULTICAST_ADDRESS),
        NodeType::None => None,
    }
}

pub async fn serve_rec_agent(shutdown: CancellationToken) -> Result<()> {
    let sock_path = CONFIG
        .lock()
        .rec_socket_path
        .clone()
        .expect("REC Agent started without rec_socket_path configured");
    let sock_guard = SockGuard(sock_path);

    // Cleanup stale socket on startup
    if sock_guard.0.exists() {
        let _ = tokio::fs::remove_file(&sock_guard.0).await;
    }
    if let Some(dir) = sock_guard.0.parent() {
        tokio::fs::create_dir_all(dir).await.ok();
    }

    let listener = UnixListener::bind(&sock_guard.0)
        .with_context(|| format!("bind {}", sock_guard.0.display()))?;
    info!("REC Agent listening at {}", sock_guard.0.display());

    // Register broadcast and multicast endpoints
    {
        let mut dtncore = DTNCORE.lock();
        for addr in [
            REC_BROADCAST_ADDRESS,
            REC_BROKER_MULTICAST_ADDRESS,
            REC_DATASTORE_MULTICAST_ADDRESS,
            REC_EXECUTOR_MULTICAST_ADDRESS,
            REC_CLIENT_MULTICAST_ADDRESS,
        ] {
            if let Ok(eid) = EndpointID::try_from(addr) {
                dtncore.register_application_agent(SimpleApplicationAgent::with(eid).into());
            }
        }
    }

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
    let buf = match read_framed(&mut stream).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            // Connection closed by client
            debug!("REC Agent: client disconnected");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
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
            let req: BundleCreate = rmp_serde::from_slice(&buf).context("decoding BundleCreate")?;
            let resp = handle_bundle_create(req).await;
            write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
        }
        _ => {
            // Unknown/unsupported type
            let resp = Reply {
                header: Message {
                    message_type: MessageType::Reply,
                },
                success: false,
                error: format!("unsupported message type: {:?}", msg.message_type),
            };
            write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
        }
    }
    Ok(())
}

async fn handle_register(req: Register) -> Reply {
    let mut resp = Reply {
        header: Message {
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
        header: Message {
            message_type: MessageType::FetchReply,
        },
        success: true,
        error: String::new(),
        bundles: Vec::new(),
    };

    let eid = match resolve_eid(&req.endpoint_id) {
        Ok(e) => e,
        Err(e) => {
            resp.success = false;
            resp.error = format!("invalid endpoint id: {e}");
            return resp;
        }
    };

    debug!(
        "REC Agent: fetch for {} (node_type={:?})",
        eid, req.node_type
    );

    let mut all_bundles: Vec<Bundle> = Vec::new();

    {
        let mut dtncore = DTNCORE.lock();

        // Get unicast bundles for the specific endpoint
        if let Some(aa) = dtncore.get_endpoint_mut(&eid) {
            while let Some(b) = aa.pop() {
                all_bundles.push(b);
            }
        }

        // Get multicast bundles based on node type
        if let Some(multicast_addr) = get_multicast_address(req.node_type)
            && let Ok(multicast_eid) = EndpointID::try_from(multicast_addr)
            && let Some(aa) = dtncore.get_endpoint_mut(&multicast_eid)
        {
            while let Some(b) = aa.pop() {
                all_bundles.push(b);
            }
        }

        // Get broadcast bundles
        if let Ok(broadcast_eid) = EndpointID::try_from(REC_BROADCAST_ADDRESS)
            && let Some(aa) = dtncore.get_endpoint_mut(&broadcast_eid)
        {
            while let Some(b) = aa.pop() {
                all_bundles.push(b);
            }
        }
    }

    resp.bundles = all_bundles.iter().filter_map(transform_bundle).collect();
    debug!(
        "REC Agent: fetch for {} returned {} bundles",
        eid,
        resp.bundles.len()
    );
    resp
}

fn transform_bundle(b: &Bundle) -> Option<BundleData> {
    let payload_data = b.payload()?;

    // Try to decode the payload as msgpack BundleData
    match rmp_serde::from_slice::<BundleData>(payload_data) {
        Ok(bundle_data) => Some(bundle_data),
        Err(e) => {
            // If msgpack decode fails, try to construct from bundle metadata
            debug!("Failed to decode bundle payload as BundleData: {e}");

            let bundle_type = b
                .extension_block_by_type(REC_BUNDLE_TYPE_BLOCK_TYPE)
                .and_then(RecBundleTypeBlock::from_canonical_block)
                .and_then(|rtb| rtb.bundle_type())?;

            Some(BundleData {
                bundle_type,
                source: b.primary.source.to_string(),
                destination: b.primary.destination.to_string(),
                payload: payload_data.to_vec(),
                success: true,
                error: String::new(),
                node_type: NodeType::None,
                submitter: String::new(),
                named_data: String::new(),
            })
        }
    }
}

async fn handle_bundle_create(req: BundleCreate) -> Reply {
    let mut resp = Reply {
        header: Message {
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

    // Use the local node's EID as the bundle source
    // The original sender info is preserved in the BundleData payload
    let local_eid = CONFIG.lock().host_eid.clone();

    let payload = match rmp_serde::to_vec_named(&req.bundle) {
        Ok(p) => p,
        Err(e) => {
            resp.success = false;
            resp.error = format!("failed to serialize bundle data: {e}");
            return resp;
        }
    };

    let type_block = RecBundleTypeBlock::new(req.bundle.bundle_type)
        .to_canonical_block(2, BlockControlFlags::empty());

    let creation_timestamp = CreationTimestamp::now();
    let lifetime = Duration::from_secs(60 * 60); // 1 hour
    let report_to = local_eid.clone();
    let b_flags = BundleControlFlags::BUNDLE_MUST_NOT_FRAGMENTED;
    let blk_flags = BlockControlFlags::from_bits_truncate(0);

    let pblock = PrimaryBlockBuilder::default()
        .bundle_control_flags(b_flags.bits())
        .destination(dst)
        .source(local_eid)
        .report_to(report_to)
        .creation_timestamp(creation_timestamp)
        .lifetime(lifetime)
        .build()
        .unwrap();

    let mut bundle = BundleBuilder::default()
        .primary(pblock)
        .canonicals(vec![
            canonical::new_payload_block(blk_flags, payload),
            canonical::new_hop_count_block(3, BlockControlFlags::empty(), 32),
            type_block,
        ])
        .build()
        .unwrap();

    bundle.set_crc(bp7::crc::CRC_NO);

    debug!("REC Agent: sending bundle {:?}", bundle.id());
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

pub async fn read_framed(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let len = stream.read_u64().await?;
    let len = usize::try_from(len)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

pub async fn write_framed(stream: &mut UnixStream, buf: &[u8]) -> std::io::Result<()> {
    stream.write_u64(buf.len() as u64).await?;
    stream.write_all(buf).await?;
    stream.flush().await?;
    Ok(())
}
