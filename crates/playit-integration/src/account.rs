//! Direct playit.gg account sessions over HTTPS.
//!
//! The account session (a `Bearer` web session from email/password login) is
//! deliberately independent from the agent secret owned by the runtime:
//! logging out or expiring the account session never stops Minecraft tunnels,
//! and deleting the agent secret never needs an account session.
//!
//! Only the authenticated session is ever persisted — never the password
//! (dropped after the sign-in request) and never a TOTP code. Persistence
//! uses the [`FileSessionStore`](playit_api_client::session::FileSessionStore)
//! backend, which restricts the file to the owner on Unix; the panel also
//! applies its secret-file protection to the session file at startup, the
//! same treatment the agent secret receives.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use playit_api_client::api::{
    AccountStatus, AccountTunnel, AccountTunnelAllocation, ApiErrorNoFail, ApiResponseError,
    ApiResult, AuthError, DeleteError, PortType, ReqTunnelsList, TunnelOfflineReason, TunnelOrigin,
    TunnelType,
};
use playit_api_client::auth::{complete_totp, sign_in, AccountSession, SigninError, TotpError};
use playit_api_client::session::{FileSessionStore, PersistedSession, SessionStore};
use playit_api_client::web_api::{
    AgentDeleteMoveDetails, ClaimAcceptFail, ClaimDetailsFail, ClaimRejectFail, ReqAgentsDelete,
    TunnelsStrategy, WebApiError,
};
use playit_api_client::{PlayitApi, PlayitApiBuilder};
use tokio::sync::{Mutex, RwLock};

use crate::error::{AccountError, PlayitError};
use crate::model::{
    AccountSessionState, AgentInfo, ClaimDetailsInfo, DeleteAgentOptions, DomainInfo,
    PlayitProtocol, PlayitTunnel,
};

/// Default playit.gg API base URL for direct account operations.
pub const DEFAULT_API_BASE: &str = "https://api.playit.gg";

/// File name of the persisted account session next to the agent secret.
pub const ACCOUNT_SESSION_FILE_NAME: &str = "account-session.json";

/// How long a TOTP-pending login is kept in memory after `sign_in` reports
/// `TotpStatus::Required`.
const PENDING_LOGIN_TTL: Duration = Duration::from_secs(5 * 60);

/// Derive the default session path from the agent secret path:
/// `data/playit/secret.toml` pairs with `data/playit/account-session.json`.
pub fn session_path_for_secret(secret_path: &Path) -> PathBuf {
    secret_path.with_file_name(ACCOUNT_SESSION_FILE_NAME)
}

/// A live account login: the secret key plus the last-known safe metadata.
///
/// The key is intentionally never shown in `Debug` output.
struct LiveSession {
    session_key: String,
    api_base: String,
    account_id: u64,
    account_status: String,
    read_only: bool,
}

impl Clone for LiveSession {
    fn clone(&self) -> Self {
        Self {
            session_key: self.session_key.clone(),
            api_base: self.api_base.clone(),
            account_id: self.account_id,
            account_status: self.account_status.clone(),
            read_only: self.read_only,
        }
    }
}

impl std::fmt::Debug for LiveSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSession")
            .field("session_key", &"<redacted>")
            .field("api_base", &self.api_base)
            .field("account_id", &self.account_id)
            .field("account_status", &self.account_status)
            .field("read_only", &self.read_only)
            .finish()
    }
}

impl LiveSession {
    fn from_full(session: &AccountSession) -> Self {
        Self {
            session_key: session.session().session_key.clone(),
            api_base: session.api_base().to_owned(),
            account_id: session.account_id(),
            account_status: account_status_name(&session.session().auth.account_status),
            read_only: session.is_read_only(),
        }
    }

    fn client(&self) -> PlayitApi {
        PlayitApiBuilder::new(self.api_base.clone())
            .bearer(self.session_key.clone())
            .build()
    }

    fn state(&self) -> AccountSessionState {
        AccountSessionState {
            authenticated: true,
            requires_totp: false,
            account_id: Some(self.account_id),
            account_status: Some(self.account_status.clone()),
            read_only: self.read_only,
        }
    }
}

/// A login waiting for its TOTP code. Held only in memory with a short
/// expiry; never persisted.
struct PendingLogin {
    session: AccountSession,
    expires_at: Instant,
}

impl std::fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingLogin")
            .field("session", &self.session)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

struct AccountInner {
    api_base: String,
    store: FileSessionStore,
    live: RwLock<Option<LiveSession>>,
    pending: Mutex<Option<PendingLogin>>,
}

/// Direct playit.gg account operations: login, TOTP, sessions, claims,
/// agents, domains, and account-wide tunnels.
///
/// Cloning shares the same underlying login state.
pub struct AccountController {
    inner: std::sync::Arc<AccountInner>,
}

impl Clone for AccountController {
    fn clone(&self) -> Self {
        Self {
            inner: std::sync::Arc::clone(&self.inner),
        }
    }
}

impl std::fmt::Debug for AccountController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountController")
            .field("api_base", &self.inner.api_base)
            .field("session_path", &self.inner.store.path())
            // Locks are never held across the debug formatting, and the live
            // session itself redacts its key.
            .field(
                "live",
                &self
                    .inner
                    .live
                    .try_read()
                    .map(|live| live.is_some())
                    .unwrap_or(false),
            )
            .field(
                "has_pending",
                &self
                    .inner
                    .pending
                    .try_lock()
                    .map(|pending| pending.is_some())
                    .unwrap_or(false),
            )
            .finish()
    }
}

