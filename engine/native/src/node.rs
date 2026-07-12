use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use iroh::endpoint::{
    presets, AfterHandshakeOutcome, Connection, Endpoint, EndpointHooks, RelayMode, Side,
};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{address_lookup::pkarr::PkarrPublisher, EndpointAddr, EndpointId, TransportAddr};
use protocol::{
    apply_options, export_connection_keying_material, read_message, sign_challenge,
    verify_challenge, write_message, AddrInfoOptions, AppHandle, ControlMessage, InviteResponse,
    PairedDevice, RememberVote, CONTROL_ALPN,
};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::device_identity::{load_or_create_identity, DeviceIdentity, DeviceInfo, PairedDeviceStore};

#[derive(Debug)]
struct AccessState {
    allowed: HashSet<EndpointId>,
    pairing_host_open: bool,
}

#[derive(Debug)]
struct PairedOnlyHook {
    access: Arc<RwLock<AccessState>>,
}

impl EndpointHooks for PairedOnlyHook {
    async fn after_handshake(&self, conn: &Connection) -> AfterHandshakeOutcome {
        if conn.side() != Side::Server {
            return AfterHandshakeOutcome::accept();
        }
        if conn.alpn() != CONTROL_ALPN {
            return AfterHandshakeOutcome::accept();
        }
        let remote = conn.remote_id();
        let access = self.access.read().await;
        if access.pairing_host_open || access.allowed.contains(&remote) {
            return AfterHandshakeOutcome::accept();
        }
        AfterHandshakeOutcome::Reject {
            error_code: 403u32.into(),
            reason: b"unauthorized control peer".to_vec(),
        }
    }
}

#[derive(Clone)]
struct ControlCtx {
    identity: Arc<DeviceIdentity>,
    paired_store: Arc<PairedDeviceStore>,
    access: Arc<RwLock<AccessState>>,
    pairing_host_open: Arc<AtomicBool>,
    pairing_expire_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    app_handle: AppHandle,
    home_relay_url: Option<String>,
}

#[derive(Clone)]
struct ControlProtocol {
    ctx: ControlCtx,
}

impl std::fmt::Debug for ControlProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlProtocol").finish_non_exhaustive()
    }
}

impl ControlProtocol {
    async fn close_pairing_host(&self) {
        if let Some(handle) = self.ctx.pairing_expire_task.lock().await.take() {
            handle.abort();
        }
        self.ctx.pairing_host_open.store(false, Ordering::SeqCst);
        self.ctx.access.write().await.pairing_host_open = false;
    }

