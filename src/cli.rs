use std::{env, path::PathBuf};

use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use snafu::OptionExt;

use crate::{
    app::App,
    config::Config,
    error::{self, Result},
    workspace::ProjectIdentity,
};

/// Command-line interface for agentbox.
#[derive(Debug, Parser)]
#[command(
    name = "agentbox",
    version,
    about = "Zero-image Podman AI sandboxes for isolated agent sessions"
)]
pub struct Cli {
    /// Increase log verbosity; can be specified multiple times.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// Path to a YAML configuration file.
    #[arg(long, global = true, value_name = "PATH", env = "AGENTBOX_CONFIG")]
    pub config: Option<PathBuf>,

    /// Subcommand to run; defaults to `opencode`.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Supported agentbox subcommands.
#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Attach to the `OpenCode` server (default).
    #[command(alias = "oc")]
    Opencode,

    /// Start or attach an `OpenCode` TUI agent session.
    OcTui {
        /// Optional tmux session name.
        session: Option<String>,
    },

    /// Start or attach a Claude agent session.
    Claude {
        /// Optional tmux session name.
        session: Option<String>,
    },

    /// Start or attach a Gemini agent session.
    Gemini {
        /// Optional tmux session name.
        session: Option<String>,
    },

    /// Attach to a plain shell session.
    #[command(alias = "tmux")]
    Shell {
        /// Optional tmux session name.
        session: Option<String>,
    },

    /// List tmux sessions inside the project container.
    Sessions,

    /// Print the host temporary directory for this project.
    Tmp,

    /// Print the persistent home directory for this project.
    Home,

    /// Stop the project container and clean temporary directories.
    Stop {
        /// Also remove the persistent home directory.
        #[arg(long)]
        purge_home: bool,
    },

    /// List agentbox-managed containers.
    #[command(aliases = ["ps", "ls"])]
    List,

    /// Generate shell completions for agentbox.
    Completions {
        /// Target shell.
        shell: Shell,
    },

    /// Manage the agentbox configuration file.
    Config {
        /// Configuration action.
        #[command(subcommand)]
        action: ConfigAction,
    },
}

/// Configuration file actions.
#[derive(Clone, Debug, Subcommand)]
pub enum ConfigAction {
    /// Write the default YAML configuration.
    Init {
        /// Overwrite an existing configuration file.
        #[arg(long)]
        force: bool,
    },

    /// Print the resolved configuration file path.
    Path,
}

impl Cli {
    /// Returns the selected command, defaulting to `opencode`.
    #[inline]
    pub fn selected_command(&self) -> Command { self.command.clone().unwrap_or(Command::Opencode) }

    /// Executes the selected command.
    pub fn run(&self) -> Result<()> {
        let home =
            env::var_os("HOME").map(PathBuf::from).with_context(|| error::HomeNotFoundSnafu)?;
        let config_path = Config::resolve_path(self.config.as_deref(), &home);
        let command = self.selected_command();

        if let Command::Completions { shell } = &command {
            let mut command = Self::command();
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
            Command::Home => {
                app.home_dir();
                Ok(())
            }
            Command::Stop { purge_home } => app.stop(purge_home),
            Command::List => app.list(),
            Command::Completions { .. } | Command::Config { .. } => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use clap::{CommandFactory, Parser};

    use super::{Cli, Command, ConfigAction};

    #[test]
    fn defaults_to_opencode() {
        let cli = Cli::try_parse_from(["agentbox"]).expect("parse empty args");
        assert!(matches!(cli.selected_command(), Command::Opencode));
        assert_eq!(cli.verbose, 0);
    }

    #[test]
    fn parses_subcommand_aliases() {
        let oc = Cli::try_parse_from(["agentbox", "oc"]).expect("parse oc");
        assert!(matches!(oc.selected_command(), Command::Opencode));

        let shell = Cli::try_parse_from(["agentbox", "tmux", "work"]).expect("parse tmux");
        assert!(
            matches!(shell.selected_command(), Command::Shell { session } if session.as_deref() == Some("work"))
        );

        let list = Cli::try_parse_from(["agentbox", "ls"]).expect("parse ls");
        assert!(matches!(list.selected_command(), Command::List));
    }

    #[test]
    fn config_option_reads_env_variable() {
        let command = Cli::command();
        let config = command
            .get_arguments()
            .find(|argument| argument.get_long() == Some("config"))
            .expect("config argument should exist");
        assert_eq!(config.get_env(), Some(OsStr::new("AGENTBOX_CONFIG")));
    }

    #[test]
    fn parses_completions() {
        let cli =
            Cli::try_parse_from(["agentbox", "completions", "zsh"]).expect("parse completions");
        assert!(matches!(
            cli.selected_command(),
            Command::Completions { shell: clap_complete::Shell::Zsh }
        ));
    }

    #[test]
    fn parses_persistent_home_commands() {
        let home = Cli::try_parse_from(["agentbox", "home"]).expect("parse home");
        assert!(matches!(home.selected_command(), Command::Home));

        let stop = Cli::try_parse_from(["agentbox", "stop", "--purge-home"])
            .expect("parse stop with purge home");
        assert!(matches!(stop.selected_command(), Command::Stop { purge_home: true }));

        let stop = Cli::try_parse_from(["agentbox", "stop"]).expect("parse stop");
        assert!(matches!(stop.selected_command(), Command::Stop { purge_home: false }));
    }

    #[test]
    fn parses_global_config_and_config_init() {
        let cli = Cli::try_parse_from([
            "agentbox",
            "--config",
            "/tmp/agentbox.yaml",
            "-vv",
            "config",
            "init",
            "--force",
        ])
        .expect("parse config init");
        assert_eq!(cli.config, Some(std::path::PathBuf::from("/tmp/agentbox.yaml")));
        assert_eq!(cli.verbose, 2);
        assert!(matches!(
            cli.selected_command(),
            Command::Config { action: ConfigAction::Init { force: true } }
        ));
    }
}