impl AccountController {
    /// Create a controller using `api_base` for account calls and
    /// `session_path` for the persisted session.
    pub fn new(api_base: impl Into<String>, session_path: impl Into<PathBuf>) -> Self {
        Self {
            inner: std::sync::Arc::new(AccountInner {
                api_base: api_base.into(),
                store: FileSessionStore::new(session_path.into()),
                live: RwLock::new(None),
                pending: Mutex::new(None),
            }),
        }
    }

    /// The API base URL used for new logins.
    pub fn api_base(&self) -> &str {
        &self.inner.api_base
    }

    /// The file the session is persisted to.
    pub fn session_path(&self) -> &Path {
        self.inner.store.path()
    }

    /// Sign in with email + password.
    ///
    /// Credentials travel only in the request body and are never stored,
    /// logged, or included in errors. When the account requires TOTP, the
    /// pending session is held in memory with a short expiry until
    /// [`Self::complete_totp`] succeeds; otherwise the session is persisted.
    pub async fn login(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AccountSessionState, PlayitError> {
        if email.trim().is_empty() || password.is_empty() {
            return Err(PlayitError::Account(AccountError::InvalidCredentials));
        }
        let session = sign_in(&self.inner.api_base, email, password)
            .await
            .map_err(map_signin_error)?;
        if session.requires_totp() {
            let state = AccountSessionState {
                authenticated: false,
                requires_totp: true,
                account_id: Some(session.account_id()),
                account_status: Some(account_status_name(&session.session().auth.account_status)),
                read_only: session.is_read_only(),
            };
            *self.inner.pending.lock().await = Some(PendingLogin {
                session,
                expires_at: Instant::now() + PENDING_LOGIN_TTL,
            });
            Ok(state)
        } else {
            self.adopt(session).await?;
            Ok(self.state().await)
        }
    }

    /// Submit the TOTP code for the pending login from [`Self::login`].
    ///
    /// The code travels only in the request body. A rejected code keeps the
    /// pending login so the operator can retry until it expires.
    pub async fn complete_totp(&self, code: &str) -> Result<AccountSessionState, PlayitError> {
        if code.trim().is_empty() {
            return Err(PlayitError::Account(AccountError::InvalidTotp));
        }
        let pending = self.inner.pending.lock().await.take();
        let Some(pending) = pending else {
            return Err(PlayitError::Account(AccountError::SessionExpired));
        };
        if Instant::now() > pending.expires_at {
            return Err(PlayitError::Account(AccountError::SessionExpired));
        }
        match complete_totp(&pending.session, code).await {
            Ok(session) => {
                self.adopt(session).await?;
                Ok(self.state().await)
            }
            Err(TotpError::InvalidCode) => {
                // Keep the pending login for a retry while it is still valid.
                if Instant::now() <= pending.expires_at {
                    *self.inner.pending.lock().await = Some(pending);
                }
                Err(PlayitError::Account(AccountError::InvalidTotp))
            }
            Err(TotpError::SessionExpired) => {
                Err(PlayitError::Account(AccountError::SessionExpired))
            }
            Err(error) => {
                if Instant::now() <= pending.expires_at {
                    *self.inner.pending.lock().await = Some(pending);
                }
                Err(PlayitError::Account(AccountError::Api(error.to_string())))
            }
        }
    }

    /// Delete the Bearer account session only. The agent secret and any
    /// running tunnels are untouched.
    pub async fn logout(&self) -> Result<(), PlayitError> {
        *self.inner.live.write().await = None;
        *self.inner.pending.lock().await = None;
        self.inner
            .store
            .clear()
            .await
            .map_err(|error| PlayitError::Unavailable(error.to_string()))?;
        Ok(())
    }

    /// Report the locally known session state without network access.
    pub async fn session_state(&self) -> AccountSessionState {
        self.state().await
    }

    /// Validate the session with a harmless account read. An expired session
    /// is cleared locally so later calls report logged-out instead of
    /// failing again; this never implies the agent secret is invalid.
    pub async fn validate(&self) -> Result<AccountSessionState, PlayitError> {
        let live = self.require_live().await?;
        match playit_api_client::web_api::validate_with_key(&live.api_base, &live.session_key).await
        {
            Ok(()) => Ok(live.state()),
            Err(WebApiError::SessionExpired) => {
                self.expire_live().await;
                Err(PlayitError::Account(AccountError::SessionExpired))
            }
            Err(error) => Err(PlayitError::Account(AccountError::Api(error.to_string()))),
        }
    }

    /// Whether an account session is currently available (restoring a
    /// persisted one best-effort, without network access).
    pub async fn is_logged_in(&self) -> bool {
        self.require_live().await.is_ok()
    }

    /// List the domains visible to the account.
    pub async fn domains(&self) -> Result<Vec<DomainInfo>, PlayitError> {
        let live = self.require_live().await?;
        let domains = live
            .client()
            .domains_list()
            .await
            .map_err(map_client_error)?;
        Ok(domains
            .domains
            .into_iter()
            .map(|domain| DomainInfo {
                id: domain.id.to_string(),
                name: domain.name,
            })
            .collect())
    }

    /// List the account-wide tunnels over Bearer auth. Returns `None` when
    /// no account session exists so the caller can fall back to the
    /// runtime/IPC view.
    pub async fn account_tunnels_if_logged_in(
        &self,
    ) -> Option<Result<Vec<PlayitTunnel>, PlayitError>> {
        let live = self.require_live().await.ok()?;
        let result = live
            .client()
            .tunnels_list(ReqTunnelsList {
                tunnel_id: None,
                agent_id: None,
            })
            .await;
        Some(match result {
            Ok(tunnels) => Ok(tunnels.tunnels.iter().map(bearer_tunnel_view).collect()),
            Err(error) => Err(self.map_session_error(error).await),
        })
    }

    /// Delete a tunnel as the account (`POST /tunnels/delete` with the
    /// Bearer web-session). This is the authority for the account
    /// dashboard and global tunnel management; server attach/detach uses the
    /// agent API instead. Never retried: this is destructive. A tunnel
    /// Playit no longer knows is reported as a "not found" rejection so
    /// callers can treat deletion as idempotent via
    /// [`PlayitError::is_not_found`].
    pub async fn delete_tunnel(&self, tunnel_id: &str) -> Result<(), PlayitError> {
        let live = self.require_live().await?;
        let tunnel_id = tunnel_id.trim();
        let id = uuid::Uuid::parse_str(tunnel_id).map_err(|_| {
            PlayitError::Account(AccountError::Api("invalid tunnel id format".into()))
        })?;
        match playit_api_client::web_api::delete_tunnel(&live.client(), id).await {
            Ok(ApiResult::Success(())) => Ok(()),
            Ok(ApiResult::Fail(DeleteError::TunnelNotFound)) => Err(PlayitError::Rejected(
                format!("Playit tunnel {tunnel_id} not found"),
            )),
            Ok(ApiResult::Error(error)) => Err(self.map_response_error(&error).await),
            Err(error) => Err(self.map_web_error(error).await),
        }
    }

    /// Look up a pending claim as the account.
    pub async fn claim_details(&self, code: &str) -> Result<ClaimDetailsInfo, PlayitError> {
        let live = self.require_live().await?;
        match playit_api_client::web_api::claim_details(&live.client(), code).await {
            Ok(ApiResult::Success(details)) => Ok(ClaimDetailsInfo {
                agent_type: claim_agent_type_name(details.agent_type),
                name: details.name,
                remote_ip: details.remote_ip,
                version: details.version,
            }),
            Ok(ApiResult::Fail(fail)) => Err(PlayitError::Account(AccountError::Api(
                claim_details_message(&fail).into(),
            ))),
            Ok(ApiResult::Error(error)) => Err(self.map_response_error(&error).await),
            Err(error) => Err(self.map_web_error(error).await),
        }
    }

    /// Approve a claim as the account, creating the agent. Returns the new
    /// agent id. Never retried: this creates a resource.
    pub async fn approve_claim(&self, code: &str, name: &str) -> Result<String, PlayitError> {
        use playit_api_client::api::ClaimAgentType;
        let live = self.require_live().await?;
        match playit_api_client::web_api::accept_claim(
            &live.client(),
            code,
            name,
            ClaimAgentType::SelfManaged,
        )
        .await
        {
            Ok(ApiResult::Success(accepted)) => Ok(accepted.agent_id.to_string()),
            Ok(ApiResult::Fail(fail)) => Err(PlayitError::Account(AccountError::Api(
                claim_accept_message(&fail).into(),
            ))),
            Ok(ApiResult::Error(error)) => Err(self.map_response_error(&error).await),
            Err(error) => Err(self.map_web_error(error).await),
        }
    }

    /// Reject a claim as the account. Never retried: this changes state.
    pub async fn reject_claim(&self, code: &str) -> Result<(), PlayitError> {
        let live = self.require_live().await?;
        match playit_api_client::web_api::reject_claim(&live.client(), code).await {
            Ok(ApiResult::Success(())) => Ok(()),
            Ok(ApiResult::Fail(fail)) => Err(PlayitError::Account(AccountError::Api(
                claim_reject_message(&fail).into(),
            ))),
            Ok(ApiResult::Error(error)) => Err(self.map_response_error(&error).await),
            Err(error) => Err(self.map_web_error(error).await),
        }
    }

    /// List the account's agents.
    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>, PlayitError> {
        let live = self.require_live().await?;
        match playit_api_client::web_api::list_agents(&live.client()).await {
            Ok(ApiResult::Success(list)) => Ok(list
                .agents
                .into_iter()
                .map(|agent| AgentInfo {
                    id: agent.id.to_string(),
                    name: agent.name,
                })
                .collect()),
            Ok(ApiResult::Fail(_)) => Err(PlayitError::Account(AccountError::Api(
                "the agents list was rejected".into(),
            ))),
            Ok(ApiResult::Error(error)) => Err(self.map_response_error(&error).await),
            Err(error) => Err(self.map_web_error(error).await),
        }
    }

    /// Delete an agent. The tunnel strategy is explicit: tunnels move to
    /// `move_to_agent` or are unassigned, optionally disabled. Never retried:
    /// this is destructive.
    pub async fn delete_agent(
        &self,
        agent_id: &str,
        options: &DeleteAgentOptions,
    ) -> Result<(), PlayitError> {
        let live = self.require_live().await?;
        let agent_id = parse_agent_id(agent_id)?;
        let move_to = parse_optional_agent_id(options.move_to_agent.as_deref())?;
        if Some(agent_id) == move_to {
            return Err(PlayitError::Account(AccountError::Api(
                "tunnels cannot be moved to the agent being deleted".into(),
            )));
        }
        let request = ReqAgentsDelete {
            agent_id,
            tunnels_strategy: TunnelsStrategy::MoveToAgent(AgentDeleteMoveDetails {
                agent_id: move_to,
                disable_tunnels: options.disable_tunnels,
            }),
        };
        match playit_api_client::web_api::delete_agent(&live.client(), request).await {
            Ok(ApiResult::Success(())) => Ok(()),
            Ok(ApiResult::Fail(_)) => Err(PlayitError::Account(AccountError::Api(
                "the agent deletion was rejected".into(),
            ))),
            Ok(ApiResult::Error(error)) => Err(self.map_response_error(&error).await),
            Err(error) => Err(self.map_web_error(error).await),
        }
    }

    /// Promote a fresh session to live and persist it, replacing any
    /// previous session. Clears any pending TOTP login.
    async fn adopt(&self, session: AccountSession) -> Result<(), PlayitError> {
        let persisted = PersistedSession::from_account_session(&session);
        *self.inner.live.write().await = Some(LiveSession::from_full(&session));
        *self.inner.pending.lock().await = None;
        self.inner
            .store
            .save(persisted)
            .await
            .map_err(|error| PlayitError::Unavailable(error.to_string()))?;
        Ok(())
    }

    /// The current live session, restoring a persisted one best-effort.
    async fn require_live(&self) -> Result<LiveSession, PlayitError> {
        if let Some(live) = self.inner.live.read().await.clone() {
            return Ok(live);
        }
        let restored = match self.inner.store.load().await {
            Ok(Some(persisted)) => persisted,
            Ok(None) => return Err(PlayitError::Account(AccountError::NotLoggedIn)),
            Err(error) => {
                tracing::warn!("stored Playit account session could not be read: {error}");
                return Err(PlayitError::Account(AccountError::NotLoggedIn));
            }
        };
        if restored.session_key.trim().is_empty() {
            return Err(PlayitError::Account(AccountError::NotLoggedIn));
        }
        let live = LiveSession {
            session_key: restored.session_key,
            api_base: if restored.api_base.trim().is_empty() {
                self.inner.api_base.clone()
            } else {
                restored.api_base
            },
            account_id: restored.account_id,
            // A restored snapshot has not been re-validated in this process,
            // so its status is honestly reported as unknown.
            account_status: "unknown".into(),
            read_only: false,
        };
        *self.inner.live.write().await = Some(live.clone());
        Ok(live)
    }

    async fn state(&self) -> AccountSessionState {
        match self.require_live().await {
            Ok(live) => live.state(),
            Err(_) => AccountSessionState {
                authenticated: false,
                requires_totp: false,
                account_id: None,
                account_status: None,
                read_only: false,
            },
        }
    }

    /// Clear an expired session locally (live + persisted). The agent secret
    /// is untouched: account and agent lifecycles are independent.
    async fn expire_live(&self) {
        *self.inner.live.write().await = None;
        if let Err(error) = self.inner.store.clear().await {
            tracing::warn!("expired Playit account session could not be cleared: {error}");
        }
    }

    async fn map_response_error(&self, error: &ApiResponseError) -> PlayitError {
        match error {
            ApiResponseError::Auth(AuthError::SessionExpired)
            | ApiResponseError::Auth(AuthError::AuthRequired)
            | ApiResponseError::Auth(AuthError::NoLongerValid)
            | ApiResponseError::Auth(AuthError::InvalidToken) => {
                self.expire_live().await;
                PlayitError::Account(AccountError::SessionExpired)
            }
            ApiResponseError::Auth(AuthError::GuestAccountNotAllowed)
            | ApiResponseError::Auth(AuthError::EmailMustBeVerified) => {
                PlayitError::Account(AccountError::Api(auth_error_message(error).into()))
            }
            other => PlayitError::Account(AccountError::Api(response_error_message(other))),
        }
    }

    async fn map_session_error<E: std::fmt::Display>(
        &self,
        error: ApiErrorNoFail<E>,
    ) -> PlayitError {
        match error {
            ApiErrorNoFail::UnexpectedFail => {
                PlayitError::Account(AccountError::Api("unexpected response".into()))
            }
            ApiErrorNoFail::ApiError(error) => self.map_response_error(&error).await,
            ApiErrorNoFail::ClientError(error) => {
                PlayitError::Account(AccountError::Api(format!("request failed: {error}")))
            }
        }
    }
}

fn map_signin_error(error: SigninError) -> PlayitError {
    match error {
        SigninError::EmptyCredentials => PlayitError::Account(AccountError::InvalidCredentials),
        SigninError::IncorrectCredentials => PlayitError::Account(AccountError::InvalidCredentials),
        SigninError::AccountBanned => {
            PlayitError::Account(AccountError::Api("the playit.gg account is banned".into()))
        }
        SigninError::Api(detail) | SigninError::Transport(detail) => {
            PlayitError::Account(AccountError::Api(detail))
        }
    }
}

fn map_client_error<E: std::fmt::Display>(error: ApiErrorNoFail<E>) -> PlayitError {
    match error {
        ApiErrorNoFail::UnexpectedFail => {
            PlayitError::Account(AccountError::Api("unexpected response".into()))
        }
        ApiErrorNoFail::ApiError(error) => match &error {
            ApiResponseError::Auth(AuthError::SessionExpired)
            | ApiResponseError::Auth(AuthError::AuthRequired)
            | ApiResponseError::Auth(AuthError::NoLongerValid)
            | ApiResponseError::Auth(AuthError::InvalidToken) => {
                PlayitError::Account(AccountError::SessionExpired)
            }
            other => PlayitError::Account(AccountError::Api(response_error_message(other))),
        },
        ApiErrorNoFail::ClientError(error) => {
            PlayitError::Account(AccountError::Api(format!("request failed: {error}")))
        }
    }
}

impl AccountController {
    /// Map a transport-level [`WebApiError`], expiring the local session when
    /// the server no longer accepts it.
    async fn map_web_error(&self, error: WebApiError) -> PlayitError {
        match error {
            WebApiError::SessionExpired => {
                self.expire_live().await;
                PlayitError::Account(AccountError::SessionExpired)
            }
            WebApiError::Api(detail) | WebApiError::Transport(detail) => {
                PlayitError::Account(AccountError::Api(detail))
            }
        }
    }
}

fn parse_agent_id(agent_id: &str) -> Result<uuid::Uuid, PlayitError> {
    uuid::Uuid::parse_str(agent_id.trim())
        .map_err(|_| PlayitError::Account(AccountError::Api("invalid agent id format".into())))
}

fn parse_optional_agent_id(agent_id: Option<&str>) -> Result<Option<uuid::Uuid>, PlayitError> {
    match agent_id {
        None => Ok(None),
        Some(agent_id) if agent_id.trim().is_empty() => Ok(None),
        Some(agent_id) => parse_agent_id(agent_id).map(Some),
    }
}

fn account_status_name(status: &AccountStatus) -> String {
    match status {
        AccountStatus::Guest => "guest".into(),
        AccountStatus::EmailNotVerified => "email-not-verified".into(),
        AccountStatus::Verified => "verified".into(),
    }
}

fn claim_agent_type_name(agent_type: playit_api_client::api::ClaimAgentType) -> String {
    use playit_api_client::api::ClaimAgentType;
    match agent_type {
        ClaimAgentType::SelfManaged => "self-managed".into(),
        ClaimAgentType::Assignable => "assignable".into(),
    }
}

fn claim_details_message(fail: &ClaimDetailsFail) -> &'static str {
    match fail {
        ClaimDetailsFail::AlreadyClaimed => "the claim was already accepted",
        ClaimDetailsFail::AlreadyRejected => "the claim was already rejected",
        ClaimDetailsFail::ClaimExpired => "the claim expired; start a new claim",
        ClaimDetailsFail::DifferentOwner => "the claim belongs to a different account",
        ClaimDetailsFail::WaitingForAgent => "the agent has not announced its claim yet",
        ClaimDetailsFail::InvalidCode => "the claim code is invalid or expired",
        ClaimDetailsFail::Other => "the claim lookup failed",
    }
}

