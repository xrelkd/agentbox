use std::{num::ParseIntError, path::PathBuf, process::ExitStatus};

use snafu::Snafu;

/// Result type used throughout agentbox.
pub type Result<T> = std::result::Result<T, Error>;

/// Structured errors produced by agentbox.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Could not determine the current working directory: {source}"))]
    CurrentDirectory { source: std::io::Error },

    #[snafu(display("HOME environment variable is not set"))]
    HomeNotFound,

    #[snafu(display("Could not canonicalize project directory `{}`: {source}", path.display()))]
    CanonicalizeProject { path: PathBuf, source: std::io::Error },

    #[snafu(display("Refusing to run in forbidden directory `{}`", path.display()))]
    ForbiddenDirectory { path: PathBuf },

    #[snafu(display("Failed to run `{program}`: {source}"))]
    SpawnCommand { program: String, source: std::io::Error },

    #[snafu(display("`{program}` exited with status {status}: {stderr}"))]
    CommandFailed { program: String, status: ExitStatus, stderr: String },

    #[snafu(display("Could not parse `{value}` as {what}: {source}"))]
    ParseValue { value: String, what: &'static str, source: ParseIntError },

    #[snafu(display("Could not read config file `{}`: {source}", path.display()))]
    ReadConfig { path: PathBuf, source: std::io::Error },

    #[snafu(display("Could not parse config file `{}`: {source}", path.display()))]
    ParseConfig { path: PathBuf, source: serde_yaml::Error },

    #[snafu(display("Could not write config file `{}`: {source}", path.display()))]
    WriteConfig { path: PathBuf, source: std::io::Error },

    #[snafu(display("Config file `{}` already exists; pass --force to overwrite", path.display()))]
    ConfigAlreadyExists { path: PathBuf },

    #[snafu(display("Invalid configuration: {message}"))]
    InvalidConfig { message: String },

    #[snafu(display("Could not serialize ocelot supervise config: {source}"))]
    SerializeOcelotConfig { source: serde_yaml::Error },

    #[snafu(display("Could not create directory `{}`: {source}", path.display()))]
    CreateDir { path: PathBuf, source: std::io::Error },

    #[snafu(display("Could not write file `{}`: {source}", path.display()))]
    WriteFile { path: PathBuf, source: std::io::Error },

    #[snafu(display("Could not remove file `{}`: {source}", path.display()))]
    RemoveFile { path: PathBuf, source: std::io::Error },

    #[snafu(display("Could not rename `{}` to `{}`: {source}", from.display(), to.display()))]
    RenameFile { from: PathBuf, to: PathBuf, source: std::io::Error },

    #[snafu(display("Could not remove directory `{}`: {source}", path.display()))]
    RemoveDir { path: PathBuf, source: std::io::Error },

    #[snafu(display("Could not set permissions on `{}`: {source}", path.display()))]
    SetPermissions { path: PathBuf, source: std::io::Error },

    #[snafu(display("Could not create symlink `{}` -> `{}`: {source}", link.display(), target.display()))]
    CreateSymlink { link: PathBuf, target: PathBuf, source: std::io::Error },

    #[snafu(display("Could not resolve AI environment path `{}`: {source}", path.display()))]
    ResolveAiEnv { path: PathBuf, source: std::io::Error },

    #[snafu(display("Container specification is missing required field `{field}`"))]
    MissingField { field: &'static str },

    #[snafu(display("Agent `{name}` is not defined in the configuration"))]
    UnknownAgent { name: String },

    #[snafu(display("Agent `{name}` has no server command configured"))]
    AgentHasNoServe { name: String },

    #[snafu(display(
        "Container `{name}` is not running (status: {status}); run `agentbox start` first"
    ))]
    ContainerNotRunning { name: String, status: String },

    #[snafu(display(
        r"Container `{name}` exited unexpectedly (status: {status}); logs follow:
{logs}"
    ))]
    ContainerExited { name: String, status: String, logs: String },

    #[snafu(display(
        r"Timed out after {seconds}s waiting for `{name}`; logs follow:
{logs}"
    ))]
    StartupTimeout { name: String, seconds: u64, logs: String },

    #[snafu(display("Could not parse Podman output `{output}`"))]
    ParsePodmanOutput { output: String },

    #[snafu(display("Could not replace the current process with `{program}`: {source}"))]
    ExecReplace { program: String, source: std::io::Error },
}
