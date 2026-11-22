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

    /// Subcommand to run; defaults to `general.default_agent`.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Supported agentbox subcommands.
#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Start the project container, optionally attaching to an agent.
    Start {
        /// Attach to the selected agent after starting.
        #[arg(long)]
        attach: bool,

        /// Agent name to attach to; defaults to `general.default_agent`.
        #[arg(long, value_name = "NAME")]
        agent: Option<String>,

        /// Optional session name for multiplexed backends.
        session: Option<String>,
    },

    /// Start or attach a session for any configured agent by name.
    Agent {
        /// Configured agent name.
        name: String,

        /// Optional session name for multiplexed backends.
        session: Option<String>,
    },

    /// Attach to the `OpenCode` server (alias for `agent opencode`).
    #[command(alias = "oc")]
    Opencode,

    /// Start or attach an `OpenCode` TUI agent session.
    OcTui {
        /// Optional session name for multiplexed backends.
        session: Option<String>,
    },

    /// Start or attach a Claude agent session.
    Claude {
        /// Optional session name for multiplexed backends.
        session: Option<String>,
    },

    /// Start or attach a Gemini agent session.
    Gemini {
        /// Optional session name for multiplexed backends.
        session: Option<String>,
    },

    /// Attach to a plain shell session.
    #[command(alias = "tmux")]
    Shell {
        /// Optional session name for multiplexed backends.
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
    /// Returns the command implied by the configuration when none was given.
    pub fn command_or_default(&self, config: &Config) -> Command {
        if let Some(command) = &self.command {
            return command.clone();
        }
        let name = config.general.default_agent.as_str();
        if name == "shell" {
            return Command::Shell { session: None };
        }
        let has_server = config.agents.get(name).is_some_and(|agent| agent.serve.is_some());
        if has_server {
            return Command::Start { attach: true, agent: Some(name.to_owned()), session: None };
        }
        Command::Agent { name: name.to_owned(), session: None }
    }

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
                    ConfigAction::Init { force } => Config::write_default(&config_path, *force),
                    ConfigAction::Path => {
                        println!("{}", config_path.display());
                        Ok(())
                    }
                };
            }
            _ => {}
        }

        let config = Config::load(&config_path)?;
        let command = self.command_or_default(&config);
        let identity = ProjectIdentity::resolve(&home, &config)?;
        let app = App::new(config, identity, home);

        match command {
            Command::Start { attach, agent, session } => {
                app.start(agent.as_deref(), session, attach)
            }
            Command::Agent { name, session } => app.launch(&name, session),
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
    use crate::config::Config;

    #[test]
    fn defaults_to_configured_shell() {
        let cli = Cli::try_parse_from(["agentbox"]).expect("parse empty args");
        assert!(matches!(
            cli.command_or_default(&Config::default()),
            Command::Shell { session: None }
        ));
        assert_eq!(cli.verbose, 0);
    }

    #[test]
    fn defaults_to_configured_agent_server() {
        let mut config = Config::default();
        config.general.default_agent = "opencode".into();
        let cli = Cli::try_parse_from(["agentbox"]).expect("parse empty args");
        assert!(matches!(
            cli.command_or_default(&config),
            Command::Start { attach: true, agent: Some(name), session: None } if name == "opencode"
        ));
    }

    #[test]
    fn parses_start_and_agent_commands() {
        let start =
            Cli::try_parse_from(["agentbox", "start", "--attach", "--agent", "claude", "review"])
                .expect("parse start");
        assert!(matches!(
            start.command,
            Some(Command::Start { attach: true, agent: Some(name), session: Some(session) })
                if name == "claude" && session == "review"
        ));

        let detached = Cli::try_parse_from(["agentbox", "start"]).expect("parse detached start");
        assert!(matches!(
            detached.command,
            Some(Command::Start { attach: false, agent: None, session: None })
        ));

        let agent =
            Cli::try_parse_from(["agentbox", "agent", "gemini"]).expect("parse generic agent");
        assert!(matches!(
            agent.command,
            Some(Command::Agent { name, session: None }) if name == "gemini"
        ));
    }

    #[test]
    fn parses_subcommand_aliases() {
        let oc = Cli::try_parse_from(["agentbox", "oc"]).expect("parse oc");
        assert!(matches!(oc.command, Some(Command::Opencode)));

        let shell = Cli::try_parse_from(["agentbox", "tmux", "work"]).expect("parse tmux");
        assert!(
            matches!(shell.command, Some(Command::Shell { session }) if session.as_deref() == Some("work"))
        );

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
        ])
        .expect("parse config init");
        assert_eq!(cli.config, Some(std::path::PathBuf::from("/tmp/agentbox.yaml")));
        assert_eq!(cli.verbose, 2);
        assert!(matches!(
            cli.command,
            Some(Command::Config { action: ConfigAction::Init { force: true } })
        ));
    }
}
