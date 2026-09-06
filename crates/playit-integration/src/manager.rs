//! High-level Playit operations used by the panel.

use std::path::PathBuf;
use std::sync::Arc;

use playit_ipc::model::{
    AccountResponse, AccountStatus, AccountTunnelListResponse, AgentLifecycle, ServicePhase,
    SubscribeResponse, TunnelProtocol,
};
use playit_runtime::{PlayitRuntime, RuntimeOptions};
use tokio::sync::Mutex;

use crate::client::{IpcPlayitService, PlayitService};
use crate::error::PlayitError;
use crate::model::{
    ClaimInfo, PlayitAccount, PlayitAccountStatus, PlayitConnectionState, PlayitProtocol,
    PlayitStatus, PlayitTunnel, TunnelCreateInfo,
};

/// The panel-facing Playit service facade.
///
/// External mode deliberately does not own a persistent IPC connection. A dead
/// socket can therefore only fail one operation instead of poisoning the panel
/// forever. Embedded mode owns one runtime shared by all manager clones.
#[derive(Clone)]
pub struct PlayitManager {
    service: Arc<dyn PlayitService>,
    runtime: Option<Arc<Mutex<Option<PlayitRuntime>>>>,
}

/// The provenance of a tunnel returned by [`PlayitManager::ensure_server_tunnel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureTunnelDisposition {
    /// The ensure operation created the tunnel, so it may be deleted as a
    /// compensation if the following local transaction fails.
    Created,
    /// An existing tunnel was used without changing its remote assignment.
    Reused,
    /// An existing tunnel was assigned to the current agent.
    Reassigned {
        /// The assignment observed before reassignment, if Playit supplied it.
        previous_agent_id: Option<String>,
    },
}

/// A tunnel returned by an ensure operation together with its remote
/// ownership history. Keeping these together prevents callers from treating a
/// reused tunnel as if it had just been created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsuredServerTunnel {
    /// The tunnel returned by Playit.
    pub tunnel: TunnelCreateInfo,
    /// How the tunnel was obtained.
    pub disposition: EnsureTunnelDisposition,
}

impl Default for PlayitManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayitManager {
    /// Construct a manager using the optional external Playit IPC service.
    ///
    /// This compatibility alias preserves the original integration API. New
    /// panel startup code should choose [`Self::embedded`] or [`Self::external`]
    /// explicitly.
    pub fn new() -> Self {
        Self::external()
    }

    /// Construct a manager using a direct, in-process Playit runtime.
    pub async fn embedded(secret_path: impl Into<PathBuf>) -> Result<Self, PlayitError> {
        let options = RuntimeOptions {
            secret_path: secret_path.into(),
            ..RuntimeOptions::default()
        };
        let (runtime, handle) = PlayitRuntime::start(options).await?;

        Ok(Self {
            service: Arc::new(crate::embedded::EmbeddedPlayitService::new(handle)),
            runtime: Some(Arc::new(Mutex::new(Some(runtime)))),
        })
    }

    /// Construct a manager using the separately managed external daemon.
    pub fn external() -> Self {
        Self {
            service: Arc::new(IpcPlayitService),
            runtime: None,
        }
    }

