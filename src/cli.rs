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

    /// Subcommand to run; defaults to `list`.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Supported agentbox subcommands.
#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Start the project container; other commands only attach to it.
    Start {
        /// Attach to the selected agent after starting.
        #[arg(long)]
        attach: bool,

        /// Agent name to attach to; defaults to `general.default_agent`.
        #[arg(long, value_name = "NAME")]
        agent: Option<String>,
    },

    /// Stop the project container and clean temporary directories.
    Stop {
        /// Also remove the persistent home directory.
        #[arg(long)]
        purge_home: bool,
    },

    /// List agentbox-managed containers.
    #[command(aliases = ["ps", "ls"])]
    List,

    /// Attach to a shell session in the running container.
    Shell,

    /// Attach to a configured agent in the running container.
    Agent {
        /// Configured agent name.
        name: String,
    },

    /// Attach to the `OpenCode` server in the running container (alias: `oc`).
    #[command(alias = "oc")]
    Opencode,

    /// Attach to a Claude session in the running container.
    Claude,

    /// Attach to a Gemini session in the running container.
    Gemini,

    /// Print the host temporary directory for this project.
    Tmp,

    /// Print the persistent home directory for this project.
    Home,

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
    /// Write the minimal YAML configuration.
    Init {
        /// Overwrite an existing configuration file.
        #[arg(long)]
        force: bool,

        /// Write the complete template instead of the minimal default. The
        /// complete template documents every setting and all default mounts.
        #[arg(long)]
        full: bool,
    },

    /// Print the resolved configuration file path.
    Path,
}

impl Cli {
    /// Returns the selected command, defaulting to a container listing.
    pub fn command_or_default(&self) -> Command { self.command.clone().unwrap_or(Command::List) }

    /// Executes the selected command.
    pub fn run(&self) -> Result<()> {
        let home =
            env::var_os("HOME").map(PathBuf::from).with_context(|| error::HomeNotFoundSnafu)?;
        let config_path = Config::resolve_path(self.config.as_deref(), &home);

        match &self.command {
            Some(Command::Completions { shell }) => {
                let mut command = Self::command();
                generate(*shell, &mut command, "agentbox", &mut std::io::stdout());
                return Ok(());
            }
            Some(Command::Config { action }) => {
                return match action {
                    ConfigAction::Init { force, full } => {
                        Config::write_default(&config_path, *force, *full)
                    }
                    ConfigAction::Path => {
                        println!("{}", config_path.display());
                        Ok(())
                    }
                };
            }
            _ => {}
        }

        let config = Config::load(&config_path)?;
        let command = self.command_or_default();
        let identity = ProjectIdentity::resolve(&home, &config)?;
        let app = App::new(config, identity, home);

        match command {
            Command::Start { attach, agent } => app.start(agent.as_deref(), attach),
            Command::Stop { purge_home } => app.stop_project(purge_home),
            Command::List => app.list_containers(),
            Command::Agent { name } => app.launch_agent(&name),
            Command::Opencode => app.launch_agent("opencode"),
            Command::Claude => app.launch_agent("claude"),
            Command::Gemini => app.launch_agent("gemini"),
            Command::Shell => app.attach_shell(),
            Command::Tmp => {
                app.print_temporary_dir();
                Ok(())
            }
            Command::Home => {
                app.print_home_dir();
                Ok(())
            }
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
    fn defaults_to_list() {
        let cli = Cli::try_parse_from(["agentbox"]).expect("parse empty args");
        assert!(matches!(cli.command_or_default(), Command::List));
        assert_eq!(cli.verbose, 0);
    }

    #[test]
    fn default_ignores_config_option() {
        let cli = Cli::try_parse_from(["agentbox", "--config", "/tmp/agentbox.yaml"])
            .expect("parse config option");
        assert!(matches!(cli.command_or_default(), Command::List));
    }

    #[test]
    fn parses_start_and_agent_commands() {
        let start = Cli::try_parse_from(["agentbox", "start", "--attach", "--agent", "claude"])
            .expect("parse start");
        assert!(matches!(
            start.command,
            Some(Command::Start { attach: true, agent: Some(name) }) if name == "claude"
        ));

        let detached = Cli::try_parse_from(["agentbox", "start"]).expect("parse detached start");
        assert!(matches!(detached.command, Some(Command::Start { attach: false, agent: None })));

        let agent =
            Cli::try_parse_from(["agentbox", "agent", "gemini"]).expect("parse generic agent");
        assert!(matches!(agent.command, Some(Command::Agent { name }) if name == "gemini"));
    }

    #[test]
    fn parses_subcommand_aliases() {
        let oc = Cli::try_parse_from(["agentbox", "oc"]).expect("parse oc");
        assert!(matches!(oc.command, Some(Command::Opencode)));

        let list = Cli::try_parse_from(["agentbox", "ls"]).expect("parse ls");
        assert!(matches!(list.command, Some(Command::List)));
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
            cli.command,
            Some(Command::Completions { shell: clap_complete::Shell::Zsh })
        ));
    }

    #[test]
    fn parses_persistent_home_commands() {
        let home = Cli::try_parse_from(["agentbox", "home"]).expect("parse home");
        assert!(matches!(home.command, Some(Command::Home)));

        let stop = Cli::try_parse_from(["agentbox", "stop", "--purge-home"])
            .expect("parse stop with purge home");
        assert!(matches!(stop.command, Some(Command::Stop { purge_home: true })));

        let stop = Cli::try_parse_from(["agentbox", "stop"]).expect("parse stop");
        assert!(matches!(stop.command, Some(Command::Stop { purge_home: false })));
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
            "--full",
        ])
        .expect("parse config init");
        assert_eq!(cli.config, Some(std::path::PathBuf::from("/tmp/agentbox.yaml")));
        assert_eq!(cli.verbose, 2);
        assert!(matches!(
            cli.command,
            Some(Command::Config { action: ConfigAction::Init { force: true, full: true } })
        ));

        let minimal =
            Cli::try_parse_from(["agentbox", "config", "init"]).expect("parse minimal init");
        assert!(matches!(
            minimal.command,
            Some(Command::Config { action: ConfigAction::Init { force: false, full: false } })
        ));
    }
}
