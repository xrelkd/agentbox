mod agent;
mod completions;
mod config;
mod internal;
mod list;
mod shell;
mod start;
mod status;
mod stop;
mod version;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use agent::{AgentArgs, ClaudeArgs, GeminiArgs, OpencodeArgs, PickArgs};
use clap::{ArgAction, Parser, Subcommand};
use completions::CompletionsArgs;
use config::ConfigArgs;
use internal::context::AppContext;
use list::ListArgs;
use shell::{HomeArgs, ShellArgs, TmpArgs};
use start::StartArgs;
use status::StatusArgs;
use stop::StopArgs;
use version::VersionArgs;

pub use crate::output::OutputFormat;
use crate::{
    app::{App, TargetRequirement},
    build::CLAP_LONG_VERSION,
    consts::CLI_PROGRAM_NAME,
    error::{Error, Result},
};

#[derive(Debug, Parser)]
#[command(
    name = CLI_PROGRAM_NAME,
    version,
    long_version = CLAP_LONG_VERSION,
    about = "Zero-image Podman AI sandboxes for isolated agent sessions"
)]
pub struct Cli {
    /// Path to an alternate config file.
    #[arg(long, global = true, value_name = "PATH", env = "AGENTBOX_CONFIG")]
    pub config: Option<PathBuf>,

    /// Increase logging verbosity (repeat for more detail).
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// Operate on a specific agentbox container instead of the current project.
    #[arg(long, global = true, value_name = "NAME")]
    pub container: Option<String>,

    /// Command to run (defaults to `list`).
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Start the project container; other commands only attach to it.
    #[command(aliases = ["s", "up"])]
    Start(StartArgs),

    /// Stop the project container and clean temporary directories.
    #[command(aliases = ["x", "down"])]
    Stop(StopArgs),

    /// List agentbox-managed containers and the current project directory.
    #[command(aliases = ["ps", "ls", "l"])]
    List(ListArgs),

    /// Show identity and paths for the current project directory.
    #[command(aliases = ["info", "project", "i", "st"])]
    Status(StatusArgs),

    /// Attach to a shell inside the session (alias: `sh`).
    #[command(alias = "sh")]
    Shell(ShellArgs),

    /// Launch a configured agent (alias: `a`).
    #[command(alias = "a")]
    Agent(AgentArgs),

    /// Print the agent chosen by the default chain or interactive picker.
    Pick(PickArgs),

    /// Attach to the `OpenCode` server in the running container (alias: `oc`).
    #[command(alias = "oc")]
    Opencode(OpencodeArgs),

    /// Attach to the `Claude` server in the running container.
    #[command(alias = "c")]
    Claude(ClaudeArgs),

    /// Attach to the `Gemini` server in the running container.
    #[command(alias = "g")]
    Gemini(GeminiArgs),

    /// Print the temporary directory for the current workspace.
    Tmp(TmpArgs),

    /// Print the container home directory for the current workspace.
    Home(HomeArgs),

    /// Print client and server version information.
    Version(VersionArgs),

    /// Generate shell completion scripts.
    Completions(CompletionsArgs),

    /// Manage the agentbox configuration file.
    Config(ConfigArgs),
}

impl Cli {
    pub fn command_or_default(&self) -> Command {
        self.command.clone().unwrap_or_else(|| Command::List(ListArgs::default()))
    }

    pub fn run(&self) -> Result<()> {
        let command = self.command_or_default();
        if let Command::Version(args) = &command {
            args.run();
            return Ok(());
        }

        let context = AppContext::new(self.config.as_deref())?;
        match &command {
            Command::Completions(args) => args.run(),
            Command::Config(args) => args.run(&context)?,
            command => {
                let app = context.load_app()?;
                run_with_app(command, &self.target_app(app, command)?)?;
            }
        }
        Ok(())
    }

    fn target_app(&self, app: App, command: &Command) -> Result<App> {
        let Some(requirement) = target_requirement(command) else {
            if self.container.is_some() {
                return Err(Error::ContainerOptionUnsupported);
            }
            return Ok(app);
        };
        app.resolve_target(self.container.as_deref(), requirement)
    }
}

const fn target_requirement(command: &Command) -> Option<TargetRequirement> {
    match command {
        Command::Shell(_)
        | Command::Agent(_)
        | Command::Opencode(_)
        | Command::Claude(_)
        | Command::Gemini(_) => Some(TargetRequirement::Running),
        Command::Stop(_) | Command::Status(_) | Command::Tmp(_) | Command::Home(_) => {
            Some(TargetRequirement::Any)
        }
        _ => None,
    }
}

fn run_with_app(command: &Command, app: &App) -> Result<()> {
    match command {
        Command::Start(args) => args.run(app),
        Command::Stop(args) => args.run(app),
        Command::List(args) => args.run(app),
        Command::Status(args) => args.run(app),
        Command::Shell(_) => ShellArgs::run(app),
        Command::Agent(args) => args.run(app),
        Command::Pick(_) => PickArgs::run(app),
        Command::Opencode(_) => OpencodeArgs::run(app),
        Command::Claude(_) => ClaudeArgs::run(app),
        Command::Gemini(_) => GeminiArgs::run(app),
        Command::Tmp(_) => {
            TmpArgs::run(app);
            Ok(())
        }
        Command::Home(_) => {
            HomeArgs::run(app);
            Ok(())
        }
        Command::Version(_) | Command::Completions(_) | Command::Config(_) => Ok(()),
    }
}
