mod app;
mod cli;
mod config;
mod error;
mod podman;
mod rootfs;
mod workspace;

use std::{env, path::PathBuf, process::ExitCode};

use clap::{CommandFactory, Parser};
use clap_complete::generate;
use tracing_subscriber::{EnvFilter, fmt};

use crate::{
    app::App,
    cli::{Cli, Command, ConfigAction},
    config::Config,
    error::{Error, Result},
    workspace::ProjectIdentity,
};

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(source) => {
            tracing::error!("{source}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(verbose: u8) {
    let directive = match env::var("RUST_LOG") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => match verbose {
            0 => "info".to_owned(),
            1 => "debug".to_owned(),
            _ => "trace".to_owned(),
        },
    };
    let filter = EnvFilter::new(directive);
    let _ = fmt().with_env_filter(filter).with_target(false).without_time().try_init();
}

fn run(cli: &Cli) -> Result<()> {
    let home = env::var_os("HOME").map(PathBuf::from).ok_or(Error::HomeNotFound)?;
    let config_path = Config::resolve_path(cli.config.as_deref(), &home);
    let command = cli.selected_command();

    if let Command::Completions { shell } = &command {
        let mut command = Cli::command();
        generate(*shell, &mut command, "agentbox", &mut std::io::stdout());
        return Ok(());
    }

    if let Command::Config { action } = &command {
        return match action {
            ConfigAction::Init { force } => Config::write_default(&config_path, *force),
            ConfigAction::Path => {
                println!("{}", config_path.display());
                Ok(())
            }
        };
    }

    let config = Config::load(&config_path)?;
    let identity = ProjectIdentity::resolve(&home, &config)?;
    let app = App::new(config, identity, home);

    match command {
        Command::Opencode => app.opencode(),
        Command::OcTui { session } => app.agent("oc-tui", session),
        Command::Claude { session } => app.agent("claude", session),
        Command::Gemini { session } => app.agent("gemini", session),
        Command::Shell { session } => app.shell(session),
        Command::Sessions => app.sessions(),
        Command::Tmp => {
            app.temporary_dir();
            Ok(())
        }
        Command::Stop => app.stop(),
        Command::List => app.list(),
        Command::Completions { .. } | Command::Config { .. } => Ok(()),
    }
}