    async fn handle_connection(&self, conn: Connection) -> anyhow::Result<()> {
        let remote = conn.remote_id();
        let allowed = self.is_allowed(&remote).await;
        let keying = export_connection_keying_material(&conn).context("export keying material")?;
        let (mut send, mut recv) = conn
            .accept_bi()
            .await
            .context("accept bi stream for control session")?;

        // Unpaired peers are joining a pairing window — send our identity first.
        // Already-paired peers typically send Invite; let them speak first.
        if !allowed {
            let our_info = ControlMessage::PairingInfo {
                endpoint_id: self.ctx.identity.endpoint_id(),
                display_name: self.ctx.identity.display_name(),
                device_type: self.ctx.identity.device_type(),
                os: self.ctx.identity.os(),
                signature: sign_challenge(&self.ctx.identity.secret_key, &keying),
            };
            write_message(&mut send, &our_info)
                .await
                .context("write local PairingInfo")?;
        }

        let mut remote_info: Option<ControlMessage> = None;
        let mut remote_vote: Option<RememberVote> = None;
        let mut pairing_completed = false;
        let session_id = uuid::Uuid::new_v4().to_string();

        loop {
            let msg = match read_message(&mut recv).await {
                Ok(m) => m,
                Err(_) => break,
            };
            match msg {
                ControlMessage::PairingInfo {
                    endpoint_id,
                    display_name,
                    device_type,
                    os,
                    signature,
                } => {
                    let Ok(peer_id) = EndpointId::from_str(&endpoint_id) else {
                        continue;
                    };
                    if !verify_challenge(&peer_id, &keying, &signature) {
                        warn!("pairing-info signature invalid from {remote}");
                        continue;
                    }
                    remote_info = Some(ControlMessage::PairingInfo {
                        endpoint_id,
                        display_name,
                        device_type,
                        os,
                        signature,
                    });
                }
                ControlMessage::RememberVote { vote, .. } => {
                    remote_vote = Some(vote);
                }
                ControlMessage::Invite {
                    blob_ticket,
                    file_count,
                    total_size,
                    sender_name,
                } => {
                    if !self.is_allowed(&remote).await {
                        continue;
                    }
                    let payload = serde_json::json!({
                        "blob_ticket": blob_ticket,
                        "file_count": file_count,
                        "total_size": total_size,
                        "sender_name": sender_name,
                        "remote_endpoint_id": remote.to_string(),
                    });
                    let ui_notified = match &self.ctx.app_handle {
                        Some(handle) => handle
                            .emit_event_with_payload(
                                "paired-invite-received",
                                &payload.to_string(),
                            )
                            .is_ok(),
                        None => false,
                    };
                    if ui_notified {
                        // Ack delivery so the sender can close without a long hold.
                        let ack = ControlMessage::InviteResponse {
                            session_id: session_id.clone(),
                            response: InviteResponse::Delivered,
                        };
                        let _ = write_message(&mut send, &ack).await;
                    } else {
                        warn!("paired invite from {remote} not surfaced to UI");
                    }
                    return Ok(());
                }
                ControlMessage::InviteResponse { .. } => {}
                ControlMessage::Recognition { signature } => {
                    if verify_challenge(&remote, &keying, &signature) {
                        let _ = self.ctx.paired_store.touch(
                            &remote.to_string(),
                            protocol::identity::unix_now_ms(),
                        );
                    }
                }
            }

            if remote_info.is_some() && remote_vote == Some(RememberVote::Remember) {
                if let Some(ControlMessage::PairingInfo {
                    endpoint_id,
                    display_name,
                    device_type,
                    os,
                    ..
                }) = &remote_info
                {
                    let now = protocol::identity::unix_now_ms();
                    let device = PairedDevice {
                        endpoint_id: endpoint_id.clone(),
                        display_name: display_name.clone(),
                        device_type: device_type.clone(),
                        os: os.clone(),
                        paired_at: now,
                        last_seen_at: now,
                        relay_url: self.ctx.home_relay_url.clone(),
                    };
                    let _ = self.ctx.paired_store.remember(device);
                    self.allow_peer(remote).await;
                    if let Some(handle) = &self.ctx.app_handle {
                        let _ = handle.emit_event("device-paired");
                    }
                }
                pairing_completed = true;
                break;
            }
        }

        if remote_info.is_some() && !allowed {
            let vote = ControlMessage::RememberVote {
                session_id,
                vote: RememberVote::Remember,
            };
            let _ = write_message(&mut send, &vote).await;
        }

        if pairing_completed {
            self.close_pairing_host().await;
        }

        Ok(())
    }

    async fn is_allowed(&self, remote: &EndpointId) -> bool {
        self.ctx.access.read().await.allowed.contains(remote)
    }

    async fn allow_peer(&self, remote: EndpointId) {
        self.ctx.access.write().await.allowed.insert(remote);
    }
}

impl ProtocolHandler for ControlProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let this = self.clone();
        // Run off the router task so accept_bi can progress while the peer opens.
        tokio::spawn(async move {
            if let Err(err) = this.handle_connection(connection).await {
                warn!("control connection failed: {err:#}");
            }
        });
        Ok(())
    }
}

struct NodeRuntime {
    endpoint: Endpoint,
    router: Router,
}

