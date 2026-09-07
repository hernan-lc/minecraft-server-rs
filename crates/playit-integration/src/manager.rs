//! High-level Playit operations used by the panel.
//!
//! The manager has two logical sides sharing one owner:
//!
//! * the **agent side** talks to the embedded runtime (or an external daemon)
//!   for status, lifecycle, claims, and tunnel runtime operations;
//! * the **account side** ([`AccountController`]) talks to `api.playit.gg`
//!   directly with a Bearer account session for login, claims, agents,
//!   domains, and account-wide tunnels.
//!
//! The account session and the agent secret have independent lifecycles:
//! expiring or logging out the account session never stops tunnels, and
//! resetting the agent secret never needs an account session.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use playit_ipc::model::{
    AccountResponse, AccountStatus, AccountTunnelListResponse, AgentLifecycle, ServicePhase,
    SubscribeResponse, TunnelProtocol,
};
use playit_runtime::{PlayitRuntime, RuntimeOptions};
use tokio::sync::{Mutex, RwLock};

use crate::account::{session_path_for_secret, AccountController, DEFAULT_API_BASE};
use crate::client::{IpcPlayitService, PlayitService};
use crate::error::PlayitError;
use crate::model::{
    AccountSessionState, AgentInfo, ClaimDetailsInfo, ClaimInfo, DeleteAgentOptions,
    DirectSetupResult, DomainInfo, PlayitAccount, PlayitAccountStatus, PlayitConnectionState,
    PlayitProtocol, PlayitStatus, PlayitTunnel, TunnelCreateInfo,
};

/// Options for the account side of [`PlayitManager`].
#[derive(Debug, Clone)]
pub struct PlayitOptions {
    /// playit.gg API base URL for direct account operations.
    pub api_base: String,
    /// File the Bearer account session is persisted to.
    pub session_path: PathBuf,
}

impl PlayitOptions {
    /// Options with the default API base and an explicit session path.
    pub fn new(session_path: impl Into<PathBuf>) -> Self {
        Self {
            api_base: DEFAULT_API_BASE.into(),
            session_path: session_path.into(),
        }
    }
}

/// Parameters the manager needs to restart its embedded runtime after a
/// secret reset.
#[derive(Debug, Clone)]
struct EmbeddedParams {
    secret_path: PathBuf,
    api_base: String,
}

/// How long [`PlayitManager::setup_direct`] waits for the claim handshake
/// and the agent lifecycle.
#[derive(Debug, Clone, Copy)]
pub struct SetupDirectOptions {
    /// How long to wait for the account side to see the pending claim.
    pub details_timeout: Duration,
    /// How long to wait for the agent lifecycle to reach Running after the
    /// claim was accepted.
    pub running_timeout: Duration,
    /// Poll interval for both waits.
    pub poll_interval: Duration,
}

impl Default for SetupDirectOptions {
    fn default() -> Self {
        Self {
            details_timeout: Duration::from_secs(15),
            running_timeout: Duration::from_secs(90),
            poll_interval: Duration::from_millis(500),
        }
    }
}

/// The panel-facing Playit service facade.
///
/// External mode deliberately does not own a persistent IPC connection. A dead
/// socket can therefore only fail one operation instead of poisoning the panel
/// forever. Embedded mode owns one runtime shared by all manager clones; the
/// service handle is swappable so a secret reset can restart the runtime
/// into `WaitingForSecret` without rebuilding the panel state.
#[derive(Clone)]
pub struct PlayitManager {
    service: Arc<RwLock<Arc<dyn PlayitService>>>,
    runtime: Option<Arc<Mutex<Option<PlayitRuntime>>>>,
    embedded: Option<EmbeddedParams>,
    account: AccountController,
}

