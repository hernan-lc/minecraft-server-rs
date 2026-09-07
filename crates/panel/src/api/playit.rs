//! Playit account, claim, and tunnel endpoints.

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use playit_integration::{
    AccountSessionState, AgentInfo, ClaimDetailsInfo, ClaimInfo, DeleteAgentOptions,
    DirectSetupResult, DomainInfo, EnsureTunnelDisposition, EnsuredServerTunnel, PlayitAccount,
    PlayitConnectionState, PlayitProtocol, PlayitStatus, PlayitTunnel, TunnelCreateInfo,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::{AdminIdentity, Identity};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::store::{now_unix_seconds, queue_playit_cleanup, PlayitBinding, ServerRecord};

const PROVISIONING_GRACE_SECS: u64 = 60;

/// The panel's view of a server's Playit tunnel association.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerPlayitState {
    /// No tunnel is configured for this server.
    Disabled,
    /// The id is stored, but Playit has not materialized it in the list yet.
    Provisioning,
    /// The Playit service reports a usable tunnel with the expected destination.
    Connected,
    /// The control connection is recovering, so tunnel data is not trusted as
    /// a healthy connection yet.
    Reconnecting,
    /// Playit knows the tunnel but has disabled it.
    DisabledByPlayit,
    /// Playit reports a destination different from the panel binding.
    Drifted,
    /// The persisted tunnel id is no longer visible after its provisioning
    /// grace period.
    Missing,
    /// The binding was created under a different observed Playit agent.
    AccountMismatch,
    /// The live tunnel belongs to another or unknown agent.
    AgentMismatch,
    /// More than one tunnel matched a managed or legacy identity.
    Ambiguous,
    /// The Playit service could not be queried.
    Unavailable,
}

/// How a server tunnel attach obtained its tunnel, when the response is the
/// direct result of an attach operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayitAttachDisposition {
    /// A new tunnel was created.
    Created,
    /// An existing tunnel was reused unchanged.
    Reused,
    /// An existing tunnel was updated/reassigned in place.
    Updated,
}

impl From<&EnsureTunnelDisposition> for PlayitAttachDisposition {
    fn from(disposition: &EnsureTunnelDisposition) -> Self {
        match disposition {
            EnsureTunnelDisposition::Created => Self::Created,
            EnsureTunnelDisposition::Reused => Self::Reused,
            EnsureTunnelDisposition::Updated { .. } => Self::Updated,
        }
    }
}

/// A safe server-scoped Playit response.
#[derive(Debug, Serialize)]
pub struct ServerPlayitView {
    /// The state the client should display.
    pub state: ServerPlayitState,
    /// The panel's persisted association, if one exists.
    pub binding: Option<PlayitBinding>,
    /// The matching live Playit tunnel, if it is currently visible.
    pub tunnel: Option<PlayitTunnel>,
    /// A diagnostic or provisioning note, when useful.
    pub message: Option<String>,
    /// A remote deletion remains in the durable cleanup queue.
    pub cleanup_pending: bool,
    /// How the tunnel was obtained, when this response follows an attach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<PlayitAttachDisposition>,
}

/// GET `/api/playit/status`.
///
/// A missing or stopped Playit service is a normal deployment state, so this endpoint
/// returns a usable status document instead of preventing the panel from
/// starting or turning the status check into a generic HTTP 500.
async fn status(State(state): State<Arc<AppState>>, _: Identity) -> Json<PlayitStatus> {
    let status = match state.playit.status().await {
        Ok(status) => status,
        Err(error) => {
            tracing::warn!(error = ?error, "Playit status unavailable");
            safe_status(playit_integration::PlayitManager::status_from_error(&error))
        }
    };
    Json(safe_status(status))
}

/// Convert a detailed integration failure into a status message that is safe
/// to expose over HTTP. The full error is logged above for operators, but it
/// may contain local paths, IPC details, or secret-file names.
fn safe_status(mut status: PlayitStatus) -> PlayitStatus {
    status.message = match status.status {
        PlayitConnectionState::Unavailable => Some("Playit service unavailable".into()),
        PlayitConnectionState::Unsupported => Some("Playit service protocol unsupported".into()),
        PlayitConnectionState::Error => Some("Playit service error".into()),
        _ => None,
    };
    status
}