pub struct NodeService {
    runtime: Mutex<NodeRuntime>,
    identity: Arc<DeviceIdentity>,
    paired_store: Arc<PairedDeviceStore>,
    access: Arc<RwLock<AccessState>>,
    pairing_host_open: Arc<AtomicBool>,
    pairing_expire_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    app_handle: AppHandle,
    relay_mode: Mutex<RelayMode>,
}

impl NodeService {
    pub async fn start(
        data_dir: &Path,
        relay_mode: RelayMode,
        app_handle: AppHandle,
    ) -> anyhow::Result<Self> {
        let identity = Arc::new(load_or_create_identity(data_dir)?);
        let paired_store = Arc::new(PairedDeviceStore::new(data_dir));
        let allowed = load_allowed_from_store(&paired_store)?;

        let access = Arc::new(RwLock::new(AccessState {
            allowed,
            pairing_host_open: false,
        }));
        let pairing_host_open = Arc::new(AtomicBool::new(false));
        let pairing_expire_task = Arc::new(Mutex::new(None));

        let runtime = build_runtime(
            identity.clone(),
            paired_store.clone(),
            access.clone(),
            pairing_host_open.clone(),
            pairing_expire_task.clone(),
            app_handle.clone(),
            relay_mode.clone(),
        )
        .await?;

        Ok(Self {
            runtime: Mutex::new(runtime),
            identity,
            paired_store,
            access,
            pairing_host_open,
            pairing_expire_task,
            app_handle,
            relay_mode: Mutex::new(relay_mode),
        })
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.stop_pairing_host().await;
        let runtime = self.runtime.lock().await;
        runtime.router.shutdown().await?;
        runtime.endpoint.close().await;
        Ok(())
    }

