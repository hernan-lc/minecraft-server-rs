//! A mockable Playit integration supporting the embedded runtime and optional
//! external daemon IPC.
//!
//! The panel talks to this crate rather than depending on Playit's wire models
//! or transport details directly.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod account;
pub mod client;
pub mod embedded;
pub mod error;
pub mod manager;
pub mod model;

pub use account::{AccountController, ACCOUNT_SESSION_FILE_NAME, DEFAULT_API_BASE};
pub use client::{IpcPlayitService, PlayitService};
pub use embedded::EmbeddedPlayitService;
pub use error::{AccountError, PlayitError};
pub use manager::{
    EnsureTunnelDisposition, EnsuredServerTunnel, PlayitManager, PlayitOptions, SetupDirectOptions,
};
pub use model::{
    AccountSessionState, AgentInfo, ClaimDetailsInfo, ClaimInfo, DeleteAgentOptions,
    DirectSetupResult, DomainInfo, PlayitAccount, PlayitAccountStatus, PlayitConnectionState,
    PlayitProtocol, PlayitStatus, PlayitTunnel, TunnelCreateInfo,
};
pub use playit_ipc::model::ServiceErrorCode;
