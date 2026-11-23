use clap::{Args, ValueEnum};

use crate::{app::App, error::Result};

#[derive(Args, Clone, Debug, Default)]
pub struct ListArgs {
    /// Emit JSON instead of a table.
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,

    /// Print only container names.
    #[arg(short, long, conflicts_with = "format")]
    pub quiet: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
}

impl ListArgs {
    pub fn run(&self, app: &App) -> Result<()> {
        app.list_containers(self.format.unwrap_or_default(), self.quiet)
    }
}
