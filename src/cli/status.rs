use clap::Args;

use super::list::OutputFormat;
use crate::{app::App, error::Result};

#[derive(Args, Clone, Debug, Default)]
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
