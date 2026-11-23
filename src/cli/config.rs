use std::io::Write;

use clap::{Args, Subcommand};

use crate::{
    cli::context::AppContext,
    config::{Config, DEFAULT_CONFIG_TEMPLATE, FULL_CONFIG_TEMPLATE},
    error::{Error, Result},
};

#[derive(Clone, Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Clone, Debug, Subcommand)]
pub enum ConfigAction {
    /// Write a config file.
    Init {
        /// Overwrite an existing config file.
        #[arg(long)]
        force: bool,

        /// Include the fully commented example config.
        #[arg(long)]
        full: bool,
    },

    /// Print the resolved config path.
    Path,

    /// Print the built-in config template without reading it from disk.
    Show {
        /// Show the fully commented example config.
        #[arg(long)]
        full: bool,
    },
}

impl ConfigArgs {
    pub fn run(&self, context: &AppContext) -> Result<()> {
        match &self.action {
            ConfigAction::Init { force, full } => {
                Config::write_default(&context.config_path, *force, *full)
            }
            ConfigAction::Path => {
                println!("{}", context.config_path.display());
                Ok(())
            }
            ConfigAction::Show { full } => {
                let template = if *full { FULL_CONFIG_TEMPLATE } else { DEFAULT_CONFIG_TEMPLATE };
                std::io::stdout()
                    .write_all(template)
                    .map_err(|source| Error::WriteStdout { source })?;
                Ok(())
            }
        }
    }
}
