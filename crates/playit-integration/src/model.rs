//! Panel-facing Playit models.

use serde::{Deserialize, Serialize};

/// The availability of the local Playit service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayitConnectionState {
    /// The Playit service is running with a configured account.
    Connected,
    /// The Playit service is running but needs account setup through the claim flow.
    NeedsClaim,
    /// The Playit service is starting.
    Starting,
    /// The Playit control connection is recovering while the service remains alive.
    Reconnecting,
    /// The Playit service is stopping.
    Stopping,
    /// No usable Playit service is available.
    Unavailable,
    /// An external Playit daemon speaks an incompatible IPC protocol.
    Unsupported,
    /// The Playit service reported an operational error.
    Error,
}

/// A safe summary of Playit's local service state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayitStatus {
    /// The state a panel UI should display.
    pub status: PlayitConnectionState,
    /// The Playit service version, when it could be read.
    pub version: Option<String>,
    /// A human-readable diagnostic, when one is available.
    pub message: Option<String>,
}

/// The account state reported by Playit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayitAccountStatus {
    /// The account state is not known yet.
    Unknown,
    /// A guest or not-yet-claimed account.
    Guest,
    /// The account has an email verification pending.
    EmailNotVerified,
    /// The account is ready for normal use.
    Verified,
}

/// Account information that is safe to send to the panel UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayitAccount {
    /// The account state, without exposing the account secret.
    pub status: PlayitAccountStatus,
    /// The Playit agent's public identifier.
    pub agent_id: Option<String>,
    /// A login link supplied by Playit, if any.
    pub login_link: Option<String>,
    /// A claim link supplied by Playit, if any.
    pub claim_url: Option<String>,
}

/// The URL the operator should open to claim/configure Playit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimInfo {
    /// The Playit claim URL.
    pub claim_url: String,
}

/// Supported transport protocols for a tunnel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayitProtocol {
    /// TCP, which is the protocol used by Java Minecraft.
    #[default]
    Tcp,
    /// UDP, useful for future Bedrock support.
    Udp,
    /// Both TCP and UDP.
    Both,
}

/// A tunnel known to the Playit service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayitTunnel {
    /// Stable Playit tunnel identifier.
    pub id: String,
    /// Optional operator-facing name.
    pub name: Option<String>,
    /// Public address players can use.
    pub display_address: String,
    /// Destination represented by the Playit service.
    pub destination: String,
    /// Transport protocol.
    pub protocol: PlayitProtocol,
    /// Playit's semantic tunnel type, when the account API supplies one.
    pub tunnel_type: Option<String>,
    /// Agent currently assigned to the tunnel, when known.
    pub agent_id: Option<String>,
    /// Local bind address, when supplied by the Playit service.
    pub local_address: Option<String>,
    /// Local destination port, when supplied by the Playit service.
    pub local_port: Option<u16>,
    /// Whether Playit has disabled this tunnel.
    pub disabled: bool,
    /// Why the tunnel is disabled, when supplied.
    pub disabled_reason: Option<String>,
}

/// The immediate result of creating a tunnel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelCreateInfo {
    /// Stable Playit tunnel identifier.
    pub tunnel_id: String,
    /// Optional Playit service message.
    pub message: Option<String>,
}

/// Which authority a tunnel list was read from.
///
/// Agent and account operations are deliberately separate: server attach,
/// detach, reconciliation, and managed Minecraft tunnels use the agent
/// secret, while the account dashboard and global tunnel management use the
/// Bearer web-session. Mixing the two made the same tunnel manageable
/// by two different authorities.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelSource {
    /// Tunnels read through the local agent (agent-secret authority).
    Agent,
    /// Tunnels read through the playit.gg account session (Bearer authority).
    Account,
    /// No tunnel source is currently usable (setup pending, logged out with
    /// a stopped agent, ...). This is a normal state, not an error.
    #[default]
    None,
}