fn claim_accept_message(fail: &ClaimAcceptFail) -> &'static str {
    match fail {
        ClaimAcceptFail::InvalidCode => "the claim code is invalid or expired",
        ClaimAcceptFail::AgentNotReady => "the agent is not ready; retry shortly",
        ClaimAcceptFail::CodeNotFound => "the claim code was not found",
        ClaimAcceptFail::InvalidAgentType => "the claim agent type is not supported",
        ClaimAcceptFail::ClaimAlreadyAccepted => "the claim was already accepted",
        ClaimAcceptFail::ClaimRejected => "the claim was rejected",
        ClaimAcceptFail::CodeExpired => "the claim expired; start a new claim",
        ClaimAcceptFail::InvalidName => "the agent name was rejected",
        ClaimAcceptFail::Other => "claim approval failed",
    }
}

fn claim_reject_message(fail: &ClaimRejectFail) -> &'static str {
    match fail {
        ClaimRejectFail::InvalidCode | ClaimRejectFail::CodeNotFound => {
            "the claim code is invalid or expired"
        }
        ClaimRejectFail::ClaimAccepted => "the claim was already accepted",
        ClaimRejectFail::ClaimAlreadyRejected => "the claim was already rejected",
        ClaimRejectFail::Other => "claim rejection failed",
    }
}