/// The provenance of a tunnel returned by [`PlayitManager::ensure_server_tunnel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureTunnelDisposition {
    /// The ensure operation created the tunnel, so it may be deleted as a
    /// compensation if the following local transaction fails.
    Created,
    /// An existing tunnel was used without changing its remote assignment.
    Reused,
    /// An existing tunnel was updated in place (destination change and/or
    /// reassignment to the current agent). The tunnel id is preserved.
    Updated {
        /// The assignment observed before the update, if Playit supplied it.
        previous_agent_id: Option<String>,
        /// Whether the local destination was changed by the update.
        destination_changed: bool,
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
    ///
    /// The account session is persisted next to the secret and restored
    /// best-effort on later calls; the default playit.gg API base is used.
    pub async fn embedded(secret_path: impl Into<PathBuf>) -> Result<Self, PlayitError> {
        let secret_path = secret_path.into();
        let session_path = session_path_for_secret(&secret_path);
        Self::embedded_with_options(secret_path, PlayitOptions::new(session_path)).await
    }

    /// Construct an embedded manager with explicit account options.
    pub async fn embedded_with_options(
        secret_path: impl Into<PathBuf>,
        options: PlayitOptions,
    ) -> Result<Self, PlayitError> {
        let secret_path = secret_path.into();
        let runtime_options = RuntimeOptions {
            secret_path: secret_path.clone(),
            api_base: options.api_base.clone(),
            ..RuntimeOptions::default()
        };
        let (runtime, handle) = PlayitRuntime::start(runtime_options).await?;

        Ok(Self {
            service: Arc::new(RwLock::new(
                Arc::new(crate::embedded::EmbeddedPlayitService::new(handle))
                    as Arc<dyn PlayitService>,
            )),
            runtime: Some(Arc::new(Mutex::new(Some(runtime)))),
            embedded: Some(EmbeddedParams {
                secret_path,
                api_base: options.api_base.clone(),
            }),
            account: AccountController::new(options.api_base, options.session_path),
        })
    }

    /// Construct a manager using the separately managed external daemon.
    pub fn external() -> Self {
        let session_path =
            std::env::temp_dir().join("mcpanel-playit-external-account-session.json");
        Self::external_with_options(PlayitOptions::new(session_path))
    }

    /// Construct an external-daemon manager with explicit account options.
    ///
    /// The panel passes its `data/playit` session path here so direct logins
    /// survive restarts in external mode too.
    pub fn external_with_options(options: PlayitOptions) -> Self {
        Self {
            service: Arc::new(RwLock::new(
                Arc::new(IpcPlayitService) as Arc<dyn PlayitService>
            )),
            runtime: None,
            embedded: None,
            account: AccountController::new(options.api_base, options.session_path),
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
    /// Account operations use the default API base and an isolated scratch
    /// session file; use [`Self::with_service_and_options`] when tests need
    /// a controlled API base or a shared session path.
    pub fn with_service<S>(service: S) -> Self
    where
        S: PlayitService + 'static,
    {
        let session_path = std::env::temp_dir().join(format!(
            "mcpanel-playit-test-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        Self::with_service_and_options(
            service,
            PlayitOptions {
                api_base: DEFAULT_API_BASE.into(),
                session_path,
            },
        )
    }

    /// Construct a manager around an injected service with explicit account
    /// options. Used by tests that drive the account side against a mock
    /// HTTPS server.
    pub fn with_service_and_options<S>(service: S, options: PlayitOptions) -> Self
    where
        S: PlayitService + 'static,
    {
        Self {
            service: Arc::new(RwLock::new(Arc::new(service) as Arc<dyn PlayitService>)),
            runtime: None,
            embedded: None,
            account: AccountController::new(options.api_base, options.session_path),
        }
    }

    /// The currently active agent backend. Embedded disconnect/reconnect
    /// swaps this when the runtime restarts, so every operation resolves it
    /// fresh instead of holding a stale handle.
    async fn agent_service(&self) -> Arc<dyn PlayitService> {
        self.service.read().await.clone()
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
        let snapshot: SubscribeResponse = self.agent_service().await.snapshot().await?;
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
        let account = self.agent_service().await.account().await?;
        Ok(account_view(account))
    }

    /// Start the browser-based Playit claim flow.
    pub async fn start_claim(&self) -> Result<ClaimInfo, PlayitError> {
        let claim = self.agent_service().await.start_claim().await?;
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
        let response = self.agent_service().await.list_tunnels().await?;
        Ok(response.tunnels.into_iter().map(tunnel_view).collect())
    }

    /// List every tunnel owned by the authenticated Playit account.
    ///
    /// When a direct account session exists, the Bearer account API is the
    /// source; otherwise the runtime/IPC view is used. This removes the old
    /// "account list denied" limitation for logged-in installations while
    /// keeping the runtime fallback for secret-only setups.
    pub async fn account_tunnels(&self) -> Result<Vec<PlayitTunnel>, PlayitError> {
        if let Some(result) = self.account.account_tunnels_if_logged_in().await {
            match result {
                Ok(tunnels) => return Ok(tunnels),
                Err(error) => {
                    tracing::warn!(
                        error = ?error,
                        "direct Playit account tunnel list failed; falling back to the agent view"
                    );
                }
            }
        }
        let response = self.agent_service().await.list_account_tunnels().await?;
        Ok(response
            .tunnels
            .into_iter()
            .map(account_tunnel_view)
            .collect())
    }

    /// List the tunnels visible to the panel, preferring the account-wide
    /// list but falling back to the current-agent materialized list when the
    /// account endpoint is denied.
    ///
    /// A Playit account-wide permission restriction must not turn into a
    /// completely broken Playit page or a failed attach when the current
    /// agent still has valid local tunnel information.
    pub async fn visible_tunnels(&self) -> Result<Vec<PlayitTunnel>, PlayitError> {
        use playit_ipc::model::ServiceErrorCode;

        match self.account_tunnels().await {
            Ok(tunnels) => Ok(tunnels),
            Err(error)
                if matches!(
                    error.service_code(),
                    Some(ServiceErrorCode::PermissionDenied)
                ) =>
            {
                tracing::warn!(
                    "Playit account tunnel list denied; falling back to current-agent tunnel list"
                );
                self.tunnels().await
            }
            Err(error) => Err(error),
        }
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
            .agent_service()
            .await
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
            .agent_service()
            .await
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
            .agent_service()
            .await
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
    /// tunnel with the same server name. A compatible but drifted tunnel
    /// (wrong destination and/or wrong agent) is updated in place via the
    /// existing reassign operation; a tunnel is created only when no
    /// compatible existing tunnel can be reused or reconfigured.
    pub async fn ensure_server_tunnel(
        &self,
        server_id: &str,
        server_name: &str,
        port: u16,
    ) -> Result<EnsuredServerTunnel, PlayitError> {
        self.ensure_server_tunnel_with_name(server_id, server_name, port, None)
            .await
    }

    /// Same as [`Self::ensure_server_tunnel`], but an operator-supplied
    /// display name is used only when a new tunnel must be created. A custom
    /// name never bypasses reuse logic or forces duplication.
    pub async fn ensure_server_tunnel_with_name(
        &self,
        server_id: &str,
        server_name: &str,
        port: u16,
        custom_name: Option<String>,
    ) -> Result<EnsuredServerTunnel, PlayitError> {
        // Prefer the account-wide list, but reuse what is visible on the
        // current agent when the broader list is forbidden. The fallback may
        // not expose tunnels on other agents; that is acceptable.
        let tunnels = self.visible_tunnels().await?;
        let current_agent_id = self.current_agent_id().await?;
        self.ensure_by_name(
            &tunnels,
            &current_agent_id,
            server_id,
            server_name,
            port,
            custom_name,
        )
        .await
    }

    /// Reconcile a server's stored tunnel association against live Playit
    /// state, healing what the read-only view cannot.
    ///
    /// The panel's stored tunnel id is tried first: a still-visible tunnel
    /// is adopted in place — reassigned to the current agent and the desired
    /// destination when it drifted — even when the panel recorded it under a
    /// different agent (for example after the agent was deleted remotely and
    /// recreated). A stored tunnel Playit no longer reports falls back to
    /// the stable-name ensure path, which recreates it when nothing reusable
    /// remains. Disabled, incompatible, and ambiguous tunnels stay conflicts
    /// so callers surface them instead of silently duplicating or destroying
    /// tunnels. This never deletes a tunnel.
    pub async fn reconcile_server_tunnel(
        &self,
        server_id: &str,
        server_name: &str,
        port: u16,
        stored_tunnel_id: Option<&str>,
        local_address: &str,
        custom_name: Option<String>,
    ) -> Result<EnsuredServerTunnel, PlayitError> {
        let tunnels = self.visible_tunnels().await?;
        let current_agent_id = self.current_agent_id().await?;

        if let Some(stored) = stored_tunnel_id
            .map(str::trim)
            .filter(|stored| !stored.is_empty())
        {
            let matches: Vec<_> = tunnels
                .iter()
                .filter(|tunnel| tunnel.id == stored)
                .cloned()
                .collect();
            if matches.len() > 1 {
                return Err(PlayitError::Conflict(
                    "Playit returned duplicate records for the stored tunnel id".into(),
                ));
            }
            if let Some(existing) = matches.into_iter().next() {
                return self
                    .adopt_tunnel(&existing, &current_agent_id, port, local_address)
                    .await;
            }
        }

        self.ensure_by_name(
            &tunnels,
            &current_agent_id,
            server_id,
            server_name,
            port,
            custom_name,
        )
        .await
    }

    /// The current agent id backing tunnel operations. Agent-scoped commands
    /// always target this agent implicitly, so a missing id fails before any
    /// remote call instead of acting on an unknown agent.
    async fn current_agent_id(&self) -> Result<String, PlayitError> {
        let account = self.account().await?;
        account
            .agent_id
            .as_deref()
            .filter(|agent_id| !agent_id.trim().is_empty())
            .ok_or_else(|| {
                PlayitError::Unavailable("the current Playit agent id is not available yet".into())
            })
            .map(str::to_owned)
    }

    /// Reuse the stable panel-owned tunnel for a server, or one unique legacy
    /// tunnel with the same server name, updating a drifted tunnel in place
    /// and creating one only when no compatible existing tunnel remains.
    async fn ensure_by_name(
        &self,
        tunnels: &[PlayitTunnel],
        current_agent_id: &str,
        server_id: &str,
        server_name: &str,
        port: u16,
        custom_name: Option<String>,
    ) -> Result<EnsuredServerTunnel, PlayitError> {
        let managed_name = format!("mcpanel:{server_id}");
        let existing = select_unique_tunnel(tunnels, &managed_name, server_name)?;

        let Some(existing) = existing else {
            let create_name = custom_name
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or(managed_name);
            return Ok(EnsuredServerTunnel {
                tunnel: self
                    .create_minecraft_java_tunnel(port, Some("127.0.0.1".into()), Some(create_name))
                    .await?,
                disposition: EnsureTunnelDisposition::Created,
            });
        };

        self.adopt_tunnel(&existing, current_agent_id, port, "127.0.0.1")
            .await
    }

    /// Reuse a visible tunnel, repairing a drifted destination or a foreign
    /// agent assignment in place. Disabled and incompatible tunnels are
    /// conflicts and must never be silently duplicated.
    async fn adopt_tunnel(
        &self,
        existing: &PlayitTunnel,
        current_agent_id: &str,
        port: u16,
        local_address: &str,
    ) -> Result<EnsuredServerTunnel, PlayitError> {
        validate_reusable_minecraft_tunnel(existing)?;

        let destination_changed = existing.local_address.as_deref() != Some(local_address)
            || existing.local_port != Some(port);
        let agent_changed = existing.agent_id.as_deref() != Some(current_agent_id);

        if destination_changed || agent_changed {
            self.reassign_tunnel(&existing.id, port, Some(local_address.to_owned()))
                .await?;

            return Ok(EnsuredServerTunnel {
                tunnel: TunnelCreateInfo {
                    tunnel_id: existing.id.clone(),
                    message: Some("Existing Minecraft Java tunnel updated".into()),
                },
                disposition: EnsureTunnelDisposition::Updated {
                    previous_agent_id: existing.agent_id.clone(),
                    destination_changed,
                },
            });
        }

        Ok(EnsuredServerTunnel {
            tunnel: TunnelCreateInfo {
                tunnel_id: existing.id.clone(),
                message: Some("Existing Minecraft Java tunnel reused".into()),
            },
            disposition: EnsureTunnelDisposition::Reused,
        })
    }

    /// Apply the only safe compensation available after an ensure operation
    /// has succeeded but the panel's local transaction failed.
    ///
    /// Playit's pinned reassign command can only target the current agent; it
    /// cannot restore an arbitrary previous agent. Updated tunnels are
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
            EnsureTunnelDisposition::Updated {
                previous_agent_id, ..
            } => {
                tracing::warn!(
                    tunnel = %ensured.tunnel.tunnel_id,
                    previous_agent_id = ?previous_agent_id,
                    "preserving updated Playit tunnel; previous assignment cannot be restored by the pinned API and reconciliation may be required"
                );
            }
        }
        Ok(())
    }

    /// Delete a tunnel by its stable Playit id.
    pub async fn delete_tunnel(&self, tunnel_id: &str) -> Result<(), PlayitError> {
        let response = self.agent_service().await.delete_tunnel(tunnel_id).await?;
        if !response.accepted {
            return Err(PlayitError::Rejected(
                response
                    .message
                    .unwrap_or_else(|| "delete command was not accepted".into()),
            ));
        }
        Ok(())
    }

    /// Sign in to playit.gg directly with email + password.
    ///
    /// Returns safe state only: never a session key. When the account
    /// requires TOTP, the pending login is held in memory until
    /// [`Self::complete_totp`] succeeds.
    pub async fn auth_login(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AccountSessionState, PlayitError> {
        self.account.login(email, password).await
    }

    /// Submit the TOTP code for the pending login from [`Self::auth_login`].
    pub async fn complete_totp(&self, code: &str) -> Result<AccountSessionState, PlayitError> {
        self.account.complete_totp(code).await
    }

    /// Delete the Bearer account session only. The agent remains running;
    /// tunnels are untouched.
    pub async fn auth_logout(&self) -> Result<(), PlayitError> {
        self.account.logout().await
    }

    /// Report the locally known account session state without network access.
    pub async fn auth_status(&self) -> AccountSessionState {
        self.account.session_state().await
    }

    /// Validate the account session with a harmless account read.
    pub async fn auth_validate(&self) -> Result<AccountSessionState, PlayitError> {
        self.account.validate().await
    }

    /// List the domains visible to the logged-in account.
    pub async fn domains(&self) -> Result<Vec<DomainInfo>, PlayitError> {
        self.account.domains().await
    }

    /// Look up a pending machine claim as the account.
    pub async fn claim_details(&self, code: &str) -> Result<ClaimDetailsInfo, PlayitError> {
        self.account.claim_details(code.trim()).await
    }

    /// Approve a pending machine claim as the account, creating the agent.
    pub async fn approve_claim(
        &self,
        code: &str,
        name: Option<String>,
    ) -> Result<String, PlayitError> {
        let name = claim_agent_name(name)?;
        self.account.approve_claim(code.trim(), &name).await
    }

    /// Reject a pending machine claim as the account.
    pub async fn reject_claim(&self, code: &str) -> Result<(), PlayitError> {
        self.account.reject_claim(code.trim()).await
    }

    /// List the agents owned by the logged-in account.
    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>, PlayitError> {
        self.account.list_agents().await
    }

    /// Delete an account agent with an explicit tunnel strategy.
    ///
    /// Deleting the agent this panel's runtime is currently running on is
    /// refused: disconnect the agent first so the operation cannot silently
    /// orphan the local runtime.
    pub async fn delete_agent(
        &self,
        agent_id: &str,
        options: &DeleteAgentOptions,
    ) -> Result<(), PlayitError> {
        let target = agent_id.trim();
        if let Ok(current) = self.agent_service().await.account().await {
            if let Some(current_id) = current
                .agent_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
            {
                if ids_match(current_id, target) {
                    return Err(PlayitError::Conflict(
                        "refusing to delete the agent this panel is running on; disconnect the agent first".into(),
                    ));
                }
            }
        }
        self.account.delete_agent(target, options).await
    }

    /// Browserless agent setup: claim this machine's agent under the logged-in
    /// account with no browser redirect.
    ///
    /// The flow verifies the account session, starts the machine claim, looks
    /// the code up and accepts it over the account API, then waits for the
    /// runtime claim exchange to finish and the agent to reach Running. The
    /// claim code stays server-side throughout. An already-configured runtime
    /// is returned as-is, so repeated calls are idempotent. The legacy
    /// browser claim route remains as a fallback.
    pub async fn setup_direct(
        &self,
        agent_name: Option<String>,
    ) -> Result<DirectSetupResult, PlayitError> {
        self.setup_direct_with_options(agent_name, SetupDirectOptions::default())
            .await
    }

    /// [`Self::setup_direct`] with injectable waits, used by tests.
    pub async fn setup_direct_with_options(
        &self,
        agent_name: Option<String>,
        options: SetupDirectOptions,
    ) -> Result<DirectSetupResult, PlayitError> {
        if !self.account.is_logged_in().await {
            return Err(PlayitError::Account(
                crate::error::AccountError::NotLoggedIn,
            ));
        }
        let service = self.agent_service().await;
        let snapshot = service.snapshot().await?;
        let has_secret = snapshot.snapshot.status.has_secret;
        if matches!(snapshot.snapshot.lifecycle, AgentLifecycle::Running(_)) && has_secret {
            let agent_id = service
                .account()
                .await
                .ok()
                .and_then(|account| account.agent_id)
                .filter(|id| !id.trim().is_empty());
            return Ok(DirectSetupResult {
                agent_id,
                already_configured: true,
                connected: true,
                message: Some("the Playit agent is already configured".into()),
            });
        }
        if !matches!(
            snapshot.snapshot.lifecycle,
            AgentLifecycle::WaitingForSecret
        ) {
            return Err(PlayitError::Conflict(
                "the Playit agent is not ready for setup; disconnect it first or wait for it to settle".into(),
            ));
        }

        let claim = service.start_claim().await?;
        let code = claim_code_from_url(&claim.claim_url)?;
        let name = claim_agent_name(agent_name)?;

        // The runtime polls claim setup in the background; the account side
        // may need a moment to see the pending claim.
        let deadline = tokio::time::Instant::now() + options.details_timeout;
        let details = loop {
            match self.account.claim_details(&code).await {
                Ok(details) => break details,
                Err(error) if is_transient_claim_lookup(&error) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(error);
                    }
                    tokio::time::sleep(options.poll_interval).await;
                }
                Err(error) => return Err(error),
            }
        };
        let _ = details;

        let agent_id = self.account.approve_claim(&code, &name).await?;

        // The runtime observes UserAccepted on its next poll and exchanges
        // the code for the agent secret on its own.
        let deadline = tokio::time::Instant::now() + options.running_timeout;
        loop {
            let snapshot = service.snapshot().await?;
            if matches!(snapshot.snapshot.lifecycle, AgentLifecycle::Running(_)) {
                return Ok(DirectSetupResult {
                    agent_id: Some(agent_id),
                    already_configured: false,
                    connected: true,
                    message: None,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(DirectSetupResult {
                    agent_id: Some(agent_id),
                    already_configured: false,
                    connected: false,
                    message: Some(
                        "the claim was accepted but the agent has not connected yet; check the Playit status".into(),
                    ),
                });
            }
            tokio::time::sleep(options.poll_interval).await;
        }
    }

    /// Disconnect the agent: remove the agent secret and restart the embedded
    /// runtime into `WaitingForSecret`. The account session is untouched.
    /// External mode resets the daemon secret and leaves the restart to the
    /// daemon's service manager.
    pub async fn disconnect_agent(&self) -> Result<PlayitStatus, PlayitError> {
        let service = self.agent_service().await;
        let snapshot = service.snapshot().await.ok();
        let needs_reset = snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot.snapshot.status.has_secret
                    || !matches!(
                        snapshot.snapshot.lifecycle,
                        AgentLifecycle::WaitingForSecret
                    )
            })
            .unwrap_or(true);
        if needs_reset {
            service.reset_secret().await?;
        }
        if self.embedded.is_some() {
            self.restart_embedded().await?;
        }
        self.status().await
    }

    /// Reconnect the agent runtime: restart a stopped embedded runtime into
    /// `WaitingForSecret` (or current secret) without touching the account
    /// session. A running runtime is left alone and its status returned.
    /// External mode verifies the daemon is reachable.
    pub async fn reconnect_agent(&self) -> Result<PlayitStatus, PlayitError> {
        if self.embedded.is_some() {
            let restart = match &self.runtime {
                None => true,
                Some(runtime) => runtime.lock().await.is_none(),
            };
            let stopped = !restart
                && matches!(
                    self.agent_service()
                        .await
                        .snapshot()
                        .await
                        .map(|snapshot| snapshot.snapshot.lifecycle),
                    Ok(AgentLifecycle::Stopping)
                );
            if restart || stopped {
                self.restart_embedded().await?;
            }
        }
        self.status().await
    }

    /// Stop the current embedded runtime (best-effort) and start a fresh one
    /// from the same secret path, swapping the active backend.
    async fn restart_embedded(&self) -> Result<(), PlayitError> {
        let Some(params) = &self.embedded else {
            return Err(PlayitError::Unavailable(
                "only the embedded runtime can be restarted by the panel".into(),
            ));
        };
        if let Some(runtime) = &self.runtime {
            if let Some(previous) = runtime.lock().await.take() {
                let _ = previous.shutdown().await;
            }
        }
        let options = RuntimeOptions {
            secret_path: params.secret_path.clone(),
            api_base: params.api_base.clone(),
            ..RuntimeOptions::default()
        };
        let (runtime, handle) = PlayitRuntime::start(options).await?;
        *self.service.write().await =
            Arc::new(crate::embedded::EmbeddedPlayitService::new(handle)) as Arc<dyn PlayitService>;
        if let Some(slot) = &self.runtime {
            *slot.lock().await = Some(runtime);
        }
        Ok(())
    }
}

/// Extract the machine claim code from a `https://playit.gg/claim/{code}`
/// URL. The code stays server-side; only safe claim details leave the panel.
fn claim_code_from_url(claim_url: &str) -> Result<String, PlayitError> {
    let code = claim_url
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim();
    if code.is_empty() || code.contains(|character: char| character.is_whitespace()) {
        return Err(PlayitError::Protocol(
            "claim response did not contain a usable claim code".into(),
        ));
    }
    Ok(code.into())
}

/// Validate an operator-supplied agent name for claim approval.
fn claim_agent_name(name: Option<String>) -> Result<String, PlayitError> {
    let name = name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "mcpanel".into());
    if name.chars().count() > 64 {
        return Err(PlayitError::Conflict(
            "agent name must be at most 64 characters".into(),
        ));
    }
    Ok(name)
}

