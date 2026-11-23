use clap::Args;

use crate::{app::App, error::Result};

#[derive(Clone, Debug, Args)]
pub struct StopArgs {
    /// Remove container-created home data before stopping.
    #[arg(long)]
    pub purge_home: bool,
}

impl StopArgs {
    pub fn run(&self, app: &App) -> Result<()> { app.stop_project(self.purge_home) }
}