    /// Construct a manager whose operations report a startup failure.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::with_service(UnavailablePlayitService {
            message: message.into(),
        })
    }

    /// Construct a manager around an injected service implementation.
    ///
    /// This is primarily useful for tests and for future alternate transports.
    pub fn with_service<S>(service: S) -> Self
    where
        S: PlayitService + 'static,
    {
        Self {
            service: Arc::new(service),
            runtime: None,
        }
    }

    /// Stop the embedded runtime owned by this manager, if any.
    ///
    /// The runtime owner is stored behind a shared mutex so clones all observe
    /// the same one-shot shutdown. External mode intentionally does nothing;
    /// its daemon belongs to the operator's service manager.
    pub async fn shutdown(&self) -> Result<(), PlayitError> {
        let Some(runtime) = &self.runtime else {
            return Ok(());
        };

        let runtime = runtime.lock().await.take();
        if let Some(runtime) = runtime {
            runtime.shutdown().await?;
        }
        Ok(())
    }

    /// Read and normalize the Playit service's status and lifecycle.
    pub async fn status(&self) -> Result<PlayitStatus, PlayitError> {
        let snapshot: SubscribeResponse = self.service.snapshot().await?;
        let service_status = snapshot.snapshot.status;
        let lifecycle = snapshot.snapshot.lifecycle;

        let version = (!service_status.version.is_empty()).then_some(service_status.version);
        let (status, message) = match (&lifecycle, service_status.has_secret) {
            (AgentLifecycle::Running(_), false) => (PlayitConnectionState::NeedsClaim, None),
            _ => lifecycle_state(&service_status.phase, &lifecycle),
        };

        Ok(PlayitStatus {
            status,
            version,
            message,
        })
    }

    /// Convert an operation error into the status representation used by the
    /// non-failing status endpoint.
    pub fn status_from_error(error: &PlayitError) -> PlayitStatus {
        let status = if error.is_unsupported() {
            PlayitConnectionState::Unsupported
        } else if error.is_unavailable() {
            PlayitConnectionState::Unavailable
        } else {
            PlayitConnectionState::Error
        };

        PlayitStatus {
            status,
            version: None,
            message: Some(error.to_string()),
        }
    }

    /// Read account information without exposing its secret.
    pub async fn account(&self) -> Result<PlayitAccount, PlayitError> {
        let account = self.service.account().await?;
        Ok(account_view(account))
    }

    /// Start the browser-based Playit claim flow.
    pub async fn start_claim(&self) -> Result<ClaimInfo, PlayitError> {
        let claim = self.service.start_claim().await?;
        if claim.claim_url.trim().is_empty() {
            return Err(PlayitError::Protocol(
                "claim response did not contain a URL".into(),
            ));
        }
        Ok(ClaimInfo {
            claim_url: claim.claim_url,
        })
    }

    /// List currently materialized tunnels.
    pub async fn tunnels(&self) -> Result<Vec<PlayitTunnel>, PlayitError> {
        let response = self.service.list_tunnels().await?;
        Ok(response.tunnels.into_iter().map(tunnel_view).collect())
    }

    /// List every tunnel owned by the authenticated Playit account.
    pub async fn account_tunnels(&self) -> Result<Vec<PlayitTunnel>, PlayitError> {
        let response = self.service.list_account_tunnels().await?;
        Ok(response
            .tunnels
            .into_iter()
            .map(account_tunnel_view)
            .collect())
    }

    /// Create a tunnel and return its immediate identifier.
    pub async fn create_tunnel(
        &self,
        local_port: u16,
        protocol: PlayitProtocol,
        local_address: Option<String>,
        name: Option<String>,
    ) -> Result<TunnelCreateInfo, PlayitError> {
        let response = self
            .service
            .create_tunnel(local_port, protocol.into(), local_address, name)
            .await?;

        if response.tunnel_id.trim().is_empty() {
            return Err(PlayitError::Protocol(
                "tunnel creation response did not contain an id".into(),
            ));
        }

        Ok(TunnelCreateInfo {
            tunnel_id: response.tunnel_id,
            message: response.message,
        })
    }

    /// Create a semantic Minecraft Java tunnel for a local server.
    pub async fn create_minecraft_java_tunnel(
        &self,
        local_port: u16,
        local_address: Option<String>,
        name: Option<String>,
    ) -> Result<TunnelCreateInfo, PlayitError> {
        let response = self
            .service
            .create_minecraft_java_tunnel(local_port, local_address, name)
            .await?;

        if response.tunnel_id.trim().is_empty() {
            return Err(PlayitError::Protocol(
                "tunnel creation response did not contain an id".into(),
            ));
        }

        Ok(TunnelCreateInfo {
            tunnel_id: response.tunnel_id,
            message: response.message,
        })
    }

    /// Reassign a tunnel to this panel's current Playit agent.
    pub async fn reassign_tunnel(
        &self,
        tunnel_id: &str,
        local_port: u16,
        local_address: Option<String>,
    ) -> Result<(), PlayitError> {
        let response = self
            .service
            .reassign_tunnel(tunnel_id, local_port, local_address)
            .await?;
        if !response.accepted {
            return Err(PlayitError::Rejected(
                response
                    .message
                    .unwrap_or_else(|| "tunnel reassignment was not accepted".into()),
            ));
        }
        Ok(())
    }

    /// Reuse the stable panel-owned tunnel for a server, or one unique legacy
    /// tunnel with the same server name and destination. Reassign it to the
    /// current agent when needed, and create it only when it does not exist.
    pub async fn ensure_server_tunnel(
        &self,
        server_id: &str,
        server_name: &str,
        port: u16,
    ) -> Result<EnsuredServerTunnel, PlayitError> {
        let name = format!("mcpanel:{server_id}");
        let tunnels = self.account_tunnels().await?;
        let stable_matches: Vec<_> = tunnels
            .iter()
            .filter(|tunnel| tunnel.name.as_deref() == Some(name.as_str()))
            .cloned()
            .collect();

        if stable_matches.len() > 1 {
            return Err(PlayitError::Conflict(format!(
                "multiple managed tunnels are named {name}"
            )));
        }

        let existing = if let Some(existing) = stable_matches.into_iter().next() {
            Some(existing)
        } else {
            let legacy_matches: Vec<_> = tunnels
                .iter()
                .filter(|tunnel| tunnel.name.as_deref() == Some(server_name))
                .cloned()
                .collect();
            if legacy_matches.len() > 1 {
                return Err(PlayitError::Conflict(format!(
                    "multiple legacy Playit tunnels match server {server_name:?}"
                )));
            }
            legacy_matches.into_iter().next()
        };

        let account = self.account().await?;
        let current_agent_id = account
            .agent_id
            .as_deref()
            .filter(|agent_id| !agent_id.trim().is_empty())
            .ok_or_else(|| {
                PlayitError::Unavailable("the current Playit agent id is not available yet".into())
            })?;

        let Some(existing) = existing else {
            return Ok(EnsuredServerTunnel {
                tunnel: self
                    .create_minecraft_java_tunnel(port, Some("127.0.0.1".into()), Some(name))
                    .await?,
                disposition: EnsureTunnelDisposition::Created,
            });
        };

        if existing.tunnel_type.as_deref() != Some("minecraft-java") {
            return Err(PlayitError::Conflict(format!(
                "Playit tunnel {} exists for this server but is not a Minecraft Java tunnel",
                existing.id
            )));
        }

        if existing.disabled {
            return Err(PlayitError::Conflict(format!(
                "Playit disabled managed tunnel {}{}",
                existing.id,
                existing
                    .disabled_reason
                    .as_deref()
                    .map(|reason| format!(": {reason}"))
                    .unwrap_or_default()
            )));
        }

        if existing.protocol != PlayitProtocol::Tcp {
            return Err(PlayitError::Conflict(format!(
                "managed tunnel {} does not use the Minecraft Java TCP protocol",
                existing.id
            )));
        }

        if existing.local_address.as_deref() != Some("127.0.0.1")
            || existing.local_port != Some(port)
        {
            return Err(PlayitError::Conflict(format!(
                "managed tunnel {} points to {} but this server requires 127.0.0.1:{port}",
                existing.id,
                existing
                    .local_address
                    .as_deref()
                    .zip(existing.local_port)
                    .map(|(address, port)| format!("{address}:{port}"))
                    .unwrap_or_else(|| "an unknown destination".into())
            )));
        }

        if existing.agent_id.as_deref() != Some(current_agent_id) {
            self.reassign_tunnel(&existing.id, port, Some("127.0.0.1".into()))
                .await?;
            return Ok(EnsuredServerTunnel {
                tunnel: TunnelCreateInfo {
                    tunnel_id: existing.id,
                    message: Some("Existing Minecraft Java tunnel reassigned".into()),
                },
                disposition: EnsureTunnelDisposition::Reassigned {
                    previous_agent_id: existing.agent_id,
                },
            });
        }

        Ok(EnsuredServerTunnel {
            tunnel: TunnelCreateInfo {
                tunnel_id: existing.id,
                message: Some("Existing Minecraft Java tunnel reused".into()),
            },
            disposition: EnsureTunnelDisposition::Reused,
        })
    }

    /// Apply the only safe compensation available after an ensure operation
    /// has succeeded but the panel's local transaction failed.
    ///
    /// Playit's pinned reassign command can only target the current agent; it
    /// cannot restore an arbitrary previous agent. Reassigned tunnels are
    /// therefore deliberately preserved and a reconciliation warning is
    /// emitted instead of risking destruction of a pre-existing tunnel.
    pub async fn compensate_ensure_failure(
        &self,
        ensured: &EnsuredServerTunnel,
    ) -> Result<(), PlayitError> {
        match &ensured.disposition {
            EnsureTunnelDisposition::Created => {
                let result = self.delete_tunnel(&ensured.tunnel.tunnel_id).await;
                if let Err(error) = result {
                    if !error.is_not_found() {
                        tracing::warn!(
                            tunnel = %ensured.tunnel.tunnel_id,
                            error = %error,
                            "failed to delete newly-created Playit tunnel after local persistence failure"
                        );
                        return Err(error);
                    }
                }
            }
            EnsureTunnelDisposition::Reused => {
                tracing::debug!(
                    tunnel = %ensured.tunnel.tunnel_id,
                    "preserving reused Playit tunnel after local persistence failure"
                );
            }
            EnsureTunnelDisposition::Reassigned { previous_agent_id } => {
                tracing::warn!(
                    tunnel = %ensured.tunnel.tunnel_id,
                    previous_agent_id = ?previous_agent_id,
                    "preserving reassigned Playit tunnel; previous assignment cannot be restored by the pinned API and reconciliation may be required"
                );
            }
        }
        Ok(())
    }

    /// Delete a tunnel by its stable Playit id.
    pub async fn delete_tunnel(&self, tunnel_id: &str) -> Result<(), PlayitError> {
        let response = self.service.delete_tunnel(tunnel_id).await?;
        if !response.accepted {
            return Err(PlayitError::Rejected(
                response
                    .message
                    .unwrap_or_else(|| "delete command was not accepted".into()),
            ));
        }
        Ok(())
    }
}