    pub async fn reconfigure_relay(&self, relay_mode: RelayMode) -> anyhow::Result<()> {
        {
            let current = self.relay_mode.lock().await;
            if format!("{current:?}") == format!("{relay_mode:?}") {
                return Ok(());
            }
        }

        self.stop_pairing_host().await;

        let mut runtime = self.runtime.lock().await;
        runtime.router.shutdown().await?;
        runtime.endpoint.close().await;

        let new_runtime = build_runtime(
            self.identity.clone(),
            self.paired_store.clone(),
            self.access.clone(),
            self.pairing_host_open.clone(),
            self.pairing_expire_task.clone(),
            self.app_handle.clone(),
            relay_mode.clone(),
        )
        .await?;

        *runtime = new_runtime;
        *self.relay_mode.lock().await = relay_mode;
        Ok(())
    }

    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo::from(self.identity.as_ref())
    }

    pub fn set_device_display_name(&self, display_name: &str) -> anyhow::Result<DeviceInfo> {
        self.identity.set_display_name(display_name)
    }

    pub fn rename_paired(
        &self,
        endpoint_id: &str,
        display_name: &str,
    ) -> anyhow::Result<PairedDevice> {
        self.paired_store.rename(endpoint_id, display_name)
    }

    pub fn list_paired(&self) -> anyhow::Result<Vec<PairedDevice>> {
        self.paired_store.list()
    }

    pub async fn forget_paired(&self, endpoint_id: &str) -> anyhow::Result<()> {
        if let Ok(id) = EndpointId::from_str(endpoint_id) {
            self.access.write().await.allowed.remove(&id);
        }
        self.paired_store.forget(endpoint_id)
    }

    pub fn pairing_ticket(&self) -> anyhow::Result<String> {
        let runtime = self.runtime.try_lock().context("node runtime busy")?;
        let mut addr = runtime.endpoint.addr();
        apply_options(&mut addr, AddrInfoOptions::Relay);
        let relay_url = addr.relay_urls().next().map(|u| u.to_string());
        let ticket = protocol::PairingTicket {
            v: 1,
            kind: protocol::PairingTicket::KIND.to_string(),
            endpoint_id: self.identity.endpoint_id(),
            relay_url,
        };
        ticket.encode()
    }

    pub async fn start_pairing_host(&self) -> anyhow::Result<String> {
        self.stop_pairing_host().await;

        self.pairing_host_open.store(true, Ordering::SeqCst);
        self.access.write().await.pairing_host_open = true;

        let access = self.access.clone();
        let flag = self.pairing_host_open.clone();
        let app_handle = self.app_handle.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(
                protocol::pairing::PAIRING_VOTE_TIMEOUT_SECS,
            ))
            .await;
            flag.store(false, Ordering::SeqCst);
            access.write().await.pairing_host_open = false;
            if let Some(handle) = &app_handle {
                let _ = handle.emit_event("pairing-host-expired");
            }
        });
        *self.pairing_expire_task.lock().await = Some(handle);

        self.pairing_ticket()
    }

    pub async fn stop_pairing_host(&self) {
        if let Some(handle) = self.pairing_expire_task.lock().await.take() {
            handle.abort();
        }
        self.pairing_host_open.store(false, Ordering::SeqCst);
        self.access.write().await.pairing_host_open = false;
    }

    pub async fn join_pairing(&self, ticket_str: &str) -> anyhow::Result<()> {
        let ticket = protocol::PairingTicket::decode(ticket_str)?;
        let remote = EndpointId::from_str(&ticket.endpoint_id)?;
        let host_relay_url = ticket.relay_url.clone();
        let mut addr = EndpointAddr::from(remote);
        if let Some(relay) = host_relay_url.as_deref() {
            if let Ok(url) = relay.parse() {
                addr.addrs.insert(TransportAddr::Relay(url));
            }
        }

        let runtime = self.runtime.lock().await;
        let conn = runtime
            .endpoint
            .connect(addr, CONTROL_ALPN)
            .await
            .context("pairing connect failed")?;
        drop(runtime);

        let keying = export_connection_keying_material(&conn)?;
        let (mut send, mut recv) = conn.open_bi().await.context("open bi stream for join")?;

        // Speak first so the host can accept_bi and complete the handshake.
        let info = ControlMessage::PairingInfo {
            endpoint_id: self.identity.endpoint_id(),
            display_name: self.identity.display_name(),
            device_type: self.identity.device_type(),
            os: self.identity.os(),
            signature: sign_challenge(&self.identity.secret_key, &keying),
        };
        write_message(&mut send, &info)
            .await
            .context("write local PairingInfo")?;

        let vote = ControlMessage::RememberVote {
            session_id: uuid::Uuid::new_v4().to_string(),
            vote: RememberVote::Remember,
        };
        write_message(&mut send, &vote)
            .await
            .context("write RememberVote")?;

        let host_info = read_message(&mut recv)
            .await
            .context("read host PairingInfo")?;
        let ControlMessage::PairingInfo {
            endpoint_id,
            display_name,
            device_type,
            os,
            signature,
        } = host_info
        else {
            anyhow::bail!("expected host PairingInfo");
        };
        let peer_id = EndpointId::from_str(&endpoint_id).context("invalid host endpoint id")?;
        if !verify_challenge(&peer_id, &keying, &signature) {
            anyhow::bail!("host PairingInfo signature invalid");
        }

        let now = protocol::identity::unix_now_ms();
        self.paired_store.remember(PairedDevice {
            endpoint_id: endpoint_id.clone(),
            display_name,
            device_type,
            os,
            paired_at: now,
            last_seen_at: now,
            relay_url: host_relay_url,
        })?;
        self.access.write().await.allowed.insert(peer_id);
        if let Some(handle) = &self.app_handle {
            let _ = handle.emit_event("device-paired");
        }
        Ok(())
    }

    pub async fn invite_paired_device(
        &self,
        remote_endpoint_id: &str,
        blob_ticket: &str,
        file_count: u32,
        total_size: u64,
    ) -> anyhow::Result<bool> {
        let remote = EndpointId::from_str(remote_endpoint_id)?;
        if !self.access.read().await.allowed.contains(&remote) {
            anyhow::bail!("unknown paired device");
        }

        let stored_relay = self
            .paired_store
            .get(remote_endpoint_id)?
            .and_then(|d| d.relay_url);

        let runtime = self.runtime.lock().await;
        let addr = build_control_connect_addr(&runtime.endpoint, remote, stored_relay.as_deref());
        let connect = tokio::time::timeout(
            Duration::from_secs(30),
            runtime.endpoint.connect(addr, CONTROL_ALPN),
        )
        .await;
        drop(runtime);

        let conn = match connect {
            Ok(Ok(conn)) => conn,
            Ok(Err(err)) => {
                debug!(error = %err, "invite connect failed");
                return Ok(false);
            }
            Err(_) => {
                debug!("invite connect timed out");
                return Ok(false);
            }
        };

        let (mut send, mut recv) = match conn.open_bi().await {
            Ok(streams) => streams,
            Err(err) => {
                debug!(error = %err, "invite open_bi failed");
                return Ok(false);
            }
        };

        let invite = ControlMessage::Invite {
            blob_ticket: blob_ticket.to_string(),
            file_count,
            total_size,
            sender_name: self.identity.display_name(),
        };
        if let Err(err) = write_message(&mut send, &invite).await {
            debug!(error = %err, "invite write failed");
            return Ok(false);
        }

        // Wait briefly for Delivered ack, then close.
        let delivered = match tokio::time::timeout(Duration::from_secs(5), read_message(&mut recv))
            .await
        {
            Ok(Ok(ControlMessage::InviteResponse {
                response: InviteResponse::Delivered | InviteResponse::Accepted,
                ..
            })) => true,
            Ok(Ok(_)) => false,
            // Older receivers never ack; invite may still have been delivered.
            Ok(Err(_)) | Err(_) => true,
        };

        drop(send);
        drop(recv);
        Ok(delivered)
    }
}

