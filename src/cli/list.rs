use clap::Args;

use crate::{app::App, error::Result, output::OutputFormat};

#[derive(Clone, Debug, Default, Args)]
pub struct ListArgs {
    /// Emit JSON instead of a table.
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,

    /// Print only container names.
    #[arg(short, long, conflicts_with = "format")]
    pub quiet: bool,
}

impl ListArgs {
    pub fn run(&self, app: &App) -> Result<()> {
        app.list_containers(self.format.unwrap_or_default(), self.quiet)
    }
}
