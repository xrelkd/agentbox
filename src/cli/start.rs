use clap::Args;

use crate::{app::App, error::Result};

#[derive(Clone, Debug, Args)]
pub struct StartArgs {
    /// Attach to the server after creating the session.
    #[arg(long)]
    pub attach: bool,

    /// Agent to launch (`general.default_agent` when omitted).
    #[arg(long, value_name = "NAME")]
    pub agent: Option<String>,
}

impl StartArgs {
    pub fn run(&self, app: &App) -> Result<()> { app.start(self.agent.as_deref(), self.attach) }
}
