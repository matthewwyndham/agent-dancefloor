//! Provider adapters. They are deliberately read-only and tolerate missing
//! or partially-written client state.

pub mod claude;
pub mod codex;
pub mod pi;

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ProviderHomes {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub pi_agent: PathBuf,
    pub pi_sessions: PathBuf,
}

impl ProviderHomes {
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let codex = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let pi_agent = std::env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".pi/agent"));
        let pi_sessions = std::env::var_os("PI_CODING_AGENT_SESSION_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| pi_agent.join("sessions"));
        Self {
            claude: home.join(".claude"),
            codex,
            pi_agent,
            pi_sessions,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderWarning {
    pub provider: &'static str,
    pub message: String,
}