fn build_control_connect_addr(
    endpoint: &Endpoint,
    remote: EndpointId,
    stored_relay: Option<&str>,
) -> EndpointAddr {
    let mut addr = EndpointAddr::from(remote);
    if let Some(relay) = stored_relay {
        if let Ok(url) = relay.parse() {
            addr.addrs.insert(TransportAddr::Relay(url));
        }
    }
    let mut local = endpoint.addr();
    apply_options(&mut local, AddrInfoOptions::Relay);
    if let Some(relay) = local.relay_urls().next() {
        addr.addrs.insert(TransportAddr::Relay(relay.clone()));
    }
    addr
}

fn load_allowed_from_store(paired_store: &PairedDeviceStore) -> anyhow::Result<HashSet<EndpointId>> {
    let mut allowed = HashSet::new();
    for device in paired_store.list()? {
        if let Ok(id) = EndpointId::from_str(&device.endpoint_id) {
            allowed.insert(id);
        }
    }
    Ok(allowed)
}

async fn build_runtime(
    identity: Arc<DeviceIdentity>,
    paired_store: Arc<PairedDeviceStore>,
    access: Arc<RwLock<AccessState>>,
    pairing_host_open: Arc<AtomicBool>,
    pairing_expire_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    app_handle: AppHandle,
    relay_mode: RelayMode,
) -> anyhow::Result<NodeRuntime> {
    let hook = PairedOnlyHook {
        access: access.clone(),
    };

    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(identity.secret_key.clone())
        .address_lookup(PkarrPublisher::n0_dns())
        .relay_mode(relay_mode)
        .hooks(hook)
        .alpns(vec![CONTROL_ALPN.to_vec()])
        .bind()
        .await?;

    endpoint.online().await;

    let mut local_addr = endpoint.addr();
    apply_options(&mut local_addr, AddrInfoOptions::Relay);
    let home_relay_url = local_addr.relay_urls().next().map(|u| u.to_string());

    let control = ControlProtocol {
        ctx: ControlCtx {
            identity,
            paired_store,
            access,
            pairing_host_open,
            pairing_expire_task,
            app_handle,
            home_relay_url,
        },
    };

    let router = Router::builder(endpoint.clone())
        .accept(CONTROL_ALPN, control)
        .spawn();

    Ok(NodeRuntime { endpoint, router })
}
