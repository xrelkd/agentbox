use clap::Args;

use crate::{app::App, cli::list::OutputFormat, error::Result};

#[derive(Clone, Debug, Default, Args)]
pub struct StatusArgs {
    /// Emit JSON instead of a key/value report.
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,

    /// Print only the project container name.
    #[arg(short, long, conflicts_with = "format")]
    pub quiet: bool,
}

impl StatusArgs {
    pub fn run(&self, app: &App) -> Result<()> {
        app.print_status(self.format.unwrap_or_default(), self.quiet)
    }
}
