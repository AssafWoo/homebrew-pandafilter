//! Agent installers — each agent gets its own sub-module.
//!
//! Adding a new agent: implement `AgentInstaller` and register in `get_installer()`.

pub mod cline;
pub mod copilot;
pub mod gemini;

/// Common interface for all agent hook installers.
pub trait AgentInstaller {
    /// Install CCR hooks for this agent. `ccr_bin` is the path to the CCR binary.
    fn install(&self, ccr_bin: &str) -> anyhow::Result<()>;
    /// Remove CCR hooks for this agent.
    fn uninstall(&self) -> anyhow::Result<()>;
    /// Display name for status messages.
    fn name(&self) -> &'static str;
}

/// Return the installer for the given agent name, or `None` if unrecognised.
pub fn get_installer(agent: &str) -> Option<Box<dyn AgentInstaller>> {
    match agent {
        "copilot" | "vscode" => Some(Box::new(copilot::CopilotInstaller)),
        "gemini" => Some(Box::new(gemini::GeminiInstaller)),
        "cline" => Some(Box::new(cline::ClineInstaller)),
        _ => None,
    }
}