fn auth_error_message(error: &ApiResponseError) -> &'static str {
    match error {
        ApiResponseError::Auth(AuthError::GuestAccountNotAllowed) => {
            "guest accounts cannot use this operation"
        }
        ApiResponseError::Auth(AuthError::EmailMustBeVerified) => {
            "the account email must be verified first"
        }
        _ => "the account request was rejected",
    }
}

fn response_error_message(error: &ApiResponseError) -> String {
    match error {
        ApiResponseError::Validation(detail) => {
            format!("rejected: {}", detail_message(detail))
        }
        ApiResponseError::Internal(detail) => {
            format!("rejected: {}", detail_message(detail))
        }
        ApiResponseError::PathNotFound(_) => "unknown endpoint".into(),
        other => auth_error_message(other).into(),
    }
}

fn detail_message(detail: &impl std::fmt::Debug) -> String {
    let rendered = format!("{detail:?}");
    if rendered.len() > 200 {
        format!("{}…", &rendered[..200])
    } else {
        rendered
    }
}

/// Map a Bearer account tunnel onto the panel-facing tunnel view.
fn bearer_tunnel_view(tunnel: &AccountTunnel) -> PlayitTunnel {
    let (agent_id, local_address, local_port) = match &tunnel.origin {
        Some(TunnelOrigin::Agent(agent)) => (
            Some(agent.agent_id.to_string()),
            Some(agent.local_ip.to_string()),
            agent.local_port,
        ),
        Some(TunnelOrigin::Managed(managed)) => (Some(managed.agent_id.to_string()), None, None),
        None => (None, None, None),
    };
    let destination = match (&local_address, local_port) {
        (Some(address), Some(port)) if address.contains(':') => format!("[{address}]:{port}"),
        (Some(address), Some(port)) => format!("{address}:{port}"),
        _ => String::new(),
    };
    let (disabled, disabled_reason) = match &tunnel.alloc {
        AccountTunnelAllocation::Disabled(disabled) => {
            (true, Some(offline_reason_name(&disabled.reason)))
        }
        AccountTunnelAllocation::Pending => (true, Some("allocation pending".to_string())),
        AccountTunnelAllocation::Allocated(_) if !tunnel.active => (true, None),
        AccountTunnelAllocation::Allocated(_) => (false, None),
    };
    PlayitTunnel {
        id: tunnel.id.to_string(),
        name: tunnel.name.clone(),
        display_address: bearer_display_address(tunnel),
        destination,
        protocol: match tunnel.port_type {
            PortType::Tcp => PlayitProtocol::Tcp,
            PortType::Udp => PlayitProtocol::Udp,
            PortType::Both => PlayitProtocol::Both,
        },
        tunnel_type: tunnel.tunnel_type.map(tunnel_type_name),
        agent_id,
        local_address,
        local_port,
        disabled,
        disabled_reason,
    }
}