/// Whether a claim-details failure is worth retrying while the runtime's
/// background claim poll announces the code to the account side.
fn is_transient_claim_lookup(error: &PlayitError) -> bool {
    match error {
        PlayitError::Account(crate::error::AccountError::Api(detail)) => {
            detail.contains("has not announced its claim yet")
        }
        _ => false,
    }
}

/// Compare two agent ids, tolerating UUID formatting differences.
fn ids_match(first: &str, second: &str) -> bool {
    if first.trim() == second.trim() {
        return true;
    }
    match (
        first.trim().parse::<uuid::Uuid>(),
        second.trim().parse::<uuid::Uuid>(),
    ) {
        (Ok(first), Ok(second)) => first == second,
        _ => false,
    }
}

/// Select the unique tunnel matching the stable managed name, falling back
/// to a single safe legacy match on the server name. Returns a conflict when
/// multiple candidates match rather than silently choosing one.
fn select_unique_tunnel(
    tunnels: &[PlayitTunnel],
    managed_name: &str,
    server_name: &str,
) -> Result<Option<PlayitTunnel>, PlayitError> {
    let stable_matches: Vec<_> = tunnels
        .iter()
        .filter(|tunnel| tunnel.name.as_deref() == Some(managed_name))
        .cloned()
        .collect();
    if stable_matches.len() > 1 {
        return Err(PlayitError::Conflict(format!(
            "multiple managed tunnels are named {managed_name}"
        )));
    }
    if let Some(existing) = stable_matches.into_iter().next() {
        return Ok(Some(existing));
    }

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
    Ok(legacy_matches.into_iter().next())
}

