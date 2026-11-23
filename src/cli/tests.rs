use std::{ffi::OsStr, path::PathBuf};

use clap::{CommandFactory, Parser};
use clap_complete::Shell;

use super::{Cli, Command, OutputFormat, config::ConfigAction};

fn command(cli: &Cli) -> Command { cli.command_or_default() }

#[test]
fn defaults_to_list() {
    let cli = Cli::try_parse_from(["agentbox"]).expect("parse empty args");
    assert_eq!(cli.verbose, 0);
    assert!(matches!(
        command(&cli),
        Command::List(args) if args.format.is_none() && !args.quiet
    ));
}

#[test]
fn default_ignores_config_option() {
    let cli = Cli::try_parse_from(["agentbox", "--config", "/tmp/agentbox.yaml"])
        .expect("parse config option");
    assert!(matches!(
        command(&cli),
        Command::List(args) if args.format.is_none() && !args.quiet
    ));
}

#[test]
fn parses_start_and_agent_commands() {
    let cli = Cli::try_parse_from(["agentbox", "start", "--attach", "--agent", "claude"])
        .expect("parse start");
    assert!(matches!(
        command(&cli),
        Command::Start(args) if args.attach && args.agent.as_deref() == Some("claude")
    ));

    let cli = Cli::try_parse_from(["agentbox", "start"]).expect("parse detached start");
    assert!(matches!(
        command(&cli),
        Command::Start(args) if !args.attach && args.agent.is_none()
    ));

    let cli = Cli::try_parse_from(["agentbox", "agent", "gemini"]).expect("parse named agent");
    assert!(matches!(
        command(&cli),
        Command::Agent(args) if args.name.as_deref() == Some("gemini")
    ));

    let cli = Cli::try_parse_from(["agentbox", "agent"]).expect("parse default agent");
    assert!(matches!(
        command(&cli),
        Command::Agent(args) if args.name.is_none()
    ));
}

#[test]
fn parses_pick_command() {
    let cli = Cli::try_parse_from(["agentbox", "pick"]).expect("parse pick");
    assert!(matches!(command(&cli), Command::Pick(_)));
}

#[test]
fn parses_subcommand_aliases() {
    for alias in ["s", "up"] {
        let cli = Cli::try_parse_from(["agentbox", alias]).expect("parse start alias");
        assert!(matches!(command(&cli), Command::Start(_)));
    }
    for alias in ["x", "down"] {
        let cli = Cli::try_parse_from(["agentbox", alias]).expect("parse stop alias");
        assert!(matches!(command(&cli), Command::Stop(_)));
    }
    for alias in ["ps", "ls", "l"] {
        let cli = Cli::try_parse_from(["agentbox", alias]).expect("parse list alias");
        assert!(matches!(command(&cli), Command::List(_)));
    }
    for alias in ["info", "project", "i", "st"] {
        let cli = Cli::try_parse_from(["agentbox", alias]).expect("parse status alias");
        assert!(matches!(command(&cli), Command::Status(_)));
    }

    let cli = Cli::try_parse_from(["agentbox", "sh"]).expect("parse shell alias");
    assert!(matches!(command(&cli), Command::Shell(_)));

    let cli = Cli::try_parse_from(["agentbox", "a", "claude"]).expect("parse agent alias");
    assert!(matches!(
        command(&cli),
        Command::Agent(args) if args.name.as_deref() == Some("claude")
    ));

    let cli = Cli::try_parse_from(["agentbox", "oc"]).expect("parse opencode alias");
    assert!(matches!(command(&cli), Command::Opencode(_)));

    let cli = Cli::try_parse_from(["agentbox", "c"]).expect("parse claude alias");
    assert!(matches!(command(&cli), Command::Claude(_)));

    let cli = Cli::try_parse_from(["agentbox", "g"]).expect("parse gemini alias");
    assert!(matches!(command(&cli), Command::Gemini(_)));
}

#[test]
fn parses_list_output_flags() {
    let cli = Cli::try_parse_from(["agentbox", "ls", "--format", "json"]).expect("parse list json");
    assert!(matches!(
        command(&cli),
        Command::List(args) if args.format == Some(OutputFormat::Json) && !args.quiet
    ));

    let cli = Cli::try_parse_from(["agentbox", "list", "-q"]).expect("parse list quiet");
    assert!(matches!(
        command(&cli),
        Command::List(args) if args.format.is_none() && args.quiet
    ));

    assert!(Cli::try_parse_from(["agentbox", "ls", "-q", "--format", "json"]).is_err());
}

#[test]
fn parses_status_command() {
    let cli = Cli::try_parse_from(["agentbox", "status"]).expect("parse status");
    assert!(matches!(
        command(&cli),
        Command::Status(args) if args.format.is_none() && !args.quiet
    ));

    let cli =
        Cli::try_parse_from(["agentbox", "info", "--format", "json"]).expect("parse status alias");
    assert!(matches!(
        command(&cli),
        Command::Status(args) if args.format == Some(OutputFormat::Json) && !args.quiet
    ));
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
fn parses_version_command() {
    let cli = Cli::try_parse_from(["agentbox", "version"]).expect("parse version");
    assert!(matches!(
        command(&cli),
        Command::Version(args) if !args.client
    ));

    let cli =
        Cli::try_parse_from(["agentbox", "version", "--client"]).expect("parse client version");
    assert!(matches!(
        command(&cli),
        Command::Version(args) if args.client
    ));
}

#[test]
fn parses_completions() {
    let cli = Cli::try_parse_from(["agentbox", "completions", "zsh"]).expect("parse completions");
    assert!(matches!(
        command(&cli),
        Command::Completions(super::completions::CompletionsArgs { shell: Shell::Zsh })
    ));
}

#[test]
fn parses_persistent_home_commands() {
    let cli = Cli::try_parse_from(["agentbox", "home"]).expect("parse home");
    assert!(matches!(command(&cli), Command::Home(_)));

    let cli = Cli::try_parse_from(["agentbox", "stop", "--purge-home"])
        .expect("parse stop with purge home");
    assert!(matches!(
        command(&cli),
        Command::Stop(args) if args.purge_home
    ));

    let cli = Cli::try_parse_from(["agentbox", "stop"]).expect("parse stop");
    assert!(matches!(
        command(&cli),
        Command::Stop(args) if !args.purge_home
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
        "--full",
    ])
    .expect("parse config init");
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/agentbox.yaml")));
    assert_eq!(cli.verbose, 2);
    assert!(matches!(
        cli.command,
        Some(Command::Config(super::config::ConfigArgs {
            action: ConfigAction::Init { force: true, full: true },
        }))
    ));

    let cli = Cli::try_parse_from(["agentbox", "config", "init"]).expect("parse minimal init");
    assert!(matches!(
        cli.command,
        Some(Command::Config(super::config::ConfigArgs {
            action: ConfigAction::Init { force: false, full: false },
        }))
    ));
}

#[test]
fn parses_config_show() {
    let cli = Cli::try_parse_from(["agentbox", "config", "show"]).expect("parse config show");
    assert!(matches!(
        command(&cli),
        Command::Config(super::config::ConfigArgs { action: ConfigAction::Show { full: false } })
    ));

    let cli = Cli::try_parse_from(["agentbox", "config", "show", "--full"])
        .expect("parse full config show");
    assert!(matches!(
        command(&cli),
        Command::Config(super::config::ConfigArgs { action: ConfigAction::Show { full: true } })
    ));
}