fn bearer_display_address(tunnel: &AccountTunnel) -> String {
    if let AccountTunnelAllocation::Allocated(alloc) = &tunnel.alloc {
        if alloc.port_end != alloc.port_start {
            return format!(
                "{}:{}-{}",
                alloc.assigned_domain, alloc.port_start, alloc.port_end
            );
        }
        return format!("{}:{}", alloc.assigned_domain, alloc.port_start);
    }
    if let Some(domain) = &tunnel.domain {
        return domain.name.clone();
    }
    tunnel.name.clone().unwrap_or_else(|| tunnel.id.to_string())
}

fn tunnel_type_name(tunnel_type: TunnelType) -> String {
    match tunnel_type {
        TunnelType::MinecraftJava => "minecraft-java".into(),
        TunnelType::MinecraftBedrock => "minecraft-bedrock".into(),
        TunnelType::Valheim => "valheim".into(),
        TunnelType::Terraria => "terraria".into(),
        TunnelType::Starbound => "starbound".into(),
        TunnelType::Rust => "rust".into(),
        TunnelType::Num7days => "7days".into(),
        TunnelType::Unturned => "unturned".into(),
        TunnelType::Https => "https".into(),
        TunnelType::Hytale => "hytale".into(),
        TunnelType::ProjectZomboid => "project-zomboid".into(),
        TunnelType::VintageStory => "vintage-story".into(),
    }
}