/// An explicit tunnel listing together with the authority it came from.
///
/// Normal pre-running states (secret provisioning, waiting claim,
/// starting, stopping) report `available: false` with an empty list
/// instead of failing: only broken IPC, a runtime crash, an unexpected
/// internal failure, or a playit.gg API outage is an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelCatalog {
    /// Whether the listing reflects a live tunnel source.
    pub available: bool,
    /// The authority the tunnels were read from.
    pub source: TunnelSource,
    /// The tunnels visible through that authority (empty when unavailable).
    pub tunnels: Vec<PlayitTunnel>,
}

impl TunnelCatalog {
    /// The normal pre-running catalog: no live source, no tunnels, no
    /// error. Only the pre-running lifecycles may use this; real failures
    /// must propagate instead of hiding behind it.
    pub fn unavailable() -> Self {
        Self {
            available: false,
            source: TunnelSource::None,
            tunnels: Vec::new(),
        }
    }
}

/// Whether the runtime agent is owned by the logged-in playit.gg account.
///
/// Automatic repair, tunnel creation, and reconciliation are only safe when
/// the agent is [`AgentOwnership::Matched`]: acting on another account's
/// agent would corrupt tunnel state that belongs elsewhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOwnership {
    /// The runtime agent id is owned by the logged-in account.
    Matched,
    /// The runtime agent belongs to a different account than the login.
    /// Automatic repair, tunnel creation, and reconciliation are blocked.
    DifferentAccount,
    /// The runtime has no agent id yet (missing secret / waiting claim).
    NoAgent,
    /// Ownership cannot be verified (not logged in, or the account agent
    /// list could not be read). Agent-only operation remains allowed.
    #[default]
    Unknown,
}

/// The verified relationship between the runtime agent and the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOwnershipInfo {
    /// The verified relationship.
    pub ownership: AgentOwnership,
    /// The runtime agent id, when one is known.
    pub agent_id: Option<String>,
}

/// The safe account-session state returned by the direct-login endpoints.
///
/// This never contains the session key, the password, or a TOTP code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSessionState {
    /// Whether an account session (or a pending TOTP login) is active.
    pub authenticated: bool,
    /// Whether a TOTP code must still be submitted for the pending login.
    pub requires_totp: bool,
    /// The playit.gg account id, when a session or pending login exists.
    pub account_id: Option<u64>,
    /// The account status reported by playit.gg (`verified`,
    /// `email-not-verified`, `guest`) or `unknown` for a session restored
    /// from disk that has not been re-validated yet.
    pub account_status: Option<String>,
    /// Whether the session token is marked read-only.
    pub read_only: bool,
}

/// A playit.gg agent owned by the logged-in account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Stable agent identifier.
    pub id: String,
    /// Operator-facing agent name.
    pub name: String,
}

/// The tunnel strategy for deleting a playit.gg agent.
///
/// Deletion never silently drops tunnels: they are either moved to another
/// agent or explicitly unassigned, with optional disabling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteAgentOptions {
    /// Move the deleted agent's tunnels to this agent, or unassign them
    /// when `None`.
    pub move_to_agent: Option<String>,
    /// Disable the affected tunnels instead of leaving them enabled.
    pub disable_tunnels: bool,
}

/// What the account side sees for a pending machine claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimDetailsInfo {
    /// The agent type the runtime registered (`self-managed` normally).
    pub agent_type: String,
    /// The agent name proposed by the claiming machine.
    pub name: String,
    /// The remote IP observed by playit.gg.
    pub remote_ip: String,
    /// The agent version reported by the claiming machine.
    pub version: String,
}

/// The outcome of the single-call browserless agent setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectSetupResult {
    /// The agent created (or already configured), when known.
    pub agent_id: Option<String>,
    /// Whether the runtime was already configured and no claim ran.
    pub already_configured: bool,
    /// Whether the agent lifecycle reached Running.
    pub connected: bool,
    /// A human-readable note, e.g. why the agent is not connected yet.
    pub message: Option<String>,
}

/// A domain visible to the logged-in playit.gg account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainInfo {
    /// Stable domain identifier.
    pub id: String,
    /// The domain name.
    pub name: String,
}
