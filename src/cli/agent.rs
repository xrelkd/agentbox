use clap::Args;

use crate::{app::App, error::Result};

#[derive(Args, Clone, Debug)]
pub struct AgentArgs {
    /// Configured agent name (`general.default_agent` when omitted).
    pub name: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct PickArgs {}

#[derive(Args, Clone, Debug)]
pub struct OpencodeArgs {}

#[derive(Args, Clone, Debug)]
pub struct ClaudeArgs {}

#[derive(Args, Clone, Debug)]
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
