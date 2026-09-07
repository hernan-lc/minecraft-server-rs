//! Errors returned by the Playit adapter.

use playit_ipc::ipc::IpcError;
use playit_ipc::model::ServiceErrorCode;
use playit_runtime::RuntimeError;

/// A failed Playit operation.
#[derive(Debug, thiserror::Error)]
pub enum PlayitError {
    /// The external daemon could not be reached or completed the IPC exchange.
    #[error("Playit IPC error: {0}")]
    Ipc(#[from] IpcError),
    /// The embedded runtime failed while performing the operation.
    #[error("Playit runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    /// Playit is not currently available to the panel.
    #[error("Playit integration unavailable: {0}")]
    Unavailable(String),
    /// Playit answered, but did not accept the requested command.
    #[error("Playit rejected the request: {0}")]
    Rejected(String),
    /// Playit returned a response that cannot be used safely.
    #[error("invalid Playit response: {0}")]
    Protocol(String),
    /// The requested operation would be ambiguous or conflict with an
    /// existing account or tunnel association.
    #[error("Playit conflict: {0}")]
    Conflict(String),
    /// A direct playit.gg account operation failed. The message is safe to
    /// surface to an admin: it never contains passwords, TOTP codes, session
    /// keys, or agent secrets.
    #[error(transparent)]
    Account(#[from] AccountError),
}

/// Why a direct playit.gg account operation failed.
///
/// Every variant renders without credentials. Account sessions and agent
/// secrets have independent lifecycles: an expired or logged-out account
/// session never implies the agent secret is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountError {
    /// No account session is available; sign in first.
    NotLoggedIn,
    /// The email/password combination was rejected.
    InvalidCredentials,
    /// A TOTP code was rejected. The pending login is retained until it
    /// expires so the operator can retry.
    InvalidTotp,
    /// The account session (or the pending TOTP login) is no longer valid;
    /// sign in again.
    SessionExpired,
    /// The account API answered with a structured or transport failure.
    /// The detail is operator-safe: the client library redacts session keys
    /// and never echoes passwords or TOTP codes.
    Api(String),
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLoggedIn => {
                write!(f, "no Playit account session; sign in first")
            }
            Self::InvalidCredentials => write!(f, "incorrect Playit email or password"),
            Self::InvalidTotp => write!(f, "the Playit TOTP code was rejected"),
            Self::SessionExpired => {
                write!(f, "Playit account session expired; sign in again")
            }
            Self::Api(detail) => write!(f, "Playit account request failed: {detail}"),
        }
    }
}

impl std::error::Error for AccountError {}

impl PlayitError {
    /// Return the structured service code, when the backend supplied one.
    pub fn service_code(&self) -> Option<ServiceErrorCode> {
        match self {
            Self::Ipc(playit_ipc::ipc::IpcError::Service(error)) => Some(error.code.clone()),
            Self::Runtime(error) => Some(error.as_service_error().code),
            Self::Rejected(_) => Some(ServiceErrorCode::ApiRejected),
            Self::Ipc(_)
            | Self::Unavailable(_)
            | Self::Protocol(_)
            | Self::Conflict(_)
            | Self::Account(_) => None,
        }
    }

    /// Whether this error means the Playit service is currently unavailable.
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Ipc(
                IpcError::ConnectionFailed(_) | IpcError::NotRunning | IpcError::IoError(_),
            )
            | Self::Unavailable(_) => true,
            Self::Ipc(IpcError::Service(error)) => service_code_is_unavailable(&error.code),
            Self::Runtime(error) => runtime_error_is_unavailable(error),
            Self::Rejected(_)
            | Self::Protocol(_)
            | Self::Conflict(_)
            | Self::Account(_)
            | Self::Ipc(_) => false,
        }
    }

    /// Whether the backend and this panel disagree about the protocol.
    pub fn is_unsupported(&self) -> bool {
        match self {
            Self::Ipc(IpcError::ProtocolMismatch { .. }) => true,
            Self::Ipc(IpcError::Service(error)) => {
                matches!(error.code, ServiceErrorCode::UnsupportedProtocol)
            }
            Self::Runtime(error) => {
                runtime_error_has_code(error, ServiceErrorCode::UnsupportedProtocol)
            }
            _ => false,
        }
    }

    /// Whether the remote operation is already satisfied because the tunnel
    /// no longer exists.
    pub fn is_not_found(&self) -> bool {
        if matches!(self.service_code(), Some(ServiceErrorCode::TunnelNotFound)) {
            return true;
        }
        // Older Playit daemons sometimes returned a human-readable rejection
        // before TunnelNotFound was added to the IPC error codes. Deletion is
        // idempotent, so recognize only the narrow missing-resource wording
        // and leave all other rejections visible to the caller.
        match self {
            Self::Rejected(message) => {
                let message = message.to_ascii_lowercase();
                message.contains("not found")
                    || message.contains("does not exist")
                    || message.contains("unknown tunnel")
            }
            _ => false,
        }
    }
}

fn runtime_error_is_unavailable(error: &RuntimeError) -> bool {
    matches!(error, RuntimeError::Stopped | RuntimeError::Io(_))
        || matches!(
            error.as_service_error().code,
            ServiceErrorCode::ApiUnavailable | ServiceErrorCode::ProvisioningUnavailable
        )
}

fn service_code_is_unavailable(code: &ServiceErrorCode) -> bool {
    matches!(
        code,
        ServiceErrorCode::ApiUnavailable | ServiceErrorCode::ProvisioningUnavailable
    )
}

fn runtime_error_has_code(error: &RuntimeError, expected: ServiceErrorCode) -> bool {
    let code = match error {
        RuntimeError::Secret { code, .. }
        | RuntimeError::Setup { code, .. }
        | RuntimeError::Api { code, .. }
        | RuntimeError::InvalidState { code, .. } => code,
        RuntimeError::Io(_) | RuntimeError::Stopped => return false,
    };
    match expected {
        ServiceErrorCode::ApiUnavailable => matches!(code, ServiceErrorCode::ApiUnavailable),
        ServiceErrorCode::UnsupportedProtocol => {
            matches!(code, ServiceErrorCode::UnsupportedProtocol)
        }
        _ => false,
    }
}