/// Verify that an existing tunnel is a safe, reusable Minecraft Java tunnel.
/// Drifted destinations or foreign agents are repaired by the caller, not
/// rejected here; disabled or incompatible tunnels are conflicts and must
/// never be silently duplicated.
fn validate_reusable_minecraft_tunnel(tunnel: &PlayitTunnel) -> Result<(), PlayitError> {
    if tunnel.id.trim().is_empty() {
        return Err(PlayitError::Conflict(
            "Playit returned a tunnel without a usable id".into(),
        ));
    }
    // The current-agent materialized list does not carry a semantic tunnel
    // type, so `None` is treated as unknown-but-reusable when the stable
    // name matched. An explicit non-Java type is still a conflict.
    if tunnel
        .tunnel_type
        .as_deref()
        .is_some_and(|kind| kind != "minecraft-java")
    {
        return Err(PlayitError::Conflict(format!(
            "Playit tunnel {} exists for this server but is not a Minecraft Java tunnel",
            tunnel.id
        )));
    }
    if tunnel.disabled {
        return Err(PlayitError::Conflict(format!(
            "Playit disabled managed tunnel {}{}",
            tunnel.id,
            tunnel
                .disabled_reason
                .as_deref()
                .map(|reason| format!(": {reason}"))
                .unwrap_or_default()
        )));
    }
    if tunnel.protocol != PlayitProtocol::Tcp {
        return Err(PlayitError::Conflict(format!(
            "managed tunnel {} does not use the Minecraft Java TCP protocol",
            tunnel.id
        )));
    }
    Ok(())
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

    async fn set_secret(
        &self,
        _: String,
    ) -> Result<playit_ipc::model::CommandResponse, PlayitError> {
        Err(PlayitError::Unavailable(self.message.clone()))
    }

    async fn reset_secret(&self) -> Result<playit_ipc::model::CommandResponse, PlayitError> {
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
    type ReassignedTunnel = (String, u16, Option<String>);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AccountListFailure {
        PermissionDenied,
        Unavailable,
    }

    #[derive(Default, Clone)]
    struct MockService {
        status: Arc<Mutex<ServiceStatus>>,
        lifecycle: Arc<Mutex<AgentLifecycle>>,
        account: Arc<Mutex<AccountResponse>>,
        claim: Arc<Mutex<ClaimResponse>>,
        tunnels: Arc<Mutex<TunnelListResponse>>,
        account_tunnels: Arc<Mutex<AccountTunnelListResponse>>,
        account_tunnels_failure: Arc<Mutex<Option<AccountListFailure>>>,
        reset_calls: Arc<Mutex<usize>>,
        created: Arc<Mutex<Vec<CreatedTunnel>>>,
        snapshot_calls: Arc<Mutex<usize>>,
        status_reads: Arc<Mutex<usize>>,
        lifecycle_reads: Arc<Mutex<usize>>,
        deleted: Arc<Mutex<Vec<String>>>,
        reassigned: Arc<Mutex<Vec<String>>>,
        reassigned_args: Arc<Mutex<Vec<ReassignedTunnel>>>,
    }

    fn permission_denied_error() -> PlayitError {
        PlayitError::Runtime(playit_runtime::RuntimeError::Api {
            code: playit_ipc::model::ServiceErrorCode::PermissionDenied,
            message: "permission denied".into(),
            retryable: false,
            details: None,
        })
    }

    fn unavailable_error() -> PlayitError {
        PlayitError::Runtime(playit_runtime::RuntimeError::Api {
            code: playit_ipc::model::ServiceErrorCode::ApiUnavailable,
            message: "not ready".into(),
            retryable: true,
            details: None,
        })
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
            match *self.account_tunnels_failure.lock().unwrap() {
                Some(AccountListFailure::PermissionDenied) => Err(permission_denied_error()),
                Some(AccountListFailure::Unavailable) => Err(unavailable_error()),
                None => Ok(self.account_tunnels.lock().unwrap().clone()),
            }
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
            local_port: u16,
            local_address: Option<String>,
        ) -> Result<CommandResponse, PlayitError> {
            self.reassigned.lock().unwrap().push(tunnel_id.into());
            self.reassigned_args.lock().unwrap().push((
                tunnel_id.into(),
                local_port,
                local_address,
            ));
            Ok(CommandResponse {
                accepted: true,
                message: None,
            })
        }

        async fn set_secret(&self, _secret: String) -> Result<CommandResponse, PlayitError> {
            Ok(CommandResponse {
                accepted: true,
                message: None,
            })
        }

        async fn reset_secret(&self) -> Result<CommandResponse, PlayitError> {
            *self.reset_calls.lock().unwrap() += 1;
            Ok(CommandResponse {
                accepted: true,
                message: None,
            })
        }
    }

    fn running_service() -> MockService {
        MockService {
            status: Arc::new(Mutex::new(ServiceStatus {
                phase: ServicePhase::Running,
                version: "1.2.3".into(),
                has_secret: true,
                protocol: ProtocolInfo {
                    ipc_version: playit_ipc::ipc::IPC_VERSION,
                    ..ProtocolInfo::default()
                },
                ..ServiceStatus::default()
            })),
            lifecycle: Arc::new(Mutex::new(AgentLifecycle::Running(Default::default()))),
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
            status: Arc::new(Mutex::new(ServiceStatus {
                phase: ServicePhase::WaitingForSecret,
                ..ServiceStatus::default()
            })),
            lifecycle: Arc::new(Mutex::new(AgentLifecycle::WaitingForSecret)),
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
                status: Arc::new(Mutex::new(ServiceStatus {
                    phase,
                    has_secret: false,
                    ..ServiceStatus::default()
                })),
                lifecycle: Arc::new(Mutex::new(lifecycle)),
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
                status: Arc::new(Mutex::new(ServiceStatus {
                    phase,
                    has_secret: true,
                    ..ServiceStatus::default()
                })),
                lifecycle: Arc::new(Mutex::new(lifecycle)),
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
            Some("Existing Minecraft Java tunnel updated")
        );
        assert_eq!(
            created.disposition,
            EnsureTunnelDisposition::Updated {
                previous_agent_id: Some("agent-2".into()),
                destination_changed: false,
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
    async fn ensure_updates_destination_in_place_without_creating() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        let mut tunnel = managed_tunnel("mcpanel:server-1", "tunnel-1", Some("agent-1"));
        tunnel.local_port = Some(25565);
        service.account_tunnels.lock().unwrap().tunnels.push(tunnel);
        let reassigned_args = Arc::clone(&service.reassigned_args);
        let manager = PlayitManager::with_service(service);

        let ensured = manager
            .ensure_server_tunnel("server-1", "Survival SMP", 25566)
            .await
            .unwrap();

        assert_eq!(ensured.tunnel.tunnel_id, "tunnel-1");
        assert_eq!(
            ensured.disposition,
            EnsureTunnelDisposition::Updated {
                previous_agent_id: Some("agent-1".into()),
                destination_changed: true,
            }
        );
        assert_eq!(
            *reassigned_args.lock().unwrap(),
            vec![("tunnel-1".to_owned(), 25566, Some("127.0.0.1".to_owned()))]
        );
    }

    #[tokio::test]
    async fn ensure_reassigns_foreign_agent_and_normalizes_destination() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("current-agent".into());
        let mut tunnel = managed_tunnel("mcpanel:server-1", "tunnel-1", Some("old-agent"));
        tunnel.local_address = Some("0.0.0.0".into());
        tunnel.local_port = Some(25565);
        service.account_tunnels.lock().unwrap().tunnels.push(tunnel);
        let reassigned_args = Arc::clone(&service.reassigned_args);
        let manager = PlayitManager::with_service(service);

        let ensured = manager
            .ensure_server_tunnel("server-1", "Survival SMP", 25566)
            .await
            .unwrap();

        assert_eq!(ensured.tunnel.tunnel_id, "tunnel-1");
        assert_eq!(
            ensured.disposition,
            EnsureTunnelDisposition::Updated {
                previous_agent_id: Some("old-agent".into()),
                destination_changed: true,
            }
        );
        assert_eq!(
            *reassigned_args.lock().unwrap(),
            vec![("tunnel-1".to_owned(), 25566, Some("127.0.0.1".to_owned()))]
        );
    }

    #[tokio::test]
    async fn reconcile_adopts_stored_tunnel_from_a_previous_agent() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        let mut stored = managed_tunnel("custom-name", "stored-tunnel", Some("agent-2"));
        stored.local_address = Some("::1".into());
        stored.local_port = Some(25565);
        service.account_tunnels.lock().unwrap().tunnels.extend([
            stored,
            managed_tunnel("mcpanel:server-1", "other-tunnel", Some("agent-1")),
        ]);
        let reassigned_args = Arc::clone(&service.reassigned_args);
        let created = Arc::clone(&service.created);
        let manager = PlayitManager::with_service(service);

        let healed = manager
            .reconcile_server_tunnel(
                "server-1",
                "Survival SMP",
                25565,
                Some("stored-tunnel"),
                "::1",
                None,
            )
            .await
            .unwrap();

        // The stored id wins over the stable-name match and is adopted onto
        // the current agent, preserving the desired loopback destination.
        assert_eq!(healed.tunnel.tunnel_id, "stored-tunnel");
        assert_eq!(
            healed.disposition,
            EnsureTunnelDisposition::Updated {
                previous_agent_id: Some("agent-2".into()),
                destination_changed: false,
            }
        );
        assert_eq!(
            *reassigned_args.lock().unwrap(),
            vec![("stored-tunnel".to_owned(), 25565, Some("::1".to_owned()))]
        );
        assert!(created.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn reconcile_reuses_a_matching_stored_tunnel_untouched() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        service
            .account_tunnels
            .lock()
            .unwrap()
            .tunnels
            .push(managed_tunnel(
                "custom-name",
                "stored-tunnel",
                Some("agent-1"),
            ));
        let reassigned = Arc::clone(&service.reassigned);
        let created = Arc::clone(&service.created);
        let manager = PlayitManager::with_service(service);

        let healed = manager
            .reconcile_server_tunnel(
                "server-1",
                "Survival SMP",
                25565,
                Some("stored-tunnel"),
                "127.0.0.1",
                None,
            )
            .await
            .unwrap();

        assert_eq!(healed.tunnel.tunnel_id, "stored-tunnel");
        assert_eq!(healed.disposition, EnsureTunnelDisposition::Reused);
        assert!(reassigned.lock().unwrap().is_empty());
        assert!(created.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn reconcile_recreates_a_remotely_deleted_tunnel() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        let created = Arc::clone(&service.created);
        let manager = PlayitManager::with_service(service);

        let healed = manager
            .reconcile_server_tunnel(
                "server-1",
                "Survival SMP",
                25565,
                Some("deleted-tunnel"),
                "127.0.0.1",
                None,
            )
            .await
            .unwrap();

        assert_eq!(healed.tunnel.tunnel_id, "tunnel-1");
        assert_eq!(healed.disposition, EnsureTunnelDisposition::Created);
        let created = created.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].0, 25565);
        assert!(matches!(created[0].1, TunnelProtocol::Tcp));
        assert_eq!(created[0].2.as_deref(), Some("127.0.0.1"));
        assert_eq!(created[0].3.as_deref(), Some("mcpanel:server-1"));
    }

    #[tokio::test]
    async fn reconcile_reports_a_disabled_stored_tunnel_as_conflict() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        let mut tunnel = managed_tunnel("custom-name", "stored-tunnel", Some("agent-1"));
        tunnel.is_disabled = true;
        tunnel.disabled_reason = Some("account limit".into());
        service.account_tunnels.lock().unwrap().tunnels.push(tunnel);
        let created = Arc::clone(&service.created);
        let manager = PlayitManager::with_service(service);

        let result = manager
            .reconcile_server_tunnel(
                "server-1",
                "Survival SMP",
                25565,
                Some("stored-tunnel"),
                "127.0.0.1",
                None,
            )
            .await;

        assert!(matches!(
            result,
            Err(PlayitError::Conflict(message)) if message.contains("account limit")
        ));
        assert!(created.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn reconcile_reports_duplicate_stored_ids_as_conflict() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        service.account_tunnels.lock().unwrap().tunnels.extend([
            managed_tunnel("custom-name", "stored-tunnel", Some("agent-1")),
            managed_tunnel("other-name", "stored-tunnel", Some("agent-1")),
        ]);
        let manager = PlayitManager::with_service(service);

        assert!(matches!(
            manager
                .reconcile_server_tunnel(
                    "server-1",
                    "Survival SMP",
                    25565,
                    Some("stored-tunnel"),
                    "127.0.0.1",
                    None,
                )
                .await,
            Err(PlayitError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn ensure_is_idempotent_for_repeated_attach() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        service
            .account_tunnels
            .lock()
            .unwrap()
            .tunnels
            .push(managed_tunnel(
                "mcpanel:server-1",
                "tunnel-1",
                Some("agent-1"),
            ));
        let manager = PlayitManager::with_service(service);

        for _ in 0..3 {
            let ensured = manager
                .ensure_server_tunnel("server-1", "Survival SMP", 25565)
                .await
                .unwrap();
            assert_eq!(ensured.tunnel.tunnel_id, "tunnel-1");
            assert_eq!(ensured.disposition, EnsureTunnelDisposition::Reused);
        }
    }

    #[tokio::test]
    async fn visible_tunnels_falls_back_when_account_list_is_denied() {
        use playit_ipc::model::TunnelState;

        let service = running_service();
        *service.account_tunnels_failure.lock().unwrap() =
            Some(AccountListFailure::PermissionDenied);
        service.tunnels.lock().unwrap().tunnels.push(TunnelState {
            id: "agent-tunnel".into(),
            name: Some("mcpanel:server-1".into()),
            display_address: "example.playit.gg:1".into(),
            destination: "127.0.0.1:25565".into(),
            protocol: TunnelProtocol::Tcp,
            local_address: Some("127.0.0.1".into()),
            local_port: Some(25565),
            ..TunnelState::default()
        });
        let manager = PlayitManager::with_service(service);

        let tunnels = manager.visible_tunnels().await.unwrap();
        assert_eq!(tunnels.len(), 1);
        assert_eq!(tunnels[0].id, "agent-tunnel");
    }

    #[tokio::test]
    async fn visible_tunnels_propagates_non_permission_errors() {
        let service = running_service();
        *service.account_tunnels_failure.lock().unwrap() = Some(AccountListFailure::Unavailable);
        let manager = PlayitManager::with_service(service);

        assert!(manager.visible_tunnels().await.is_err());
    }

    #[tokio::test]
    async fn ensure_reuses_current_agent_tunnel_when_account_list_is_denied() {
        use playit_ipc::model::TunnelState;

        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        *service.account_tunnels_failure.lock().unwrap() =
            Some(AccountListFailure::PermissionDenied);
        service.tunnels.lock().unwrap().tunnels.push(TunnelState {
            id: "agent-tunnel".into(),
            name: Some("mcpanel:server-1".into()),
            display_address: "example.playit.gg:1".into(),
            destination: "127.0.0.1:25565".into(),
            protocol: TunnelProtocol::Tcp,
            local_address: Some("127.0.0.1".into()),
            local_port: Some(25565),
            ..TunnelState::default()
        });
        let manager = PlayitManager::with_service(service);

        let ensured = manager
            .ensure_server_tunnel("server-1", "Survival SMP", 25565)
            .await
            .unwrap();
        // The same visible tunnel id is reused (possibly via an in-place
        // update normalizing unknown fallback metadata), never duplicated.
        assert_eq!(ensured.tunnel.tunnel_id, "agent-tunnel");
    }

    #[tokio::test]
    async fn ensure_with_custom_name_reuses_existing_and_uses_name_only_on_create() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        service
            .account_tunnels
            .lock()
            .unwrap()
            .tunnels
            .push(managed_tunnel(
                "mcpanel:server-1",
                "tunnel-1",
                Some("agent-1"),
            ));
        let manager = PlayitManager::with_service(service);

        let ensured = manager
            .ensure_server_tunnel_with_name(
                "server-1",
                "Survival SMP",
                25565,
                Some("Custom Display".into()),
            )
            .await
            .unwrap();
        assert_eq!(ensured.tunnel.tunnel_id, "tunnel-1");
        assert_eq!(ensured.disposition, EnsureTunnelDisposition::Reused);
    }

    #[tokio::test]
    async fn ensure_with_custom_name_uses_it_for_new_tunnels() {
        let service = running_service();
        service.account.lock().unwrap().agent_id = Some("agent-1".into());
        let created = Arc::clone(&service.created);
        let manager = PlayitManager::with_service(service);

        let ensured = manager
            .ensure_server_tunnel_with_name(
                "server-1",
                "Survival SMP",
                25565,
                Some("Custom Display".into()),
            )
            .await
            .unwrap();
        assert_eq!(ensured.disposition, EnsureTunnelDisposition::Created);
        let created = created.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].3.as_deref(), Some("Custom Display"));
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
    async fn compensation_preserves_reused_and_updated_tunnels() {
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

    const MOCK_SIGNIN_OK: &str = concat!(
        r#"{"status":"success","data":{"session_key":"manager-key","auth":{"#,
        r#""update_version":1,"account_id":7,"timestamp":456,"#,
        r#""account_status":"verified","totp_status":{"status":"not-setup"},"#,
        r#""admin_id":null,"admin_review_id":null,"read_only":false,"show_admin":false}}}"#,
    );
    const MOCK_DETAILS_OK: &str = concat!(
        r#"{"status":"success","data":{"agent_type":"self-managed","#,
        r#""name":"panel-agent","remote_ip":"::1","version":"fixture"}}"#,
    );
    const MOCK_ACCEPT_OK: &str = concat!(
        r#"{"status":"success","data":{"agent_id":"#,
        r#""11111111-1111-1111-1111-111111111111"}}"#,
    );
    const MOCK_ACCEPT_REJECTED: &str = r#"{"status":"fail","data":"ClaimRejected"}"#;
    const MOCK_DELETE_OK: &str = r#"{"status":"success","data":null}"#;
    const MOCK_TUNNELS_EMPTY: &str = r#"{"status":"success","data":{"tunnels":[],"tcp_alloc":{"allowed":0,"claimed":0,"desired":0},"udp_alloc":{"allowed":0,"claimed":0,"desired":0}}}"#;

    async fn mock_api(bodies: Vec<String>) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for body in bodies {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut request = [0u8; 32 * 1024];
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(2_000),
                    stream.read(&mut request),
                )
                .await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        (base, task)
    }

    fn mock_session_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mcpanel-manager-{name}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn short_waits() -> SetupDirectOptions {
        SetupDirectOptions {
            details_timeout: std::time::Duration::from_secs(5),
            running_timeout: std::time::Duration::from_millis(300),
            poll_interval: std::time::Duration::from_millis(10),
        }
    }

    fn waiting_service() -> MockService {
        MockService {
            status: Arc::new(Mutex::new(ServiceStatus {
                phase: ServicePhase::WaitingForSecret,
                has_secret: false,
                ..ServiceStatus::default()
            })),
            lifecycle: Arc::new(Mutex::new(AgentLifecycle::WaitingForSecret)),
            claim: Arc::new(Mutex::new(ClaimResponse {
                claim_url: "https://playit.gg/claim/fixturecode".into(),
            })),
            ..MockService::default()
        }
    }

    #[test]
    fn claim_code_is_extracted_from_the_claim_url() {
        assert_eq!(
            claim_code_from_url("https://playit.gg/claim/abc123").unwrap(),
            "abc123"
        );
        assert!(claim_code_from_url("https://playit.gg/claim/").is_err());
        assert!(claim_code_from_url("").is_err());
        assert!(claim_code_from_url("https://playit.gg/claim/a b").is_err());
    }

    #[test]
    fn claim_agent_names_have_a_safe_default_and_limit() {
        assert_eq!(claim_agent_name(None).unwrap(), "mcpanel");
        assert_eq!(
            claim_agent_name(Some("  panel-1  ".into())).unwrap(),
            "panel-1"
        );
        assert!(claim_agent_name(Some("x".repeat(65))).is_err());
    }

    #[test]
    fn agent_ids_match_across_uuid_formatting() {
        assert!(ids_match("abc", "abc"));
        assert!(ids_match(
            "11111111-1111-1111-1111-111111111111",
            "11111111-1111-1111-1111-111111111111"
        ));
        assert!(ids_match(
            "11111111-1111-1111-1111-111111111111",
            "11111111111111111111111111111111"
        ));
        assert!(!ids_match(
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222"
        ));
    }

    #[tokio::test]
    async fn setup_direct_requires_a_login() {
        let manager = PlayitManager::with_service(waiting_service());
        let error = manager.setup_direct(None).await.unwrap_err();
        assert!(matches!(
            error,
            PlayitError::Account(crate::error::AccountError::NotLoggedIn)
        ));
    }

    #[tokio::test]
    async fn setup_direct_is_idempotent_when_configured() {
        let (base, task) = mock_api(vec![MOCK_SIGNIN_OK.to_owned()]).await;
        let service = running_service();
        service.account.lock().unwrap().agent_id =
            Some("11111111-1111-1111-1111-111111111111".into());
        let manager = PlayitManager::with_service_and_options(
            service,
            PlayitOptions {
                api_base: base,
                session_path: mock_session_path("idempotent"),
            },
        );
        manager
            .auth_login("user@example.com", "secret")
            .await
            .unwrap();
        // The only account call is the login: the configured runtime short-
        // circuits before any claim traffic.
        let result = manager.setup_direct(None).await.unwrap();
        assert!(result.already_configured);
        assert!(result.connected);
        assert_eq!(
            result.agent_id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        task.abort();
    }

    #[tokio::test]
    async fn setup_direct_claims_and_waits_for_running() {
        let (base, task) = mock_api(vec![
            MOCK_SIGNIN_OK.to_owned(),
            MOCK_DETAILS_OK.to_owned(),
            MOCK_ACCEPT_OK.to_owned(),
        ])
        .await;
        let service = waiting_service();
        let control = service.clone();
        let manager = PlayitManager::with_service_and_options(
            service,
            PlayitOptions {
                api_base: base,
                session_path: mock_session_path("setup"),
            },
        );
        manager
            .auth_login("user@example.com", "secret")
            .await
            .unwrap();

        // The runtime exchange finishing flips the lifecycle; the setup waits
        // for it instead of returning at accept time.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            *control.lifecycle.lock().unwrap() = AgentLifecycle::Running(Default::default());
        });
        let result = manager
            .setup_direct_with_options(None, short_waits())
            .await
            .unwrap();
        assert!(!result.already_configured);
        assert!(result.connected);
        assert_eq!(
            result.agent_id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        task.abort();
    }

    #[tokio::test]
    async fn setup_direct_reports_pending_when_exchange_is_delayed() {
        let (base, task) = mock_api(vec![
            MOCK_SIGNIN_OK.to_owned(),
            MOCK_DETAILS_OK.to_owned(),
            MOCK_ACCEPT_OK.to_owned(),
        ])
        .await;
        let manager = PlayitManager::with_service_and_options(
            waiting_service(),
            PlayitOptions {
                api_base: base,
                session_path: mock_session_path("delayed"),
            },
        );
        manager
            .auth_login("user@example.com", "secret")
            .await
            .unwrap();

        let result = manager
            .setup_direct_with_options(None, short_waits())
            .await
            .unwrap();
        assert!(!result.connected);
        assert_eq!(
            result.agent_id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        assert!(result.message.is_some());
        task.abort();
    }

    #[tokio::test]
    async fn setup_direct_surfaces_claim_rejection() {
        let (base, task) = mock_api(vec![
            MOCK_SIGNIN_OK.to_owned(),
            MOCK_DETAILS_OK.to_owned(),
            MOCK_ACCEPT_REJECTED.to_owned(),
        ])
        .await;
        let manager = PlayitManager::with_service_and_options(
            waiting_service(),
            PlayitOptions {
                api_base: base,
                session_path: mock_session_path("rejected"),
            },
        );
        manager
            .auth_login("user@example.com", "secret")
            .await
            .unwrap();

        let error = manager
            .setup_direct_with_options(None, short_waits())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("rejected"));
        task.abort();
    }

    #[tokio::test]
    async fn delete_agent_refuses_the_current_agent() {
        let service = running_service();
        service.account.lock().unwrap().agent_id =
            Some("11111111-1111-1111-1111-111111111111".into());
        let manager = PlayitManager::with_service(service);
        let error = manager
            .delete_agent(
                "11111111-1111-1111-1111-111111111111",
                &DeleteAgentOptions {
                    move_to_agent: None,
                    disable_tunnels: true,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, PlayitError::Conflict(_)));
    }

    #[tokio::test]
    async fn delete_agent_deletes_other_agents() {
        let (base, task) =
            mock_api(vec![MOCK_SIGNIN_OK.to_owned(), MOCK_DELETE_OK.to_owned()]).await;
        let service = running_service();
        service.account.lock().unwrap().agent_id =
            Some("11111111-1111-1111-1111-111111111111".into());
        let manager = PlayitManager::with_service_and_options(
            service,
            PlayitOptions {
                api_base: base,
                session_path: mock_session_path("delete"),
            },
        );
        manager
            .auth_login("user@example.com", "secret")
            .await
            .unwrap();
        manager
            .delete_agent(
                "22222222-2222-2222-2222-222222222222",
                &DeleteAgentOptions {
                    move_to_agent: Some("11111111-1111-1111-1111-111111111111".into()),
                    disable_tunnels: false,
                },
            )
            .await
            .unwrap();
        task.abort();
    }

    #[tokio::test]
    async fn disconnect_resets_the_secret_without_touching_the_session() {
        let (base, task) = mock_api(vec![MOCK_SIGNIN_OK.to_owned()]).await;
        let service = MockService {
            status: Arc::new(Mutex::new(ServiceStatus {
                phase: ServicePhase::Running,
                has_secret: true,
                ..ServiceStatus::default()
            })),
            lifecycle: Arc::new(Mutex::new(AgentLifecycle::Running(Default::default()))),
            ..MockService::default()
        };
        let resets = Arc::clone(&service.reset_calls);
        let manager = PlayitManager::with_service_and_options(
            service,
            PlayitOptions {
                api_base: base,
                session_path: mock_session_path("disconnect"),
            },
        );
        manager
            .auth_login("user@example.com", "secret")
            .await
            .unwrap();

        let status = manager.disconnect_agent().await.unwrap();
        assert_eq!(*resets.lock().unwrap(), 1);
        // The account session survives the agent disconnect.
        assert!(manager.account.is_logged_in().await);
        assert_eq!(status.status, PlayitConnectionState::Connected);
        task.abort();
    }

    #[tokio::test]
    async fn bearer_tunnels_win_over_the_runtime_list_when_logged_in() {
        let (base, task) = mock_api(vec![
            MOCK_SIGNIN_OK.to_owned(),
            MOCK_TUNNELS_EMPTY.to_owned(),
        ])
        .await;
        let service = MockService {
            account_tunnels_failure: Arc::new(Mutex::new(Some(AccountListFailure::Unavailable))),
            ..MockService::default()
        };
        let manager = PlayitManager::with_service_and_options(
            service,
            PlayitOptions {
                api_base: base,
                session_path: mock_session_path("preference"),
            },
        );
        manager
            .auth_login("user@example.com", "secret")
            .await
            .unwrap();
        // The runtime list would fail; the Bearer list succeeds instead.
        assert!(manager.account_tunnels().await.unwrap().is_empty());
        task.abort();
    }
}