/// GET `/api/playit/account`.
async fn account(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> ApiResult<Json<PlayitAccount>> {
    Ok(Json(state.playit.account().await?))
}

/// POST `/api/playit/claim`.
async fn claim(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> ApiResult<Json<ClaimInfo>> {
    Ok(Json(state.playit.start_claim().await?))
}

#[derive(Debug, Deserialize)]
struct CreateTunnelRequest {
    local_port: u16,
    #[serde(default)]
    protocol: PlayitProtocol,
    #[serde(default)]
    local_address: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// GET `/api/playit/tunnels`.
///
/// The account-level list is preferred so tunnels assigned to another Playit
/// agent are visible and can be identified before an operator creates another
/// one. When the account endpoint is denied, the current-agent materialized
/// list is returned instead of failing the whole Playit page with 403.
async fn list_tunnels(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> ApiResult<Json<Vec<PlayitTunnel>>> {
    Ok(Json(state.playit.visible_tunnels().await?))
}

/// POST `/api/playit/tunnels`.
async fn create_tunnel(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
    Json(body): Json<CreateTunnelRequest>,
) -> ApiResult<Json<TunnelCreateInfo>> {
    let local_address = local_address(body.local_address)?;
    let name = tunnel_name(body.name)?;

    if body.local_port == 0 {
        return Err(ApiError::BadRequest(
            "local_port must be between 1 and 65535".into(),
        ));
    }

    Ok(Json(
        state
            .playit
            .create_tunnel(body.local_port, body.protocol, Some(local_address), name)
            .await?,
    ))
}

/// DELETE `/api/playit/tunnels/:id`.
async fn delete_tunnel(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
    Path(tunnel_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let _server_lock = state.server_mutation_lock.lock().await;
    // Record the local desired state before the remote operation. If the
    // store write fails, no remote deletion is attempted; if Playit is down,
    // the queued cleanup can be retried later without leaving a stale server
    // association behind.
    let tunnel_id_for_store = tunnel_id.clone();
    let affected_servers = state
        .store
        .try_update(move |data| -> ApiResult<Vec<String>> {
            let mut affected = Vec::new();
            for server in &mut data.servers {
                if server
                    .playit
                    .as_ref()
                    .is_some_and(|binding| binding.tunnel_id == tunnel_id_for_store)
                {
                    server.playit = None;
                    affected.push(server.id.clone());
                }
            }
            for server_id in &affected {
                queue_playit_cleanup(data, server_id, &tunnel_id_for_store);
            }
            Ok(affected)
        })
        .await??;

    let remote_result = state.playit.delete_tunnel(&tunnel_id).await;
    let remote_pending = match remote_result {
        Ok(()) => {
            for server_id in &affected_servers {
                remove_cleanup(&state, server_id, &tunnel_id).await;
            }
            false
        }
        Err(error) if error.is_not_found() => {
            for server_id in &affected_servers {
                remove_cleanup(&state, server_id, &tunnel_id).await;
            }
            false
        }
        Err(error) if affected_servers.is_empty() => return Err(ApiError::Playit(error)),
        Err(error) => {
            tracing::warn!(
                tunnel = %tunnel_id,
                error = ?error,
                "direct Playit tunnel deletion is pending"
            );
            true
        }
    };

    Ok(Json(serde_json::json!({
        "ok": true,
        "cleanup_pending": remote_pending,
    })))
}

#[derive(Debug, Default, Deserialize)]
struct AttachServerTunnelRequest {
    /// An optional operator-facing override. The default is stable and
    /// includes the server id so renaming a server does not break matching.
    #[serde(default)]
    name: Option<String>,
}

/// GET `/api/servers/:id/playit`.
async fn server_playit(
    State(state): State<Arc<AppState>>,
    identity: Identity,
    Path(id): Path<String>,
) -> ApiResult<Json<ServerPlayitView>> {
    let record = authorized_server(&state, &identity, &id).await?;
    Ok(Json(server_playit_view(&state, &record).await))
}

/// POST `/api/servers/:id/playit`.
///
/// Server tunnels deliberately use loopback and the server's configured port;
/// arbitrary destinations belong to the admin-only global tunnel endpoint.
async fn attach_server_playit(
    State(state): State<Arc<AppState>>,
    AdminIdentity(admin): AdminIdentity,
    Path(id): Path<String>,
    Json(body): Json<AttachServerTunnelRequest>,
) -> ApiResult<Json<ServerPlayitView>> {
    let _server_lock = state.server_mutation_lock.lock().await;
    let mut record = authorized_server(&state, &admin, &id).await?;
    let previous_binding = record.playit.clone();
    if previous_binding.is_none() {
        // Do not start a new association while an older remote deletion is
        // still pending. Otherwise a retry could race the cleanup and leave
        // the server with an untracked or newly-deleted tunnel.
        retry_server_cleanup(&state, &id).await;
        if cleanup_pending_for_server(&state, &id).await {
            return Err(ApiError::Conflict(
                "Playit cleanup is still pending for this server; reconcile or forget the association before attaching"
                    .into(),
            ));
        }
    }
    let current_agent_id = required_current_agent_id(&state).await?;

    // An operator-supplied display name is presentation metadata only: it is
    // used when a new tunnel must be created, and never bypasses the reuse
    // logic that keeps repeated attaches idempotent.
    let custom_name = tunnel_name(body.name)?;
    let ensured = if let Some(existing_binding) = previous_binding.as_ref() {
        match inspect_existing_binding(&state, &record, existing_binding, &current_agent_id).await?
        {
            ExistingBinding::Present(ensured) => ensured,
            // A retry is an explicit repair request. The stable matcher
            // still runs first, so a missing stale id cannot create a
            // duplicate when the same managed tunnel is visible again.
            ExistingBinding::Missing => {
                state
                    .playit
                    .ensure_server_tunnel_with_name(
                        &id,
                        &record.name,
                        record.config.port,
                        custom_name,
                    )
                    .await?
            }
        }
    } else {
        state
            .playit
            .ensure_server_tunnel_with_name(&id, &record.name, record.config.port, custom_name)
            .await?
    };

    let binding = PlayitBinding {
        tunnel_id: ensured.tunnel.tunnel_id.clone(),
        protocol: PlayitProtocol::Tcp,
        // A persisted binding is input from an older state file, not proof
        // that its destination is still safe. Repairs always fall back to
        // loopback rather than carrying an untrusted address into Playit.
        local_address: previous_binding
            .as_ref()
            .filter(|binding| is_loopback_address(&binding.local_address))
            .map(|binding| binding.local_address.clone())
            .unwrap_or_else(|| "127.0.0.1".into()),
        local_port: record.config.port,
        agent_id: Some(current_agent_id),
        created_at: previous_binding
            .as_ref()
            .filter(|binding| binding.tunnel_id == ensured.tunnel.tunnel_id)
            .and_then(|binding| binding.created_at)
            .or_else(|| Some(now_unix_seconds())),
    };

    let expected_tunnel_id = previous_binding
        .as_ref()
        .map(|binding| binding.tunnel_id.clone());
    let binding_for_store = binding.clone();
    let server_id = id.clone();
    let write = state
        .store
        .try_update(move |data| -> ApiResult<()> {
            let Some(server) = data
                .servers
                .iter_mut()
                .find(|server| server.id == server_id)
            else {
                return Err(ApiError::NotFound("server".into()));
            };
            if server
                .playit
                .as_ref()
                .map(|current| current.tunnel_id != expected_tunnel_id.as_deref().unwrap_or(""))
                .unwrap_or(expected_tunnel_id.is_some())
            {
                return Err(ApiError::Conflict(
                    "the server's Playit association changed while it was being attached; retry"
                        .into(),
                ));
            }
            if server.config.port != binding_for_store.local_port {
                return Err(ApiError::Conflict(
                    "the server port changed while the Playit tunnel was being created; retry"
                        .into(),
                ));
            }
            server.playit = Some(binding_for_store);
            Ok(())
        })
        .await;

    match write {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            compensate_attach_failure(&state, &id, &ensured).await;
            return Err(error);
        }
        Err(error) => {
            compensate_attach_failure(&state, &id, &ensured).await;
            return Err(ApiError::Internal(error));
        }
    }

    record.playit = Some(binding);
    let disposition = PlayitAttachDisposition::from(&ensured.disposition);
    let mut view = server_playit_view(&state, &record).await;
    view.disposition = Some(disposition);
    Ok(Json(view))
}

/// Compensate an attach whose remote ensure succeeded but whose local commit
/// did not. A newly-created tunnel is deleted; if that delete is unavailable,
/// retain it in the durable cleanup queue. Reused and updated tunnels are
/// always preserved by the manager.
async fn compensate_attach_failure(
    state: &AppState,
    server_id: &str,
    ensured: &EnsuredServerTunnel,
) {
    if let Err(error) = state.playit.compensate_ensure_failure(ensured).await {
        let cleanup_server_id = server_id.to_owned();
        let cleanup_tunnel_id = ensured.tunnel.tunnel_id.clone();
        if let Err(queue_error) = state
            .store
            .update(move |data| {
                queue_playit_cleanup(data, &cleanup_server_id, &cleanup_tunnel_id);
            })
            .await
        {
            tracing::error!(
                server = %server_id,
                tunnel = %ensured.tunnel.tunnel_id,
                error = ?error,
                queue_error = ?queue_error,
                "newly-created Playit tunnel could not be compensated or queued for cleanup"
            );
        } else {
            tracing::warn!(
                server = %server_id,
                tunnel = %ensured.tunnel.tunnel_id,
                error = ?error,
                "newly-created Playit tunnel cleanup was queued after local attach failure"
            );
        }
    }
}

enum ExistingBinding {
    Present(EnsuredServerTunnel),
    Missing,
}

async fn required_current_agent_id(state: &AppState) -> ApiResult<String> {
    let account = state.playit.account().await?;
    account
        .agent_id
        .filter(|agent_id| !agent_id.trim().is_empty())
        .ok_or_else(|| {
            ApiError::Playit(playit_integration::PlayitError::Unavailable(
                "the current Playit agent id is not available yet".into(),
            ))
        })
}

/// Inspect a stored binding without ever creating a second tunnel for it.
/// A live Java tunnel with a safe but stale assignment is repaired in place;
/// disabled, ambiguous, and incompatible tunnels remain visible as conflicts.
async fn inspect_existing_binding(
    state: &AppState,
    record: &ServerRecord,
    binding: &PlayitBinding,
    current_agent_id: &str,
) -> ApiResult<ExistingBinding> {
    if binding
        .agent_id
        .as_deref()
        .is_some_and(|agent_id| agent_id != current_agent_id)
    {
        return Err(ApiError::Conflict(
            "this Playit association belongs to a different account or agent; reconcile it explicitly"
                .into(),
        ));
    }

    if !is_loopback_address(&binding.local_address) {
        return Err(ApiError::Conflict(
            "the stored Playit association has an unsafe local destination; forget it before repairing"
                .into(),
        ));
    }

    // Use the resilient tunnel source: when the account-wide list is
    // denied, what is visible on the current agent is enough to repair the
    // stored binding instead of failing the attach.
    let tunnels = state.playit.visible_tunnels().await?;
    let matches: Vec<_> = tunnels
        .into_iter()
        .filter(|tunnel| tunnel.id == binding.tunnel_id)
        .collect();
    if matches.len() > 1 {
        return Err(ApiError::Conflict(
            "Playit returned duplicate records for the stored tunnel id".into(),
        ));
    }
    let Some(tunnel) = matches.into_iter().next() else {
        return Ok(ExistingBinding::Missing);
    };

    if tunnel.disabled {
        return Err(ApiError::Conflict(format!(
            "Playit disabled this tunnel{}",
            tunnel
                .disabled_reason
                .as_deref()
                .map(|reason| format!(": {reason}"))
                .unwrap_or_default()
        )));
    }
    // The current-agent fallback list carries no semantic tunnel type, so
    // `None` is unknown-but-repairable when the stored id matched exactly.
    if tunnel
        .tunnel_type
        .as_deref()
        .is_some_and(|kind| kind != "minecraft-java")
    {
        return Err(ApiError::Conflict(
            "the stored Playit tunnel is not a Minecraft Java tunnel".into(),
        ));
    }
    if tunnel.protocol != PlayitProtocol::Tcp {
        return Err(ApiError::Conflict(
            "the stored Playit tunnel does not use Minecraft Java TCP".into(),
        ));
    }

    // A live tunnel whose destination drifted (e.g. the server port changed)
    // or whose agent changed is repaired in place: the same tunnel id is
    // kept and reassigned rather than duplicated.
    let destination_changed = tunnel.local_address.as_deref()
        != Some(binding.local_address.as_str())
        || tunnel.local_port != Some(record.config.port);
    let agent_changed = tunnel.agent_id.as_deref() != Some(current_agent_id);
    if destination_changed || agent_changed {
        state
            .playit
            .reassign_tunnel(
                &binding.tunnel_id,
                record.config.port,
                Some(binding.local_address.clone()),
            )
            .await?;
        return Ok(ExistingBinding::Present(EnsuredServerTunnel {
            tunnel: TunnelCreateInfo {
                tunnel_id: binding.tunnel_id.clone(),
                message: Some("Existing Minecraft Java tunnel updated".into()),
            },
            disposition: EnsureTunnelDisposition::Updated {
                previous_agent_id: tunnel.agent_id,
                destination_changed,
            },
        }));
    }

    Ok(ExistingBinding::Present(EnsuredServerTunnel {
        tunnel: TunnelCreateInfo {
            tunnel_id: binding.tunnel_id.clone(),
            message: Some("Existing Minecraft Java tunnel reused".into()),
        },
        disposition: EnsureTunnelDisposition::Reused,
    }))
}

/// DELETE `/api/servers/:id/playit`.
async fn detach_server_playit(
    State(state): State<Arc<AppState>>,
    AdminIdentity(admin): AdminIdentity,
    Path(id): Path<String>,
) -> ApiResult<Json<ServerPlayitView>> {
    let _server_lock = state.server_mutation_lock.lock().await;
    let mut record = authorized_server(&state, &admin, &id).await?;

    let Some(binding) = record.playit.clone() else {
        retry_server_cleanup(&state, &id).await;
        return Ok(Json(server_playit_view(&state, &record).await));
    };

    // Commit the desired local state before contacting Playit. This makes the
    // operation recoverable when Playit is down and prevents a successful
    // remote delete from leaving a durable local binding to a missing tunnel.
    let server_id = id.clone();
    let tunnel_id = binding.tunnel_id.clone();
    let cleared = state
        .store
        .try_update(move |data| -> ApiResult<bool> {
            let Some(server) = data
                .servers
                .iter_mut()
                .find(|server| server.id == server_id)
            else {
                return Err(ApiError::NotFound("server".into()));
            };
            if server
                .playit
                .as_ref()
                .is_some_and(|current| current.tunnel_id == tunnel_id)
            {
                server.playit = None;
                queue_playit_cleanup(data, &server_id, &tunnel_id);
                Ok(true)
            } else {
                Ok(false)
            }
        })
        .await??;

    if !cleared {
        return Err(ApiError::Conflict(
            "the server's Playit tunnel changed while it was being detached; retry".into(),
        ));
    }

    match state.playit.delete_tunnel(&binding.tunnel_id).await {
        Ok(()) => remove_cleanup(&state, &id, &binding.tunnel_id).await,
        Err(error) if error.is_not_found() => remove_cleanup(&state, &id, &binding.tunnel_id).await,
        Err(error) => {
            tracing::warn!(
                server = %id,
                tunnel = %binding.tunnel_id,
                error = ?error,
                "Playit tunnel cleanup is pending after local detach"
            );
        }
    }

    record.playit = None;
    Ok(Json(server_playit_view(&state, &record).await))
}

/// Explicitly remove only the panel association. The remote tunnel is left
/// untouched so this is safe to use while Playit is unavailable or the account
/// has changed.
async fn forget_server_playit(
    State(state): State<Arc<AppState>>,
    AdminIdentity(admin): AdminIdentity,
    Path(id): Path<String>,
) -> ApiResult<Json<ServerPlayitView>> {
    let _server_lock = state.server_mutation_lock.lock().await;
    let mut record = authorized_server(&state, &admin, &id).await?;
    let expected_tunnel_id = record
        .playit
        .as_ref()
        .map(|binding| binding.tunnel_id.clone());

    state
        .store
        .try_update(move |data| -> ApiResult<()> {
            let Some(server) = data.servers.iter_mut().find(|server| server.id == id) else {
                return Err(ApiError::NotFound("server".into()));
            };
            if expected_tunnel_id.as_deref()
                == server
                    .playit
                    .as_ref()
                    .map(|binding| binding.tunnel_id.as_str())
            {
                server.playit = None;
            }
            data.playit_cleanup
                .retain(|cleanup| cleanup.server_id != id);
            Ok(())
        })
        .await??;

    record.playit = None;
    Ok(Json(server_playit_view(&state, &record).await))
}

/// Reconcile a stored association in place. It never creates or deletes a
/// tunnel: missing, disabled, incompatible, and ambiguous records remain
/// visible for an explicit repair or forget operation.
async fn reconcile_server_playit(
    State(state): State<Arc<AppState>>,
    AdminIdentity(admin): AdminIdentity,
    Path(id): Path<String>,
) -> ApiResult<Json<ServerPlayitView>> {
    let _server_lock = state.server_mutation_lock.lock().await;
    let mut record = authorized_server(&state, &admin, &id).await?;

    retry_server_cleanup(&state, &id).await;

    let Some(binding) = record.playit.clone() else {
        return Ok(Json(server_playit_view(&state, &record).await));
    };
    let current_agent_id = required_current_agent_id(&state).await?;
    if let Ok(ExistingBinding::Present(_)) =
        inspect_existing_binding(&state, &record, &binding, &current_agent_id).await
    {
        let mut next = binding.clone();
        next.agent_id = Some(current_agent_id);
        next.local_port = record.config.port;
        let server_id = id.clone();
        let expected_id = binding.tunnel_id.clone();
        state
            .store
            .try_update(move |data| -> ApiResult<()> {
                let Some(server) = data
                    .servers
                    .iter_mut()
                    .find(|server| server.id == server_id)
                else {
                    return Err(ApiError::NotFound("server".into()));
                };
                if server
                    .playit
                    .as_ref()
                    .map(|binding| binding.tunnel_id.as_str())
                    != Some(expected_id.as_str())
                {
                    return Err(ApiError::Conflict(
                        "the server's Playit association changed while it was being reconciled"
                            .into(),
                    ));
                }
                server.playit = Some(next);
                Ok(())
            })
            .await??;
        record.playit = state
            .store
            .server(&id)
            .await
            .and_then(|server| server.playit);
    }

    Ok(Json(server_playit_view(&state, &record).await))
}

async fn authorized_server(
    state: &AppState,
    identity: &Identity,
    id: &str,
) -> ApiResult<ServerRecord> {
    if !identity.may_access(id) {
        return Err(ApiError::NotFound("server".into()));
    }
    state
        .store
        .server(id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("server {id}")))
}

async fn server_playit_view(state: &AppState, record: &ServerRecord) -> ServerPlayitView {
    let cleanup_pending = state
        .store
        .read()
        .await
        .playit_cleanup
        .iter()
        .any(|cleanup| cleanup.server_id == record.id);

    let Some(binding) = record.playit.clone() else {
        return ServerPlayitView {
            state: ServerPlayitState::Disabled,
            binding: None,
            tunnel: None,
            message: cleanup_pending.then_some(
                "The local Playit association is cleared; remote tunnel cleanup is pending".into(),
            ),
            cleanup_pending,
            disposition: None,
        };
    };

    let service_status = match state.playit.status().await {
        Ok(status) => status,
        Err(error) => {
            tracing::warn!(error = ?error, "Playit status unavailable for server view");
            return ServerPlayitView {
                state: ServerPlayitState::Unavailable,
                binding: Some(binding),
                tunnel: None,
                message: Some("Playit service unavailable".into()),
                cleanup_pending,
                disposition: None,
            };
        }
    };
    match service_status.status {
        PlayitConnectionState::Reconnecting
        | PlayitConnectionState::Starting
        | PlayitConnectionState::Stopping => {
            return ServerPlayitView {
                state: ServerPlayitState::Reconnecting,
                binding: Some(binding),
                tunnel: None,
                message: Some("Playit is reconnecting; tunnel state is not trusted yet".into()),
                cleanup_pending,
                disposition: None,
            };
        }
        PlayitConnectionState::Connected => {}
        PlayitConnectionState::NeedsClaim => {
            return ServerPlayitView {
                state: ServerPlayitState::Unavailable,
                binding: Some(binding),
                tunnel: None,
                message: Some("Playit account needs to be claimed".into()),
                cleanup_pending,
                disposition: None,
            };
        }
        PlayitConnectionState::Unavailable
        | PlayitConnectionState::Unsupported
        | PlayitConnectionState::Error => {
            return ServerPlayitView {
                state: ServerPlayitState::Unavailable,
                binding: Some(binding),
                tunnel: None,
                message: Some("Playit service unavailable".into()),
                cleanup_pending,
                disposition: None,
            };
        }
    }

    let account = match state.playit.account().await {
        Ok(account) => account,
        Err(error) => {
            tracing::warn!(error = ?error, "Playit account unavailable for server view");
            return ServerPlayitView {
                state: ServerPlayitState::Unavailable,
                binding: Some(binding),
                tunnel: None,
                message: Some("Playit account unavailable".into()),
                cleanup_pending,
                disposition: None,
            };
        }
    };
    let Some(current_agent_id) = account
        .agent_id
        .as_deref()
        .filter(|agent_id| !agent_id.trim().is_empty())
    else {
        return ServerPlayitView {
            state: ServerPlayitState::Unavailable,
            binding: Some(binding),
            tunnel: None,
            message: Some("The current Playit agent is not known yet".into()),
            cleanup_pending,
            disposition: None,
        };
    };

    if binding
        .agent_id
        .as_deref()
        .is_some_and(|agent_id| agent_id != current_agent_id)
    {
        return ServerPlayitView {
            state: ServerPlayitState::AccountMismatch,
            binding: Some(binding),
            tunnel: None,
            message: Some(
                "The stored Playit association belongs to a different account or agent".into(),
            ),
            cleanup_pending,
            disposition: None,
        };
    }

    // The resilient tunnel source keeps per-server views working when the
    // account-wide list is denied but the current agent still sees tunnels.
    let tunnels = match state.playit.visible_tunnels().await {
        Ok(tunnels) => tunnels,
        Err(error) => {
            tracing::warn!(error = ?error, "Playit status unavailable for server view");
            return ServerPlayitView {
                state: ServerPlayitState::Unavailable,
                binding: Some(binding),
                tunnel: None,
                message: Some("Playit service unavailable".into()),
                cleanup_pending,
                disposition: None,
            };
        }
    };

    let matches: Vec<_> = tunnels
        .into_iter()
        .filter(|tunnel| tunnel.id == binding.tunnel_id)
        .collect();
    if matches.len() > 1 {
        return ServerPlayitView {
            state: ServerPlayitState::Ambiguous,
            binding: Some(binding),
            tunnel: None,
            message: Some("Playit returned duplicate records for this tunnel id".into()),
            cleanup_pending,
            disposition: None,
        };
    }
    let Some(tunnel) = matches.into_iter().next() else {
        let state = missing_tunnel_state(binding.created_at, now_unix_seconds());
        return ServerPlayitView {
            state,
            binding: Some(binding),
            tunnel: None,
            message: Some(match state {
                ServerPlayitState::Provisioning => {
                    "Playit accepted the tunnel, but it is not visible in the service yet".into()
                }
                ServerPlayitState::Missing => {
                    "The stored Playit tunnel is missing; repair or forget the association".into()
                }
                _ => unreachable!("missing tunnel only has provisioning or missing states"),
            }),
            cleanup_pending,
            disposition: None,
        };
    };

    let (state, message) =
        classify_server_tunnel(&binding, record.config.port, &tunnel, current_agent_id);

    ServerPlayitView {
        state,
        binding: Some(binding),
        tunnel: Some(tunnel),
        message,
        cleanup_pending,
        disposition: None,
    }
}

fn missing_tunnel_state(created_at: Option<u64>, now: u64) -> ServerPlayitState {
    match created_at.and_then(|created_at| now.checked_sub(created_at)) {
        Some(age) if age <= PROVISIONING_GRACE_SECS => ServerPlayitState::Provisioning,
        _ => ServerPlayitState::Missing,
    }
}

fn classify_server_tunnel(
    binding: &PlayitBinding,
    expected_port: u16,
    tunnel: &PlayitTunnel,
    current_agent_id: &str,
) -> (ServerPlayitState, Option<String>) {
    if tunnel.disabled {
        return (
            ServerPlayitState::DisabledByPlayit,
            tunnel.disabled_reason.clone(),
        );
    }
    // The current-agent fallback list carries no agent or semantic-type
    // metadata; missing values mean "unknown", not "mismatched", because that
    // list is inherently scoped to the current agent.
    if tunnel
        .agent_id
        .as_deref()
        .is_some_and(|agent_id| agent_id != current_agent_id)
    {
        return (
            ServerPlayitState::AgentMismatch,
            Some("The Playit tunnel is not assigned to the current agent".into()),
        );
    }
    if tunnel
        .tunnel_type
        .as_deref()
        .is_some_and(|kind| kind != "minecraft-java")
    {
        return (
            ServerPlayitState::Drifted,
            Some("The Playit tunnel is not a Minecraft Java tunnel".into()),
        );
    }
    if tunnel.protocol != PlayitProtocol::Tcp {
        return (
            ServerPlayitState::Drifted,
            Some("The Playit tunnel protocol is not compatible with Minecraft Java".into()),
        );
    }
    if binding.local_port != expected_port {
        return (
            ServerPlayitState::Drifted,
            Some("The stored Playit destination differs from the server port".into()),
        );
    }
    if binding.local_address != "127.0.0.1" && binding.local_address != "::1" {
        return (
            ServerPlayitState::Drifted,
            Some("The stored Playit destination is not a safe loopback address".into()),
        );
    }
    if tunnel.local_address.as_deref() != Some(binding.local_address.as_str())
        || tunnel.local_port != Some(expected_port)
        || !destination_matches(tunnel, &binding.local_address, expected_port)
    {
        return (
            ServerPlayitState::Drifted,
            Some("The Playit destination is missing or differs from the server port".into()),
        );
    }

    (ServerPlayitState::Connected, None)
}

fn destination_matches(tunnel: &PlayitTunnel, address: &str, port: u16) -> bool {
    let expected = format!("{address}:{port}");
    tunnel.destination == expected
        || (address == "::1" && tunnel.destination == format!("[{address}]:{port}"))
}

async fn remove_cleanup(state: &AppState, server_id: &str, tunnel_id: &str) {
    if let Err(error) = state
        .store
        .update(|data| {
            data.playit_cleanup
                .retain(|cleanup| cleanup.server_id != server_id || cleanup.tunnel_id != tunnel_id);
        })
        .await
    {
        tracing::warn!(
            server = %server_id,
            tunnel = %tunnel_id,
            error = ?error,
            "Playit cleanup completed remotely but its queue entry could not be cleared"
        );
    }
}

async fn cleanup_pending_for_server(state: &AppState, server_id: &str) -> bool {
    state
        .store
        .read()
        .await
        .playit_cleanup
        .iter()
        .any(|cleanup| cleanup.server_id == server_id)
}

async fn retry_server_cleanup(state: &AppState, server_id: &str) {
    let pending: Vec<_> = state
        .store
        .read()
        .await
        .playit_cleanup
        .into_iter()
        .filter(|cleanup| cleanup.server_id == server_id)
        .collect();
    for cleanup in pending {
        let result = state.playit.delete_tunnel(&cleanup.tunnel_id).await;
        if result.is_ok() || result.as_ref().is_err_and(|error| error.is_not_found()) {
            remove_cleanup(state, server_id, &cleanup.tunnel_id).await;
        } else if let Err(error) = result {
            tracing::debug!(
                server = %server_id,
                tunnel = %cleanup.tunnel_id,
                error = ?error,
                "Playit cleanup retry did not complete"
            );
        }
    }
}

fn local_address(address: Option<String>) -> ApiResult<String> {
    let address = address.unwrap_or_else(|| "127.0.0.1".into());
    let address = address.trim();
    if address != "127.0.0.1" && address != "::1" {
        return Err(ApiError::BadRequest(
            "local_address must be 127.0.0.1 or ::1".into(),
        ));
    }
    Ok(address.into())
}

fn is_loopback_address(address: &str) -> bool {
    address == "127.0.0.1" || address == "::1"
}

fn tunnel_name(name: Option<String>) -> ApiResult<Option<String>> {
    let Some(name) = name else {
        return Ok(None);
    };
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > 100 {
        return Err(ApiError::BadRequest("tunnel name is too long".into()));
    }
    Ok(Some(name.into()))
}

/// POST `/api/playit/auth/login`.
///
/// Signs in to playit.gg directly with email + password. Only safe session
/// state is returned: the session key never leaves the server. Request
/// bodies are never logged.
async fn auth_login(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Json<AccountSessionState>> {
    let email = valid_email(&body.email)?;
    let password = valid_password(&body.password)?;
    Ok(Json(state.playit.auth_login(&email, &password).await?))
}

/// POST `/api/playit/auth/totp`.
async fn auth_totp(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
    Json(body): Json<TotpRequest>,
) -> ApiResult<Json<AccountSessionState>> {
    let code = valid_totp_code(&body.code)?;
    Ok(Json(state.playit.complete_totp(&code).await?))
}

/// GET `/api/playit/auth/session`.
async fn auth_session(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> Json<AccountSessionState> {
    Json(state.playit.auth_status().await)
}

/// POST `/api/playit/auth/validate`.
async fn auth_validate(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> ApiResult<Json<AccountSessionState>> {
    Ok(Json(state.playit.auth_validate().await?))
}

/// DELETE `/api/playit/auth/session`.
///
/// Logs out the playit.gg account session only. The agent keeps running and
/// tunnels are untouched; use `/playit/agent/disconnect` to remove the agent.
async fn auth_logout(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> ApiResult<Json<serde_json::Value>> {
    state.playit.auth_logout().await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST `/api/playit/setup/direct`.
///
/// Single-call browserless agent setup: the panel starts the machine claim,
/// approves it under the logged-in account, and waits for the agent to
/// connect. The claim code never leaves the server. An already-configured
/// agent is returned as-is.
async fn setup_direct(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
    Json(body): Json<DirectSetupRequest>,
) -> ApiResult<Json<DirectSetupResult>> {
    let name = valid_agent_name(body.name)?;
    Ok(Json(state.playit.setup_direct(name).await?))
}

/// GET `/api/playit/claim/details?code=...`.
async fn claim_details(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
    Query(query): Query<ClaimCodeQuery>,
) -> ApiResult<Json<ClaimDetailsInfo>> {
    let code = valid_claim_code(&query.code)?;
    Ok(Json(state.playit.claim_details(&code).await?))
}

/// POST `/api/playit/claim/approve`.
async fn approve_claim(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
    Json(body): Json<ApproveClaimRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let code = valid_claim_code(&body.code)?;
    let name = valid_agent_name(body.name)?;
    let agent_id = state.playit.approve_claim(&code, name).await?;
    Ok(Json(
        serde_json::json!({ "ok": true, "agent_id": agent_id }),
    ))
}

/// POST `/api/playit/claim/reject`.
async fn reject_claim(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
    Json(body): Json<RejectClaimRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let code = valid_claim_code(&body.code)?;
    state.playit.reject_claim(&code).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET `/api/playit/agents`.
async fn list_agents(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> ApiResult<Json<Vec<AgentInfo>>> {
    Ok(Json(state.playit.list_agents().await?))
}

/// DELETE `/api/playit/agents/:agent_id`.
///
/// The tunnel strategy is explicit: tunnels move to `move_to_agent` or are
/// unassigned (`null`), optionally disabled. Deleting the agent this panel
/// runs on is refused.
async fn delete_agent(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
    Path(agent_id): Path<String>,
    Json(body): Json<DeleteAgentRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let options = DeleteAgentOptions {
        move_to_agent: match body.move_to_agent {
            None => None,
            Some(target) if target.trim().is_empty() => None,
            Some(target) => Some(target.trim().to_owned()),
        },
        disable_tunnels: body.disable_tunnels.unwrap_or(false),
    };
    state.playit.delete_agent(agent_id.trim(), &options).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET `/api/playit/domains`.
async fn list_domains(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> ApiResult<Json<Vec<DomainInfo>>> {
    Ok(Json(state.playit.domains().await?))
}

/// POST `/api/playit/agent/disconnect`.
///
/// Removes the agent secret (and restarts the embedded runtime into
/// `WaitingForSecret`). The account session is untouched. This is distinct
/// from logging out.
async fn disconnect_agent(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> ApiResult<Json<PlayitStatus>> {
    Ok(Json(safe_status(state.playit.disconnect_agent().await?)))
}

/// POST `/api/playit/agent/reconnect`.
///
/// Restarts a stopped embedded runtime without touching the account session.
/// A running runtime is left alone. External mode verifies the daemon is
/// reachable.
async fn reconnect_agent(
    State(state): State<Arc<AppState>>,
    AdminIdentity(_admin): AdminIdentity,
) -> ApiResult<Json<PlayitStatus>> {
    Ok(Json(safe_status(state.playit.reconnect_agent().await?)))
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct TotpRequest {
    code: String,
}

#[derive(Debug, Deserialize)]
struct DirectSetupRequest {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaimCodeQuery {
    code: String,
}

#[derive(Debug, Deserialize)]
struct ApproveClaimRequest {
    code: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RejectClaimRequest {
    code: String,
}

#[derive(Debug, Deserialize)]
struct DeleteAgentRequest {
    #[serde(default)]
    move_to_agent: Option<String>,
    #[serde(default)]
    disable_tunnels: Option<bool>,
}

fn valid_email(email: &str) -> ApiResult<String> {
    let email = email.trim();
    if email.is_empty() || email.chars().count() > 320 {
        return Err(ApiError::BadRequest("email must not be empty".into()));
    }
    Ok(email.into())
}

fn valid_password(password: &str) -> ApiResult<String> {
    if password.is_empty() || password.len() > 4096 {
        return Err(ApiError::BadRequest("password must not be empty".into()));
    }
    Ok(password.into())
}

fn valid_totp_code(code: &str) -> ApiResult<String> {
    let code = code.trim().replace([' ', '-'], "");
    if !(6..=8).contains(&code.len()) || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::BadRequest(
            "totp code must be 6 to 8 digits".into(),
        ));
    }
    Ok(code)
}

fn valid_claim_code(code: &str) -> ApiResult<String> {
    let code = code.trim();
    if code.is_empty() || code.chars().count() > 256 {
        return Err(ApiError::BadRequest("claim code must not be empty".into()));
    }
    Ok(code.into())
}

fn valid_agent_name(name: Option<String>) -> ApiResult<Option<String>> {
    let Some(name) = name else {
        return Ok(None);
    };
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > 64 {
        return Err(ApiError::BadRequest(
            "agent name must be at most 64 characters".into(),
        ));
    }
    Ok(Some(name.into()))
}

/// Routes under `/api`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/playit/status", get(status))
        .route("/playit/account", get(account))
        .route("/playit/claim", post(claim))
        .route("/playit/auth/login", post(auth_login))
        .route("/playit/auth/totp", post(auth_totp))
        .route(
            "/playit/auth/session",
            get(auth_session).delete(auth_logout),
        )
        .route("/playit/auth/validate", post(auth_validate))
        .route("/playit/setup/direct", post(setup_direct))
        .route("/playit/claim/details", get(claim_details))
        .route("/playit/claim/approve", post(approve_claim))
        .route("/playit/claim/reject", post(reject_claim))
        .route("/playit/agents", get(list_agents))
        .route("/playit/agents/{agent_id}", delete(delete_agent))
        .route("/playit/domains", get(list_domains))
        .route("/playit/agent/disconnect", post(disconnect_agent))
        .route("/playit/agent/reconnect", post(reconnect_agent))
        .route("/playit/tunnels", get(list_tunnels).post(create_tunnel))
        .route("/playit/tunnels/{tunnel_id}", delete(delete_tunnel))
        .route(
            "/servers/{id}/playit",
            get(server_playit)
                .post(attach_server_playit)
                .delete(detach_server_playit),
        )
        .route("/servers/{id}/playit/forget", post(forget_server_playit))
        .route(
            "/servers/{id}/playit/reconcile",
            post(reconcile_server_playit),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_tunnels_are_restricted_to_loopback() {
        assert_eq!(local_address(None).unwrap(), "127.0.0.1");
        assert_eq!(local_address(Some(" ::1 ".into())).unwrap(), "::1");
        assert!(local_address(Some("192.168.1.10".into())).is_err());
    }

    #[test]
    fn blank_names_are_omitted_and_long_names_are_rejected() {
        assert_eq!(tunnel_name(Some("  ".into())).unwrap(), None);
        assert_eq!(
            tunnel_name(Some(" survival ".into())).unwrap(),
            Some("survival".into())
        );
        assert!(tunnel_name(Some("x".repeat(101))).is_err());
    }

    #[test]
    fn status_diagnostics_do_not_contain_integration_error_details() {
        let status = safe_status(PlayitStatus {
            status: PlayitConnectionState::Unavailable,
            version: None,
            message: Some("C:\\private\\playit\\secret.toml leaked".into()),
        });

        assert_eq!(
            status.message.as_deref(),
            Some("Playit service unavailable")
        );
        assert!(!status.message.as_deref().unwrap().contains("secret.toml"));
    }

    fn test_binding() -> PlayitBinding {
        PlayitBinding {
            tunnel_id: "tunnel-1".into(),
            protocol: PlayitProtocol::Tcp,
            local_address: "127.0.0.1".into(),
            local_port: 25565,
            agent_id: Some("agent-1".into()),
            created_at: Some(now_unix_seconds()),
        }
    }

    fn test_tunnel() -> PlayitTunnel {
        PlayitTunnel {
            id: "tunnel-1".into(),
            name: Some("mcpanel:server-1".into()),
            display_address: "example.playit.gg:25565".into(),
            destination: "127.0.0.1:25565".into(),
            protocol: PlayitProtocol::Tcp,
            tunnel_type: Some("minecraft-java".into()),
            agent_id: Some("agent-1".into()),
            local_address: Some("127.0.0.1".into()),
            local_port: Some(25565),
            disabled: false,
            disabled_reason: None,
        }
    }

    #[test]
    fn server_tunnel_is_connected_only_with_validated_destination_metadata() {
        let (state, message) =
            classify_server_tunnel(&test_binding(), 25565, &test_tunnel(), "agent-1");
        assert_eq!(state, ServerPlayitState::Connected);
        assert_eq!(message, None);
    }

    #[test]
    fn missing_or_wrong_destination_metadata_is_drifted() {
        for (address, port) in [
            (None, Some(25565)),
            (Some("127.0.0.2"), Some(25565)),
            (Some("127.0.0.1"), None),
            (Some("127.0.0.1"), Some(25566)),
        ] {
            let mut tunnel = test_tunnel();
            tunnel.local_address = address.map(str::to_owned);
            tunnel.local_port = port;
            assert_eq!(
                classify_server_tunnel(&test_binding(), 25565, &tunnel, "agent-1").0,
                ServerPlayitState::Drifted
            );
        }

        let mut tunnel = test_tunnel();
        tunnel.destination.clear();
        assert_eq!(
            classify_server_tunnel(&test_binding(), 25565, &tunnel, "agent-1").0,
            ServerPlayitState::Drifted
        );

        tunnel = test_tunnel();
        tunnel.destination = "10.0.0.5:25565".into();
        assert_eq!(
            classify_server_tunnel(&test_binding(), 25565, &tunnel, "agent-1").0,
            ServerPlayitState::Drifted
        );
    }

    #[test]
    fn disabled_tunnel_is_reported_with_the_playit_reason() {
        let mut tunnel = test_tunnel();
        tunnel.disabled = true;
        tunnel.disabled_reason = Some("account limit".into());

        let (state, message) = classify_server_tunnel(&test_binding(), 25565, &tunnel, "agent-1");
        assert_eq!(state, ServerPlayitState::DisabledByPlayit);
        assert_eq!(message.as_deref(), Some("account limit"));
    }

    #[test]
    fn wrong_type_protocol_and_agent_are_not_connected() {
        let mut tunnel = test_tunnel();
        tunnel.tunnel_type = Some("minecraft-bedrock".into());
        assert_eq!(
            classify_server_tunnel(&test_binding(), 25565, &tunnel, "agent-1").0,
            ServerPlayitState::Drifted
        );

        tunnel = test_tunnel();
        tunnel.protocol = PlayitProtocol::Udp;
        assert_eq!(
            classify_server_tunnel(&test_binding(), 25565, &tunnel, "agent-1").0,
            ServerPlayitState::Drifted
        );

        tunnel = test_tunnel();
        tunnel.agent_id = Some("agent-2".into());
        assert_eq!(
            classify_server_tunnel(&test_binding(), 25565, &tunnel, "agent-1").0,
            ServerPlayitState::AgentMismatch
        );
    }

    #[test]
    fn login_bodies_are_validated_before_any_network_call() {
        assert_eq!(
            valid_email("  user@example.com ").unwrap(),
            "user@example.com"
        );
        assert!(valid_email("").is_err());
        assert!(valid_email("   ").is_err());
        assert!(valid_email(&"x".repeat(321)).is_err());
        assert!(valid_password("secret").is_ok());
        assert!(valid_password("").is_err());
    }

    #[test]
    fn totp_codes_accept_formatting_but_stay_numeric() {
        assert_eq!(valid_totp_code("123456").unwrap(), "123456");
        assert_eq!(valid_totp_code(" 123 456 ").unwrap(), "123456");
        assert_eq!(valid_totp_code("123-456").unwrap(), "123456");
        assert_eq!(valid_totp_code("12345678").unwrap(), "12345678");
        assert!(valid_totp_code("12345").is_err());
        assert!(valid_totp_code("123456789").is_err());
        assert!(valid_totp_code("abcdef").is_err());
        assert!(valid_totp_code("").is_err());
    }

    #[test]
    fn claim_codes_and_agent_names_are_bounded() {
        assert_eq!(valid_claim_code("abc123").unwrap(), "abc123");
        assert!(valid_claim_code("").is_err());
        assert_eq!(valid_agent_name(None).unwrap(), None);
        assert_eq!(valid_agent_name(Some("  ".into())).unwrap(), None);
        assert_eq!(
            valid_agent_name(Some(" panel-1 ".into())).unwrap(),
            Some("panel-1".into())
        );
        assert!(valid_agent_name(Some("x".repeat(65))).is_err());
    }

    #[test]
    fn auth_state_serializes_the_safe_contract() {
        let state = AccountSessionState {
            authenticated: true,
            requires_totp: false,
            account_id: Some(123),
            account_status: Some("verified".into()),
            read_only: false,
        };
        let body = serde_json::to_value(&state).unwrap();
        assert_eq!(body["authenticated"], true);
        assert_eq!(body["requires_totp"], false);
        assert_eq!(body["account_id"], 123);
        assert_eq!(body["account_status"], "verified");
        assert_eq!(body["read_only"], false);
        assert!(body.get("session_key").is_none());
        assert!(body.get("password").is_none());
    }

    #[test]
    fn agent_delete_requests_default_to_unassign_without_disabling() {
        let body: DeleteAgentRequest = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(body.move_to_agent, None);
        assert_eq!(body.disable_tunnels, None);
        let body: DeleteAgentRequest = serde_json::from_value(
            serde_json::json!({ "move_to_agent": null, "disable_tunnels": true }),
        )
        .unwrap();
        assert_eq!(body.move_to_agent, None);
        assert_eq!(body.disable_tunnels, Some(true));
    }

    #[tokio::test]
    async fn new_routes_are_registered_behind_admin_auth() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let data_dir = tempfile::tempdir().unwrap();
        let state =
            crate::state::AppState::bootstrap(data_dir.path(), crate::state::PlayitMode::External)
                .await
                .unwrap();
        let app = axum::Router::new()
            .nest(
                "/api",
                super::super::router_with_limits(crate::limits::ResourceLimits::default()),
            )
            .with_state(state);

        // Every new Playit route must exist and require an admin session.
        // Without credentials each one answers 401 before touching any body.
        for (method, uri) in [
            ("POST", "/api/playit/auth/login"),
            ("POST", "/api/playit/auth/totp"),
            ("GET", "/api/playit/auth/session"),
            ("DELETE", "/api/playit/auth/session"),
            ("POST", "/api/playit/auth/validate"),
            ("POST", "/api/playit/setup/direct"),
            ("GET", "/api/playit/claim/details?code=abc"),
            ("POST", "/api/playit/claim/approve"),
            ("POST", "/api/playit/claim/reject"),
            ("GET", "/api/playit/agents"),
            (
                "DELETE",
                "/api/playit/agents/11111111-1111-1111-1111-111111111111",
            ),
            ("GET", "/api/playit/domains"),
            ("POST", "/api/playit/agent/disconnect"),
            ("POST", "/api/playit/agent/reconnect"),
            ("GET", "/api/playit/status"),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri}"
            );
        }
    }

    #[test]
    fn missing_tunnels_have_a_bounded_provisioning_window() {
        assert_eq!(
            missing_tunnel_state(Some(940), 1_000),
            ServerPlayitState::Provisioning
        );
        assert_eq!(
            missing_tunnel_state(Some(939), 1_000),
            ServerPlayitState::Missing
        );
        assert_eq!(
            missing_tunnel_state(None, 1_000),
            ServerPlayitState::Missing
        );
        assert_eq!(
            missing_tunnel_state(Some(1_001), 1_000),
            ServerPlayitState::Missing
        );
    }
}
