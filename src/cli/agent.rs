use clap::Args;

use crate::{app::App, error::Result};

#[derive(Clone, Debug, Args)]
pub struct AgentArgs {
    /// Configured agent name (`general.default_agent` when omitted).
    pub name: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub struct PickArgs {}

#[derive(Clone, Debug, Args)]
pub struct OpencodeArgs {}

#[derive(Clone, Debug, Args)]
pub struct ClaudeArgs {}

#[derive(Clone, Debug, Args)]
pub struct GeminiArgs {}

impl AgentArgs {
    pub fn run(&self, app: &App) -> Result<()> { app.launch_agent_named(self.name.as_deref()) }
}

impl PickArgs {
    pub fn run(app: &App) -> Result<()> { app.print_agent_choice() }
}

impl OpencodeArgs {
    pub fn run(app: &App) -> Result<()> { app.launch_agent("opencode") }
}

impl ClaudeArgs {
    pub fn run(app: &App) -> Result<()> { app.launch_agent("claude") }
}

impl GeminiArgs {
    pub fn run(app: &App) -> Result<()> { app.launch_agent("gemini") }
}