fn offline_reason_name(reason: &TunnelOfflineReason) -> String {
    match reason {
        TunnelOfflineReason::RequiresPremium => "requires premium".into(),
        TunnelOfflineReason::OverPortLimit => "over port limit".into(),
        TunnelOfflineReason::IpUsedInGre => "ip used in gre".into(),
        TunnelOfflineReason::PublicPortNotAvailable => "public port not available".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const SIGNIN_OK: &str = concat!(
        r#"{"status":"success","data":{"session_key":"live-key-1","auth":{"#,
        r#""update_version":1,"account_id":123,"timestamp":456,"#,
        r#""account_status":"verified","totp_status":{"status":"not-setup"},"#,
        r#""admin_id":null,"admin_review_id":null,"read_only":false,"show_admin":false}}}"#,
    );
    const SIGNIN_TOTP: &str = concat!(
        r#"{"status":"success","data":{"session_key":"pending-key","auth":{"#,
        r#""update_version":1,"account_id":123,"timestamp":456,"#,
        r#""account_status":"verified","totp_status":{"status":"required"},"#,
        r#""admin_id":null,"admin_review_id":null,"read_only":false,"show_admin":false}}}"#,
    );
    const SIGNIN_BAD: &str = r#"{"status":"fail","data":"IncorrectCredentials"}"#;
    const TOTP_OK: &str = concat!(
        r#"{"status":"success","data":{"session_key":"live-key-2","auth":{"#,
        r#""update_version":1,"account_id":123,"timestamp":456,"#,
        r#""account_status":"verified","totp_status":{"status":"signed","epoch_sec":789},"#,
        r#""admin_id":null,"admin_review_id":null,"read_only":false,"show_admin":false}}}"#,
    );
    const TOTP_BAD: &str = r#"{"status":"fail","data":"InvalidCode"}"#;
    const SESSION_EXPIRED: &str =
        r#"{"status":"error","data":{"type":"auth","message":"SessionExpired"}}"#;
    const DOMAINS_EMPTY: &str = r#"{"status":"success","data":{"domains":[]}}"#;
    const DETAILS_OK: &str = concat!(
        r#"{"status":"success","data":{"agent_type":"self-managed","#,
        r#""name":"fixture-agent","remote_ip":"::1","version":"fixture"}}"#,
    );
    const DETAILS_REJECTED: &str = r#"{"status":"fail","data":"AlreadyRejected"}"#;
    const ACCEPT_OK: &str = concat!(
        r#"{"status":"success","data":{"agent_id":"#,
        r#""00000000-0000-0000-0000-000000000001"}}"#,
    );
    const REJECT_OK: &str = r#"{"status":"success","data":null}"#;
    const AGENTS_OK: &str = concat!(
        r#"{"status":"success","data":{"agents":[{"id":"#,
        r#""00000000-0000-0000-0000-000000000002","name":"fixture-agent","#,
        r#""created_at":"2026-01-01T00:00:00Z","self_managed":true,"#,
        r#""status":{"state":"offline"},"routing":{"type":"Automatic"}}]}}"#,
    );
    const DELETE_OK: &str = r#"{"status":"success","data":null}"#;
    const ONE_TUNNEL: &str = concat!(
        r#"{"status":"success","data":{"tunnels":[{"#,
        r#""id":"00000000-0000-0000-0000-000000000010","tunnel_type":"minecraft-java","#,
        r#""created_at":"2026-01-01T00:00:00Z","name":"mcpanel:srv","port_type":"tcp","#,
        r#""port_count":1,"alloc":{"status":"allocated","data":{"#,
        r#""id":"00000000-0000-0000-0000-000000000011","ip_hostname":"ip.example","#,
        r#""static_ip4":"1.2.3.4","static_ip6":"::1","assigned_domain":"example.playit.gg","#,
        r#""assigned_srv":null,"tunnel_ip":"127.0.0.1","port_start":25565,"port_end":25565,"#,
        r#""assignment":{"type":"shared-ip"},"ip_type":"both","region":"global"}},"#,
        r#""origin":{"type":"agent","data":{"agent_id":"00000000-0000-0000-0000-000000000002","#,
        r#""agent_name":"fixture-agent","local_ip":"127.0.0.1","local_port":25565}},"#,
        r#""domain":null,"firewall_id":null,"ratelimit":{"bytes_per_second":null,"#,
        r#""packets_per_second":null},"active":true,"disabled_reason":null,"#,
        r#""region":"global","expire_notice":null,"proxy_protocol":null,"#,
        r#""hostname_verify_level":"None","agent_over_limit":false}],"#,
        r#""tcp_alloc":{"allowed":1,"claimed":1,"desired":1},"#,
        r#""udp_alloc":{"allowed":0,"claimed":0,"desired":0}}}"#,
    );

    /// Serve each body to one connection in order, ignoring request content.
    async fn mock_server(bodies: Vec<String>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for body in bodies {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut request = [0u8; 32 * 1024];
                let _ =
                    tokio::time::timeout(Duration::from_millis(2_000), stream.read(&mut request))
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

    fn scratch_session(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mcpanel-playit-account-{}-{}-{}.json",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ))
    }

    async fn cleanup(path: &Path) {
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn login_success_persists_and_restores() {
        let (base, task) = mock_server(vec![SIGNIN_OK.to_owned()]).await;
        let path = scratch_session("persist");
        let controller = AccountController::new(base, &path);

        let state = controller
            .login("user@example.com", "secret")
            .await
            .unwrap();
        assert!(state.authenticated);
        assert!(!state.requires_totp);
        assert_eq!(state.account_id, Some(123));
        assert_eq!(state.account_status.as_deref(), Some("verified"));
        assert!(path.exists());
        task.abort();

        // A new controller with the same path restores the session without
        // any network access: the mock server is gone.
        let restarted = AccountController::new("http://127.0.0.1:1".to_owned(), &path);
        let restored = restarted.session_state().await;
        assert!(restored.authenticated);
        assert_eq!(restored.account_id, Some(123));
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn login_rejects_bad_credentials_without_persisting() {
        let (base, task) = mock_server(vec![SIGNIN_BAD.to_owned()]).await;
        let path = scratch_session("bad");
        let controller = AccountController::new(base, &path);

        let error = controller
            .login("user@example.com", "wrong")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            PlayitError::Account(AccountError::InvalidCredentials)
        ));
        assert!(!path.exists());
        assert!(!controller.is_logged_in().await);
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn login_rejects_empty_credentials_without_network() {
        let path = scratch_session("empty");
        let controller = AccountController::new("http://127.0.0.1:1".to_owned(), &path);
        assert!(matches!(
            controller.login("", "secret").await.unwrap_err(),
            PlayitError::Account(AccountError::InvalidCredentials)
        ));
        assert!(matches!(
            controller.login("user@example.com", "").await.unwrap_err(),
            PlayitError::Account(AccountError::InvalidCredentials)
        ));
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn totp_flow_keeps_pending_for_retry_then_succeeds() {
        let (base, task) = mock_server(vec![
            SIGNIN_TOTP.to_owned(),
            TOTP_BAD.to_owned(),
            TOTP_OK.to_owned(),
        ])
        .await;
        let path = scratch_session("totp");
        let controller = AccountController::new(base, &path);

        let pending = controller
            .login("user@example.com", "secret")
            .await
            .unwrap();
        assert!(!pending.authenticated);
        assert!(pending.requires_totp);
        // Nothing is persisted until the TOTP step succeeds.
        assert!(!path.exists());

        let error = controller.complete_totp("000000").await.unwrap_err();
        assert!(matches!(
            error,
            PlayitError::Account(AccountError::InvalidTotp)
        ));
        assert!(!error.to_string().contains("000000"));

        let done = controller.complete_totp("123456").await.unwrap();
        assert!(done.authenticated);
        assert!(!done.requires_totp);
        assert!(path.exists());
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn totp_without_pending_login_fails() {
        let path = scratch_session("nopending");
        let controller = AccountController::new("http://127.0.0.1:1".to_owned(), &path);
        assert!(matches!(
            controller.complete_totp("123456").await.unwrap_err(),
            PlayitError::Account(AccountError::SessionExpired)
        ));
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn validate_expired_session_clears_it() {
        let (base, task) =
            mock_server(vec![SIGNIN_OK.to_owned(), SESSION_EXPIRED.to_owned()]).await;
        let path = scratch_session("expired");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();

        let error = controller.validate().await.unwrap_err();
        assert!(matches!(
            error,
            PlayitError::Account(AccountError::SessionExpired)
        ));
        // Expiry is sticky: later calls report logged-out, and the file is gone.
        assert!(!controller.is_logged_in().await);
        assert!(!path.exists());
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn logout_clears_live_and_persisted_session() {
        let (base, task) = mock_server(vec![SIGNIN_OK.to_owned()]).await;
        let path = scratch_session("logout");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();

        controller.logout().await.unwrap();
        assert!(!controller.is_logged_in().await);
        assert!(!path.exists());
        let state = controller.session_state().await;
        assert!(!state.authenticated);
        assert_eq!(state.account_id, None);
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn debug_output_never_exposes_the_session_key() {
        let (base, task) = mock_server(vec![SIGNIN_OK.to_owned()]).await;
        let path = scratch_session("redact");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();

        let live = controller.require_live().await.unwrap();
        let rendered = format!("{live:?}");
        assert!(!rendered.contains("live-key-1"), "{rendered}");
        let controller_rendered = format!("{controller:?}");
        assert!(
            !controller_rendered.contains("live-key-1"),
            "{controller_rendered}"
        );
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn bearer_tunnels_are_mapped_to_the_panel_view() {
        let (base, task) = mock_server(vec![SIGNIN_OK.to_owned(), ONE_TUNNEL.to_owned()]).await;
        let path = scratch_session("bearertunnels");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();

        let tunnels = controller
            .account_tunnels_if_logged_in()
            .await
            .expect("logged in")
            .unwrap();
        assert_eq!(tunnels.len(), 1);
        let tunnel = &tunnels[0];
        assert_eq!(tunnel.id, "00000000-0000-0000-0000-000000000010");
        assert_eq!(tunnel.name.as_deref(), Some("mcpanel:srv"));
        assert_eq!(tunnel.display_address, "example.playit.gg:25565");
        assert_eq!(tunnel.destination, "127.0.0.1:25565");
        assert_eq!(tunnel.protocol, PlayitProtocol::Tcp);
        assert_eq!(tunnel.tunnel_type.as_deref(), Some("minecraft-java"));
        assert_eq!(
            tunnel.agent_id.as_deref(),
            Some("00000000-0000-0000-0000-000000000002")
        );
        assert_eq!(tunnel.local_address.as_deref(), Some("127.0.0.1"));
        assert_eq!(tunnel.local_port, Some(25565));
        assert!(!tunnel.disabled);
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn logged_out_tunnels_report_no_session() {
        let path = scratch_session("nologin");
        let controller = AccountController::new("http://127.0.0.1:1".to_owned(), &path);
        assert!(controller.account_tunnels_if_logged_in().await.is_none());
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn agents_list_and_delete_with_strategy() {
        let (base, task) = mock_server(vec![
            SIGNIN_OK.to_owned(),
            AGENTS_OK.to_owned(),
            DELETE_OK.to_owned(),
        ])
        .await;
        let path = scratch_session("agents");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();

        let agents = controller.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "00000000-0000-0000-0000-000000000002");
        assert_eq!(agents[0].name, "fixture-agent");

        controller
            .delete_agent(
                "00000000-0000-0000-0000-000000000002",
                &DeleteAgentOptions {
                    move_to_agent: Some("00000000-0000-0000-0000-000000000003".into()),
                    disable_tunnels: true,
                },
            )
            .await
            .unwrap();
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn agent_delete_rejects_bad_ids_and_self_moves() {
        let (base, task) = mock_server(vec![SIGNIN_OK.to_owned()]).await;
        let path = scratch_session("agentdelete");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();

        let options = DeleteAgentOptions {
            move_to_agent: None,
            disable_tunnels: false,
        };
        assert!(controller
            .delete_agent("not-a-uuid", &options)
            .await
            .is_err());
        let self_move = DeleteAgentOptions {
            move_to_agent: Some("00000000-0000-0000-0000-000000000002".into()),
            disable_tunnels: false,
        };
        let error = controller
            .delete_agent("00000000-0000-0000-0000-000000000002", &self_move)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cannot be moved"));
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn claim_details_approve_and_reject() {
        let (base, task) = mock_server(vec![
            SIGNIN_OK.to_owned(),
            DETAILS_OK.to_owned(),
            ACCEPT_OK.to_owned(),
            REJECT_OK.to_owned(),
        ])
        .await;
        let path = scratch_session("claims");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();

        let details = controller.claim_details("fixture-code").await.unwrap();
        assert_eq!(details.agent_type, "self-managed");
        assert_eq!(details.name, "fixture-agent");

        let agent_id = controller
            .approve_claim("fixture-code", "panel-agent")
            .await
            .unwrap();
        assert_eq!(agent_id, "00000000-0000-0000-0000-000000000001");

        controller.reject_claim("other-code").await.unwrap();
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn claim_details_failure_maps_to_safe_message() {
        let (base, task) =
            mock_server(vec![SIGNIN_OK.to_owned(), DETAILS_REJECTED.to_owned()]).await;
        let path = scratch_session("claimfail");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();

        let error = controller.claim_details("gone-code").await.unwrap_err();
        assert!(error.to_string().contains("already rejected"));
        assert!(!error.to_string().contains("gone-code"));
        task.abort();
        cleanup(&path).await;
    }

    #[tokio::test]
    async fn domains_list_returns_empty() {
        let (base, task) = mock_server(vec![SIGNIN_OK.to_owned(), DOMAINS_EMPTY.to_owned()]).await;
        let path = scratch_session("domains");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();
        assert!(controller.domains().await.unwrap().is_empty());
        task.abort();
        cleanup(&path).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn persisted_session_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (base, task) = mock_server(vec![SIGNIN_OK.to_owned()]).await;
        let path = scratch_session("perms");
        let controller = AccountController::new(base, &path);
        controller
            .login("user@example.com", "secret")
            .await
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "session file must be owner-only");
        task.abort();
        cleanup(&path).await;
    }
}
