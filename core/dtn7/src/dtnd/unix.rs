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

use crate::core::application_agent::ApplicationAgent;
use crate::core::application_agent::SimpleApplicationAgent;
use crate::core::helpers::is_valid_service_name;
use crate::core::processing;
use crate::dtnd::unix_messages::*;
use crate::{CONFIG, DTNCORE};

struct SockGuard(std::path::PathBuf);
impl Drop for SockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub async fn serve_unix_agent(shutdown: CancellationToken) -> Result<()> {
    let sock_guard = SockGuard(CONFIG.lock().unix_socket_path.to_path_buf());

    // cleanup stale socket on startup
    if sock_guard.0.exists() {
        let _ = tokio::fs::remove_file(&sock_guard.0).await;
    }
    if let Some(dir) = sock_guard.0.parent() {
        tokio::fs::create_dir_all(dir).await.ok();
    }

    let listener = UnixListener::bind(&sock_guard.0)
        .with_context(|| format!("bind {}", sock_guard.0.display()))?;
    info!(
        "Unix Domain Socket Agent listening at {}",
        sock_guard.0.display()
    );

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("Unix Domain Socket Agent shutdown");
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
            MessageType::RegisterEID | MessageType::UnregisterEID => {
                let req: RegisterUnregisterMessage =
                    rmp_serde::from_slice(&buf).context("decoding Register/Unregister")?;
                let resp = handle_register(req, msg.message_type == MessageType::RegisterEID).await;
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
            MessageType::BundleCreate => {
                let req: BundleCreateMessage =
                    rmp_serde::from_slice(&buf).context("decoding BundleCreate")?;
                let resp = handle_bundle_create(req).await;
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
            MessageType::List => {
                let req: MailboxListMessage =
                    rmp_serde::from_slice(&buf).context("decoding List")?;
                let resp = handle_list(req).await;
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
            MessageType::GetBundle => {
                let req: GetBundleMessage =
                    rmp_serde::from_slice(&buf).context("decoding GetBundle")?;
                let resp = handle_get(req).await;
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
            MessageType::GetAllBundles => {
                let req: GetAllBundlesMessage =
                    rmp_serde::from_slice(&buf).context("decoding GetAllBundles")?;
                let resp = handle_get_all(req).await;
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
            _ => {
                // Unknown/unsupported type
                let resp = GeneralResponse {
                    message: Message {
                        message_type: MessageType::GeneralResponse,
                    },
                    success: false,
                    error: format!("unsupported message type: {:?}", msg.message_type),
                };
                write_framed(&mut stream, &rmp_serde::to_vec_named(&resp)?).await?;
            }
        }
    }
}

async fn handle_register(req: RegisterUnregisterMessage, register: bool) -> GeneralResponse {
    let mut resp = GeneralResponse {
        message: Message {
            message_type: MessageType::GeneralResponse,
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

    if register {
        (*DTNCORE.lock())
            .register_application_agent(SimpleApplicationAgent::with(eid.clone()).into());
        debug!("Unix Domain Socket Agent: registered {eid}");
    } else {
        (*DTNCORE.lock())
            .unregister_application_agent(SimpleApplicationAgent::with(eid.clone()).into());
        debug!("Unix Domain Socket Agent: unregistered {eid}");
    }
    resp
}

async fn handle_bundle_create(req: BundleCreateMessage) -> GeneralResponse {
    let mut resp = GeneralResponse {
        message: Message {
            message_type: MessageType::GeneralResponse,
        },
        success: true,
        error: String::new(),
    };

    let dst = match resolve_eid(&req.destination_id) {
        Ok(d) => d,
        Err(e) => {
            resp.success = false;
            resp.error = format!("invalid destination: {e}");
            return resp;
        }
    };

    let src = match req.source_id.as_deref() {
        Some(s) => match resolve_eid(s) {
            Ok(e) => e,
            Err(e) => {
                resp.success = false;
                resp.error = format!("invalid source: {e}");
                return resp;
            }
        },
        None => CONFIG.lock().host_eid.clone(),
    };

    // TODO: parse req.creation_timestamp
    let creation_timestamp = CreationTimestamp::now();
    let lifetime = req
        .lifetime
        .as_deref()
        .and_then(|s| humantime::parse_duration(s).ok())
        .unwrap_or_else(|| Duration::from_secs(3600));

    let report_to = match req.report_to.as_deref() {
        Some(s) => match resolve_eid(s) {
            Ok(e) => e,
            Err(e) => {
                resp.success = false;
                resp.error = format!("invalid report_to: {e}");
                return resp;
            }
        },
        None => src.clone(),
    };

    let b_flags = if let Some(bits) = req.bundle_flags {
        BundleControlFlags::from_bits_truncate(bits)
    } else {
        BundleControlFlags::BUNDLE_MUST_NOT_FRAGMENTED
    };

    let blk_flags =
        BlockControlFlags::from_bits_truncate(req.block_flags.unwrap_or(0).try_into().unwrap_or(0));

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
            canonical::new_payload_block(blk_flags, req.payload),
            canonical::new_hop_count_block(2, BlockControlFlags::empty(), 32),
        ])
        .build()
        .unwrap();

    bundle.set_crc(bp7::crc::CRC_NO);

    processing::send_bundle(bundle).await;
    resp
}

async fn handle_list(req: MailboxListMessage) -> MailboxListResponse {
    let mut resp = MailboxListResponse {
        general_response: GeneralResponse {
            message: Message {
                message_type: MessageType::ListResponse,
            },
            success: true,
            error: String::new(),
        },
        bundles: Vec::new(),
    };

    let eid = match resolve_eid(&req.mailbox) {
        Ok(e) => e,
        Err(e) => {
            resp.general_response.success = false;
            resp.general_response.error = format!("invalid endpoint id: {e}");
            return resp;
        }
    };

    if let Some(aa) = (*DTNCORE.lock()).get_endpoint_mut(&eid) {
        let mut drained: Vec<Bundle> = Vec::new();
        while let Some(b) = aa.pop() {
            drained.push(b);
        }
        resp.bundles = drained.iter().map(|b| b.id().to_string()).collect();

        // Preserve original order even if pop() is LIFO
        for b in drained.into_iter().rev() {
            aa.push(&b);
        }
    } else {
        resp.general_response.success = false;
        resp.general_response.error = "unknown endpoint".to_string();
    }
    resp
}

async fn handle_get(req: GetBundleMessage) -> GetBundleResponse {
    let mut resp = GetBundleResponse {
        general_response: GeneralResponse {
            message: Message {
                message_type: MessageType::GetBundleResponse,
            },
            success: true,
            error: String::new(),
        },
        content: BundleContent::default(),
    };

    let eid = match resolve_eid(&req.mailbox) {
        Ok(e) => e,
        Err(_) => {
            resp.general_response.success = false;
            resp.general_response.error = "invalid endpoint id".to_string();
            return resp;
        }
    };

    if let Some(aa) = (*DTNCORE.lock()).get_endpoint_mut(&eid) {
        let mut found: Option<Bundle> = None;
        let mut drained: Vec<Bundle> = Vec::new();

        while let Some(b) = aa.pop() {
            if found.is_none() && b.id() == req.bundle_id {
                found = Some(b);
            } else {
                drained.push(b);
            }
        }

        // Preserve original order even if pop() is LIFO
        for b in drained.into_iter().rev() {
            aa.push(&b);
        }

        if let Some(bndl) = found {
            if !req.remove {
                aa.push(&bndl);
            }
            resp.content = bundle_to_content(&bndl);
        } else {
            resp.general_response.success = false;
            resp.general_response.error = "bundle not found".to_string();
        }
    } else {
        resp.general_response.success = false;
        resp.general_response.error = "unknown endpoint".to_string();
    }
    resp
}

async fn handle_get_all(req: GetAllBundlesMessage) -> GetAllBundlesResponse {
    let mut resp = GetAllBundlesResponse {
        general_response: GeneralResponse {
            message: Message {
                message_type: MessageType::GetAllBundlesResponse,
            },
            success: true,
            error: String::new(),
        },
        bundles: Vec::new(),
    };

    let eid = match resolve_eid(&req.mailbox) {
        Ok(e) => e,
        Err(_) => {
            resp.general_response.success = false;
            resp.general_response.error = "invalid endpoint id".to_string();
            return resp;
        }
    };

    if let Some(aa) = (*DTNCORE.lock()).get_endpoint_mut(&eid) {
        let mut drained: Vec<Bundle> = Vec::new();
        while let Some(b) = aa.pop() {
            drained.push(b);
        }

        resp.bundles = drained.iter().map(bundle_to_content).collect();

        if !req.remove {
            // Preserve original order even if pop() is LIFO
            for b in drained.into_iter().rev() {
                aa.push(&b);
            }
        }
    } else {
        resp.general_response.success = false;
        resp.general_response.error = "unknown endpoint".to_string();
    }
    resp
}

fn bundle_to_content(b: &Bundle) -> BundleContent {
    let payload = b.payload().map(|p| p.to_vec()).unwrap_or_default();
    let src = format!("{}", b.primary.source);
    let dst = format!("{}", b.primary.destination);
    BundleContent {
        bundle_id: b.id().to_string(),
        source_id: src,
        destination_id: dst,
        payload,
    }
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