struct UnavailablePlayitService {
    message: String,
}

#[async_trait::async_trait]
impl PlayitService for UnavailablePlayitService {
    async fn status(&self) -> Result<playit_ipc::model::ServiceStatus, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn lifecycle(&self) -> Result<AgentLifecycle, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn snapshot(&self) -> Result<SubscribeResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn account(&self) -> Result<AccountResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn start_claim(&self) -> Result<playit_ipc::model::ClaimResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn list_tunnels(&self) -> Result<playit_ipc::model::TunnelListResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn list_account_tunnels(&self) -> Result<AccountTunnelListResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn create_tunnel(
        &self,
        _: u16,
        _: TunnelProtocol,
        _: Option<String>,
        _: Option<String>,
    ) -> Result<playit_ipc::model::TunnelCreateResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn create_minecraft_java_tunnel(
        &self,
        _: u16,
        _: Option<String>,
        _: Option<String>,
    ) -> Result<playit_ipc::model::TunnelCreateResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn delete_tunnel(
        &self,
        _: &str,
    ) -> Result<playit_ipc::model::CommandResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn reassign_tunnel(
        &self,
        _: &str,
        _: u16,
        _: Option<String>,
    ) -> Result<playit_ipc::model::CommandResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }
}

fn account_view(account: AccountResponse) -> PlayitAccount {
    PlayitAccount {
        status: match account.status {
            AccountStatus::Unknown => PlayitAccountStatus::Unknown,
            AccountStatus::Guest => PlayitAccountStatus::Guest,
            AccountStatus::EmailNotVerified => PlayitAccountStatus::EmailNotVerified,
            AccountStatus::Verified => PlayitAccountStatus::Verified,
        },
        agent_id: account.agent_id,
        login_link: account.login_link,
        claim_url: account.claim_url,
    }
}

