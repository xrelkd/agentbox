use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};
use clap_complete::Shell;

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
    #[arg(long, global = true, value_name = "PATH")]
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

    /// Stop the project container and clean temporary directories.
    Stop,

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
}

#[cfg(test)]
mod tests {
    use clap::Parser;

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
    fn parses_completions() {
        let cli =
            Cli::try_parse_from(["agentbox", "completions", "zsh"]).expect("parse completions");
        assert!(matches!(
            cli.selected_command(),
            Command::Completions { shell: clap_complete::Shell::Zsh }
        ));
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