fn tunnel_view(tunnel: playit_ipc::model::TunnelState) -> PlayitTunnel {
    PlayitTunnel {
        id: tunnel.id,
        name: tunnel.name,
        display_address: tunnel.display_address,
        destination: tunnel.destination,
        protocol: tunnel.protocol.into(),
        tunnel_type: None,
        agent_id: None,
        local_address: tunnel.local_address,
        local_port: tunnel.local_port,
        disabled: tunnel.is_disabled,
        disabled_reason: tunnel.disabled_reason,
    }
}

fn account_tunnel_view(tunnel: playit_ipc::model::AccountTunnelState) -> PlayitTunnel {
    PlayitTunnel {
        id: tunnel.id,
        name: tunnel.name,
        display_address: tunnel.display_address,
        destination: tunnel.destination,
        protocol: tunnel.protocol.into(),
        tunnel_type: tunnel.tunnel_type,
        agent_id: tunnel.agent_id,
        local_address: tunnel.local_address,
        local_port: tunnel.local_port,
        disabled: tunnel.is_disabled,
        disabled_reason: tunnel.disabled_reason,
    }
}

fn lifecycle_state(
    phase: &ServicePhase,
    lifecycle: &AgentLifecycle,
) -> (PlayitConnectionState, Option<String>) {
    match lifecycle {
        AgentLifecycle::WaitingForSecret => (PlayitConnectionState::NeedsClaim, None),
        AgentLifecycle::HasInvalidSecret(error)
        | AgentLifecycle::DisabledOverLimit(error)
        | AgentLifecycle::Error(error) => {
            (PlayitConnectionState::Error, Some(error.message.clone()))
        }
        AgentLifecycle::Starting => match phase {
            ServicePhase::Reconnecting => (PlayitConnectionState::Reconnecting, None),
            _ => (PlayitConnectionState::Starting, None),
        },
        AgentLifecycle::Stopping => (PlayitConnectionState::Stopping, None),
        AgentLifecycle::Running(_) => match phase {
            ServicePhase::WaitingForSecret => (PlayitConnectionState::NeedsClaim, None),
            ServicePhase::Starting => (PlayitConnectionState::Starting, None),
            ServicePhase::Stopping => (PlayitConnectionState::Stopping, None),
            ServicePhase::HasInvalidSecret
            | ServicePhase::DisabledOverLimit
            | ServicePhase::Error => (PlayitConnectionState::Error, phase_error_message(phase)),
            ServicePhase::Reconnecting => (PlayitConnectionState::Reconnecting, None),
            ServicePhase::Running => (PlayitConnectionState::Connected, None),
        },
    }
}

fn phase_error_message(phase: &ServicePhase) -> Option<String> {
    let message = match phase {
        ServicePhase::HasInvalidSecret => "Playit has an invalid account secret",
        ServicePhase::DisabledOverLimit => "Playit disabled the agent over its limit",
        ServicePhase::Error => "Playit reported an error",
        ServicePhase::WaitingForSecret
        | ServicePhase::Starting
        | ServicePhase::Reconnecting
        | ServicePhase::Running
        | ServicePhase::Stopping => return None,
    };
    Some(message.into())
}

impl From<PlayitProtocol> for TunnelProtocol {
    fn from(protocol: PlayitProtocol) -> Self {
        match protocol {
            PlayitProtocol::Tcp => Self::Tcp,
            PlayitProtocol::Udp => Self::Udp,
            PlayitProtocol::Both => Self::Both,
        }
    }
}

impl From<TunnelProtocol> for PlayitProtocol {
    fn from(protocol: TunnelProtocol) -> Self {
        match protocol {
            TunnelProtocol::Tcp => Self::Tcp,
            TunnelProtocol::Udp => Self::Udp,
            TunnelProtocol::Both => Self::Both,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use playit_ipc::model::{
        AccountTunnelListResponse, AccountTunnelState, AgentLifecycle, ClaimResponse,
        CommandResponse, ProtocolInfo, ServiceError, ServiceStatus, SubscribeResponse,
        SubscriptionSnapshot, TunnelCreateResponse, TunnelListResponse,
    };
    use std::sync::{Arc, Mutex};

    type CreatedTunnel = (u16, TunnelProtocol, Option<String>, Option<String>);

    #[derive(Default)]
    struct MockService {
        status: Mutex<ServiceStatus>,
        lifecycle: Mutex<AgentLifecycle>,
        account: Mutex<AccountResponse>,
        claim: Mutex<ClaimResponse>,
        tunnels: Mutex<TunnelListResponse>,
        account_tunnels: Mutex<AccountTunnelListResponse>,
        created: Mutex<Vec<CreatedTunnel>>,
        snapshot_calls: Arc<Mutex<usize>>,
        status_reads: Arc<Mutex<usize>>,
        lifecycle_reads: Arc<Mutex<usize>>,
        deleted: Arc<Mutex<Vec<String>>>,
        reassigned: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl PlayitService for MockService {
        async fn status(&self) -> Result<ServiceStatus, PlayitError> {
            *self.status_reads.lock().unwrap() += 1;
            Ok(self.status.lock().unwrap().clone())
        }

        async fn lifecycle(&self) -> Result<AgentLifecycle, PlayitError> {
            *self.lifecycle_reads.lock().unwrap() += 1;
            Ok(self.lifecycle.lock().unwrap().clone())
        }

        async fn snapshot(&self) -> Result<SubscribeResponse, PlayitError> {
            *self.snapshot_calls.lock().unwrap() += 1;
            let status = self.status.lock().unwrap().clone();
            let lifecycle = self.lifecycle.lock().unwrap().clone();
            Ok(SubscribeResponse {
                protocol: status.protocol.clone(),
                snapshot: SubscriptionSnapshot {
                    status,
                    lifecycle,
                    ..SubscriptionSnapshot::default()
                },
            })
        }

        async fn account(&self) -> Result<AccountResponse, PlayitError> {
            Ok(self.account.lock().unwrap().clone())
        }

        async fn start_claim(&self) -> Result<ClaimResponse, PlayitError> {
            Ok(self.claim.lock().unwrap().clone())
        }

        async fn list_tunnels(&self) -> Result<TunnelListResponse, PlayitError> {
            Ok(self.tunnels.lock().unwrap().clone())
        }

        async fn list_account_tunnels(&self) -> Result<AccountTunnelListResponse, PlayitError> {
            Ok(self.account_tunnels.lock().unwrap().clone())
        }

        async fn create_tunnel(
            &self,
            local_port: u16,
            protocol: TunnelProtocol,
            local_address: Option<String>,
            name: Option<String>,
        ) -> Result<TunnelCreateResponse, PlayitError> {
            self.created
                .lock()
                .unwrap()
                .push((local_port, protocol, local_address, name));
            Ok(TunnelCreateResponse {
                tunnel_id: "generic-tunnel".into(),
                message: None,
            })
        }

        async fn create_minecraft_java_tunnel(
            &self,
            local_port: u16,
            local_address: Option<String>,
            name: Option<String>,
        ) -> Result<TunnelCreateResponse, PlayitError> {
            self.created.lock().unwrap().push((
                local_port,
                TunnelProtocol::Tcp,
                local_address,
                name,
            ));
            Ok(TunnelCreateResponse {
                tunnel_id: "tunnel-1".into(),
                message: None,
            })
        }

        async fn delete_tunnel(&self, tunnel_id: &str) -> Result<CommandResponse, PlayitError> {
            self.deleted.lock().unwrap().push(tunnel_id.into());
            Ok(CommandResponse {
                accepted: true,
                message: None,
            })
        }

        async fn reassign_tunnel(
            &self,
            tunnel_id: &str,
            _: u16,
            _: Option<String>,
        ) -> Result<CommandResponse, PlayitError> {
            self.reassigned.lock().unwrap().push(tunnel_id.into());
            Ok(CommandResponse {
                accepted: true,
                message: None,
            })
        }
    }

    fn running_service() -> MockService {
        MockService {
            status: Mutex::new(ServiceStatus {
                phase: ServicePhase::Running,
                version: "1.2.3".into(),
                has_secret: true,
                protocol: ProtocolInfo {
                    ipc_version: playit_ipc::ipc::IPC_VERSION,
                    ..ProtocolInfo::default()
                },
                ..ServiceStatus::default()
            }),
            lifecycle: Mutex::new(AgentLifecycle::Running(Default::default())),
            ..MockService::default()
        }
    }

    #[tokio::test]
    async fn running_service_is_connected() {
        let service = running_service();
        let snapshot_calls = Arc::clone(&service.snapshot_calls);
        let status_reads = Arc::clone(&service.status_reads);
        let lifecycle_reads = Arc::clone(&service.lifecycle_reads);
        let manager = PlayitManager::with_service(service);
        let status = manager.status().await.unwrap();

        assert_eq!(status.status, PlayitConnectionState::Connected);
        assert_eq!(status.version.as_deref(), Some("1.2.3"));
        assert_eq!(*snapshot_calls.lock().unwrap(), 1);
        assert_eq!(*status_reads.lock().unwrap(), 0);
        assert_eq!(*lifecycle_reads.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn waiting_for_secret_needs_claim() {
        let service = MockService {
            status: Mutex::new(ServiceStatus {
                phase: ServicePhase::WaitingForSecret,
                ..ServiceStatus::default()
            }),
            lifecycle: Mutex::new(AgentLifecycle::WaitingForSecret),
            ..MockService::default()
        };
        let manager = PlayitManager::with_service(service);

        assert_eq!(
            manager.status().await.unwrap().status,
            PlayitConnectionState::NeedsClaim
        );
    }

    #[tokio::test]
    async fn starting_and_stopping_states_are_preserved_without_a_secret() {
        for (lifecycle, phase, expected) in [
            (
                AgentLifecycle::Starting,
                ServicePhase::Starting,
                PlayitConnectionState::Starting,
            ),
            (
                AgentLifecycle::Starting,
                ServicePhase::Reconnecting,
                PlayitConnectionState::Reconnecting,
            ),
            (
                AgentLifecycle::Stopping,
                ServicePhase::Stopping,
                PlayitConnectionState::Stopping,
            ),
        ] {
            let service = MockService {
                status: Mutex::new(ServiceStatus {
                    phase,
                    has_secret: false,
                    ..ServiceStatus::default()
                }),
                lifecycle: Mutex::new(lifecycle),
                ..MockService::default()
            };
            let manager = PlayitManager::with_service(service);

            assert_eq!(manager.status().await.unwrap().status, expected);
        }
    }

    #[tokio::test]
    async fn reconnect_and_failure_lifecycle_states_are_normalized() {
        let service_error = || ServiceError {
            message: "Playit setup failed".into(),
            ..ServiceError::default()
        };
        let cases = [
            (
                AgentLifecycle::Running(Default::default()),
                ServicePhase::Running,
                PlayitConnectionState::Connected,
            ),
            (
                AgentLifecycle::Running(Default::default()),
                ServicePhase::Reconnecting,
                PlayitConnectionState::Reconnecting,
            ),
            (
                AgentLifecycle::Starting,
                ServicePhase::Starting,
                PlayitConnectionState::Starting,
            ),
            (
                AgentLifecycle::WaitingForSecret,
                ServicePhase::WaitingForSecret,
                PlayitConnectionState::NeedsClaim,
            ),
            (
                AgentLifecycle::HasInvalidSecret(service_error()),
                ServicePhase::HasInvalidSecret,
                PlayitConnectionState::Error,
            ),
            (
                AgentLifecycle::Stopping,
                ServicePhase::Stopping,
                PlayitConnectionState::Stopping,
            ),
            (
                AgentLifecycle::Error(service_error()),
                ServicePhase::Error,
                PlayitConnectionState::Error,
            ),
        ];

        for (lifecycle, phase, expected) in cases {
            let service = MockService {
                status: Mutex::new(ServiceStatus {
                    phase,
                    has_secret: true,
                    ..ServiceStatus::default()
                }),
                lifecycle: Mutex::new(lifecycle),
                ..MockService::default()
            };
            let manager = PlayitManager::with_service(service);

            assert_eq!(manager.status().await.unwrap().status, expected);
        }
    }

    #[test]
    fn protocol_errors_are_reported_as_unsupported_status() {
        let error = PlayitError::from(playit_ipc::ipc::IpcError::ProtocolMismatch {
            expected: 2,
            actual: 1,
        });

        assert_eq!(
            PlayitManager::status_from_error(&error).status,
            PlayitConnectionState::Unsupported
        );
    }

    #[tokio::test]
    async fn ensure_server_tunnel_reuses_matching_account_tunnel() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        service
            .account_tunnels
            .lock()
            .unwrap()
            .tunnels
            .push(AccountTunnelState {
                id: "existing-tunnel".into(),
                name: Some("mcpanel:server-1".into()),
                tunnel_type: Some("minecraft-java".into()),
                local_address: Some("127.0.0.1".into()),
                local_port: Some(25565),
                agent_id: Some("agent-1".into()),
                ..AccountTunnelState::default()
            });
        let manager = PlayitManager::with_service(service);

        let created = manager
            .ensure_server_tunnel("server-1", "Survival SMP", 25565)
            .await
            .unwrap();

        assert_eq!(created.tunnel.tunnel_id, "existing-tunnel");
        assert_eq!(
            created.tunnel.message.as_deref(),
            Some("Existing Minecraft Java tunnel reused")
        );
        assert_eq!(created.disposition, EnsureTunnelDisposition::Reused);
    }

    #[tokio::test]
    async fn ensure_server_tunnel_imports_one_matching_legacy_tunnel() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        service
            .account_tunnels
            .lock()
            .unwrap()
            .tunnels
            .push(AccountTunnelState {
                id: "legacy-tunnel".into(),
                name: Some("Survival SMP".into()),
                tunnel_type: Some("minecraft-java".into()),
                local_address: Some("127.0.0.1".into()),
                local_port: Some(25565),
                agent_id: Some("agent-1".into()),
                ..AccountTunnelState::default()
            });
        let manager = PlayitManager::with_service(service);

        let created = manager
            .ensure_server_tunnel("server-1", "Survival SMP", 25565)
            .await
            .unwrap();

        assert_eq!(created.tunnel.tunnel_id, "legacy-tunnel");
        assert_eq!(
            created.tunnel.message.as_deref(),
            Some("Existing Minecraft Java tunnel reused")
        );
        assert_eq!(created.disposition, EnsureTunnelDisposition::Reused);
    }

    #[tokio::test]
    async fn ensure_server_tunnel_reassigns_matching_tunnel_from_another_agent() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        service
            .account_tunnels
            .lock()
            .unwrap()
            .tunnels
            .push(AccountTunnelState {
                id: "existing-tunnel".into(),
                name: Some("mcpanel:server-1".into()),
                tunnel_type: Some("minecraft-java".into()),
                local_address: Some("127.0.0.1".into()),
                local_port: Some(25565),
                agent_id: Some("agent-2".into()),
                ..AccountTunnelState::default()
            });
        let manager = PlayitManager::with_service(service);

        let created = manager
            .ensure_server_tunnel("server-1", "Survival SMP", 25565)
            .await
            .unwrap();

        assert_eq!(created.tunnel.tunnel_id, "existing-tunnel");
        assert_eq!(
            created.tunnel.message.as_deref(),
            Some("Existing Minecraft Java tunnel reassigned")
        );
        assert_eq!(
            created.disposition,
            EnsureTunnelDisposition::Reassigned {
                previous_agent_id: Some("agent-2".into()),
            }
        );
    }

    fn managed_tunnel(name: &str, id: &str, agent_id: Option<&str>) -> AccountTunnelState {
        AccountTunnelState {
            id: id.into(),
            name: Some(name.into()),
            tunnel_type: Some("minecraft-java".into()),
            protocol: TunnelProtocol::Tcp,
            local_address: Some("127.0.0.1".into()),
            local_port: Some(25565),
            agent_id: agent_id.map(str::to_owned),
            ..AccountTunnelState::default()
        }
    }

    #[tokio::test]
    async fn ensure_reports_ambiguous_stable_names_instead_of_picking_one() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        service.account_tunnels.lock().unwrap().tunnels.extend([
            managed_tunnel("mcpanel:server-1", "tunnel-1", Some("agent-1")),
            managed_tunnel("mcpanel:server-1", "tunnel-2", Some("agent-1")),
        ]);
        let manager = PlayitManager::with_service(service);

        assert!(matches!(
            manager
                .ensure_server_tunnel("server-1", "Survival SMP", 25565)
                .await,
            Err(PlayitError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn ensure_reports_ambiguous_legacy_names_instead_of_picking_one() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        service.account_tunnels.lock().unwrap().tunnels.extend([
            managed_tunnel("Survival SMP", "tunnel-1", Some("agent-1")),
            managed_tunnel("Survival SMP", "tunnel-2", Some("agent-1")),
        ]);
        let manager = PlayitManager::with_service(service);

        assert!(matches!(
            manager
                .ensure_server_tunnel("server-1", "Survival SMP", 25565)
                .await,
            Err(PlayitError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn ensure_requires_a_real_current_agent_id() {
        let service = running_service();
        service
            .account_tunnels
            .lock()
            .unwrap()
            .tunnels
            .push(managed_tunnel("mcpanel:server-1", "tunnel-1", None));
        let manager = PlayitManager::with_service(service);

        assert!(matches!(
            manager
                .ensure_server_tunnel("server-1", "Survival SMP", 25565)
                .await,
            Err(PlayitError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn disabled_managed_tunnel_is_a_conflict_not_a_reuse() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        let mut tunnel = managed_tunnel("mcpanel:server-1", "tunnel-1", Some("agent-1"));
        tunnel.is_disabled = true;
        tunnel.disabled_reason = Some("account limit".into());
        service.account_tunnels.lock().unwrap().tunnels.push(tunnel);
        let manager = PlayitManager::with_service(service);

        assert!(matches!(
            manager
                .ensure_server_tunnel("server-1", "Survival SMP", 25565)
                .await,
            Err(PlayitError::Conflict(message)) if message.contains("account limit")
        ));
    }

    #[tokio::test]
    async fn compensation_deletes_only_a_newly_created_tunnel() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        let deleted = Arc::clone(&service.deleted);
        let manager = PlayitManager::with_service(service);
        let ensured = manager
            .ensure_server_tunnel("server-1", "Survival SMP", 25565)
            .await
            .unwrap();

        assert_eq!(ensured.disposition, EnsureTunnelDisposition::Created);
        manager.compensate_ensure_failure(&ensured).await.unwrap();
        assert_eq!(*deleted.lock().unwrap(), vec!["tunnel-1"]);
    }

    #[tokio::test]
    async fn compensation_preserves_reused_and_reassigned_tunnels() {
        for previous_agent in [Some("agent-1"), Some("agent-2")] {
            let service = running_service();
            service.account.lock().unwrap().agent_id = Some("agent-1".into());
            let deleted = Arc::clone(&service.deleted);
            let reassigned = Arc::clone(&service.reassigned);
            service
                .account_tunnels
                .lock()
                .unwrap()
                .tunnels
                .push(managed_tunnel(
                    "mcpanel:server-1",
                    "tunnel-1",
                    previous_agent,
                ));
            let manager = PlayitManager::with_service(service);
            let ensured = manager
                .ensure_server_tunnel("server-1", "Survival SMP", 25565)
                .await
                .unwrap();
            manager.compensate_ensure_failure(&ensured).await.unwrap();

            assert!(deleted.lock().unwrap().is_empty());
            if previous_agent == Some("agent-2") {
                assert_eq!(*reassigned.lock().unwrap(), vec!["tunnel-1"]);
            } else {
                assert!(reassigned.lock().unwrap().is_empty());
            }
        }
    }

    #[test]
    fn runtime_stopped_is_unavailable() {
        let error = PlayitError::from(playit_runtime::RuntimeError::Stopped);

        assert!(error.is_unavailable());
        assert_eq!(
            PlayitManager::status_from_error(&error).status,
            PlayitConnectionState::Unavailable
        );
    }

    #[test]
    fn runtime_api_unavailable_is_unavailable_but_business_errors_are_not() {
        let unavailable = PlayitError::from(playit_runtime::RuntimeError::Api {
            code: playit_ipc::model::ServiceErrorCode::ApiUnavailable,
            message: "not ready".into(),
            retryable: true,
            details: None,
        });
        let rejected = PlayitError::from(playit_runtime::RuntimeError::InvalidState {
            code: playit_ipc::model::ServiceErrorCode::InvalidTunnelRequest,
            message: "bad tunnel".into(),
            retryable: false,
            details: None,
        });

        assert!(unavailable.is_unavailable());
        assert!(!rejected.is_unavailable());
    }

    #[tokio::test]
    async fn unavailable_backend_reports_the_startup_message() {
        let manager = PlayitManager::unavailable("embedded startup failed");
        let error = manager.status().await.unwrap_err();

        assert!(error.is_unavailable());
        assert!(error.to_string().contains("embedded startup failed"));
    }

    #[tokio::test]
    async fn embedded_shutdown_is_idempotent_and_shared_by_clones() {
        let secret_path = std::env::temp_dir().join(format!(
            "mcpanel-manager-shutdown-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let manager = PlayitManager::embedded(secret_path.clone()).await.unwrap();
        let clone = manager.clone();

        clone.shutdown().await.unwrap();
        manager.shutdown().await.unwrap();

        assert!(matches!(
            manager.account().await,
            Err(PlayitError::Runtime(playit_runtime::RuntimeError::Stopped))
        ));
        let _ = tokio::fs::remove_file(secret_path).await;
    }

    #[tokio::test]
    async fn external_shutdown_is_a_no_op() {
        PlayitManager::external().shutdown().await.unwrap();
    }
}
