use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt};

use crate::error::{self, Result};

/// Minimal YAML template written by `agentbox config init`.
pub const DEFAULT_CONFIG_TEMPLATE: &[u8] = include_bytes!("default.yaml");

/// Complete YAML template written by `agentbox config init --full`.
pub const FULL_CONFIG_TEMPLATE: &[u8] = include_bytes!("full.yaml");

/// Container path where the Nix-built AI environment is mounted.
pub const AI_ENV_MOUNT_PATH: &str = "/run/ai-env";

/// Top-level agentbox configuration.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// General runtime paths and process settings.
    pub general: GeneralConfig,
    /// Container isolation, resources, and rootfs settings.
    pub container: ContainerConfig,
    /// Nix store/daemon and kernel-module mount settings.
    pub nix: NixConfig,
    /// Host-to-container mount declarations.
    pub mounts: Vec<MountEntry>,
    /// Environment variables injected into the container.
    pub environment: EnvironmentConfig,
    /// SOPS secret files injected as environment variables.
    pub secrets: SecretsConfig,
    /// Generic server startup and readiness polling settings.
    pub server: ServerConfig,
    /// Agent commands for interactive sessions.
    pub agents: BTreeMap<String, AgentConfig>,
}

/// General runtime paths and process settings.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Host path to the Nix-produced AI environment, resolved through symlinks.
    pub ai_env_path: String,
    /// Prefix used for generated container, home, temp, and metadata names.
    pub container_prefix: String,
    /// Host base directory containing `<uid>-<name>-<hash>/{home,tmp,meta}`.
    pub runtime_base_dir: String,
    /// Project directories that `start` rejects before creating a container.
    pub forbidden_dirs: Vec<String>,
    /// Preferred container shell; falls back to bash when it is unavailable.
    pub shell: String,
    /// Agent used when no command is given; `shell` opens an interactive shell.
    pub default_agent: String,
    /// Container path to the `ocelot` supervisor binary.
    pub ocelot_path: String,
    /// Podman executable used for all container operations.
    pub podman_binary: String,
}

/// Container isolation, resources, and rootfs settings.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "container isolation flags map directly to Podman options"
)]
pub struct ContainerConfig {
    /// Prefix for the generated container hostname.
    pub hostname_prefix: String,
    /// Work directory inside the container.
    pub workdir: String,
    /// Mount the container root filesystem read-only.
    pub read_only: bool,
    /// Prevent child processes from gaining new privileges.
    pub no_new_privileges: bool,
    /// Drop every Linux capability.
    pub drop_all_caps: bool,
    /// Maximum number of processes in the container.
    pub pids_limit: u32,
    /// Container memory limit passed to Podman.
    pub memory_limit: String,
    /// Container memory-plus-swap limit passed to Podman.
    pub memory_swap: Option<String>,
    /// Container CPU quota passed to Podman.
    pub cpus: Option<String>,
    /// Additional container ulimits.
    pub ulimits: BTreeMap<String, String>,
    /// Size of the `/tmp` tmpfs.
    pub tmp_size: String,
    /// Size of the `/var/tmp` tmpfs.
    pub var_tmp_size: String,
    /// Inherit host proxy environment variables into the container.
    pub inherit_proxy: bool,
    /// Mount the generated rootfs as an overlay (`O`).
    pub rootfs_overlay: bool,
}

/// Nix store/daemon and kernel-module mount settings.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct NixConfig {
    /// Mount the Nix daemon socket, profiles, and `/etc/nix` when the socket
    /// exists.
    pub enable_daemon_mount: bool,
    /// Host Nix store path, mounted read-only into the container.
    pub store_path: String,
    /// Host directory containing the Nix daemon socket.
    pub daemon_socket_dir: String,
    /// Host Nix profiles directory.
    pub profiles_dir: String,
    /// Host `/etc/nix` directory.
    pub etc_nix_dir: String,
    /// Mount matching host kernel modules when available.
    pub mount_kernel_modules: bool,
    /// Host directory containing kernel module version directories.
    pub kernel_modules_dir: String,
    /// Container directory where kernel modules are mounted.
    pub kernel_modules_mount_dir: String,
    /// Host Nix configuration directory whose safe settings are filtered into
    /// the per-project home.
    pub host_config_dir: String,
    /// Host Nix cache directory created before the container starts.
    pub host_cache_dir: String,
}

/// A single host-to-container mount declaration.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct MountEntry {
    /// Host source path; supports `~` and environment variables.
    pub host: String,
    /// Container destination path; supports `~` and environment variables.
    pub container: String,
    /// Mount read-only unless explicitly disabled.
    #[serde(default = "default_read_only")]
    pub read_only: bool,
}

impl Default for MountEntry {
    fn default() -> Self {
        Self { host: String::new(), container: String::new(), read_only: default_read_only() }
    }
}

const fn default_read_only() -> bool { true }

/// Environment variables injected into the container.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct EnvironmentConfig {
    /// Container `PATH`.
    pub path: String,
    /// Container `SSL_CERT_FILE` and `NIX_SSL_CERT_FILE`.
    pub ssl_cert_file: String,
    /// Container `CPATH`.
    pub cpath: String,
    /// Container `LIBRARY_PATH`.
    pub library_path: String,
    /// Container `PKG_CONFIG_PATH`.
    pub pkg_config_path: String,
    /// Container `LANG` and `LC_ALL`.
    pub lang: String,
    /// Container `HISTFILE`.
    pub histfile: String,
    /// `TERM` used when the host does not provide one.
    pub default_term: String,
    /// Container `NIX_REMOTE`.
    pub nix_remote: String,
    /// Additional environment variables.
    pub extra_env: BTreeMap<String, String>,
}

/// SOPS secret files injected as environment variables.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct SecretsConfig {
    /// Host base directory containing SOPS secret files.
    pub base_dir: String,
    /// Mapping from environment variable name to path relative to `base_dir`.
    pub entries: BTreeMap<String, String>,
}

/// Generic server startup and readiness polling settings.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Port used by an agent `serve` command.
    #[serde(alias = "opencode_port")]
    pub serve_port: u16,
    /// Host used by readiness probes inside the container.
    #[serde(alias = "opencode_host")]
    pub serve_host: String,
    /// Address the server listens on inside the container.
    pub listen_host: String,
    /// Maximum wait time for a server to become ready.
    pub startup_timeout_secs: u64,
    /// Delay between readiness probes.
    pub poll_interval_ms: u64,
}

/// Client/server commands for an agent that exposes a long-lived server.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct AgentServe {
    /// Command that starts the server, e.g. `opencode serve`.
    pub command: String,
    /// Command run inside the container with the attach URL appended, e.g.
    /// `opencode attach`.
    pub attach: String,
}

/// Agent command and optional server definition.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct AgentConfig {
    /// Command executed for an interactive agent session.
    pub command: String,
    /// Optional client/server commands for the agent.
    pub serve: Option<AgentServe>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            container: ContainerConfig::default(),
            nix: NixConfig::default(),
            mounts: default_mounts(),
            environment: EnvironmentConfig::default(),
            secrets: SecretsConfig::default(),
            server: ServerConfig::default(),
            agents: BTreeMap::from([
                ("claude".into(), AgentConfig::new("claude --resume")),
                ("gemini".into(), AgentConfig::new("gemini --resume")),
                (
                    "opencode".into(),
                    AgentConfig::with_serve(
                        "opencode",
                        AgentServe::new("opencode serve", "opencode attach"),
                    ),
                ),
            ]),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            ai_env_path: "~/.local/share/agentbox-env".into(),
            container_prefix: "agentbox".into(),
            runtime_base_dir: "/tmp/agentbox".into(),
            forbidden_dirs: vec!["/".into(), "/tmp".into()],
            shell: "zsh".into(),
            default_agent: "shell".into(),
            ocelot_path: "ocelot".into(),
            podman_binary: "podman".into(),
        }
    }
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            hostname_prefix: "sandbox".into(),
            workdir: "/workspace".into(),
            read_only: true,
            no_new_privileges: true,
            drop_all_caps: true,
            pids_limit: 1024,
            memory_limit: "32g".into(),
            memory_swap: None,
            cpus: None,
            ulimits: BTreeMap::from([
                ("core".into(), "0".into()),
                ("nofile".into(), "4096:8192".into()),
            ]),
            tmp_size: "2g".into(),
            var_tmp_size: "2g".into(),
            rootfs_overlay: true,
            inherit_proxy: false,
        }
    }
}

impl Default for NixConfig {
    fn default() -> Self {
        Self {
            enable_daemon_mount: true,
            store_path: "/nix/store".into(),
            daemon_socket_dir: "/nix/var/nix/daemon-socket".into(),
            profiles_dir: "/nix/var/nix/profiles".into(),
            etc_nix_dir: "/etc/nix".into(),
            mount_kernel_modules: false,
            kernel_modules_dir: "/run/current-system/kernel-modules/lib/modules".into(),
            kernel_modules_mount_dir: "/lib/modules".into(),
            host_config_dir: "~/.config/nix".into(),
            host_cache_dir: "~/.cache/nix".into(),
        }
    }
}

impl MountEntry {
    /// Creates a mount entry with an explicit read-only flag.
    pub fn new(host: impl Into<String>, container: impl Into<String>, read_only: bool) -> Self {
        Self { host: host.into(), container: container.into(), read_only }
    }
}

fn default_mounts() -> Vec<MountEntry> {
    vec![
        MountEntry::new("~/.agents", "~/.agents", true),
        MountEntry::new("~/.claude", "~/.claude", true),
        MountEntry::new("~/.claude/projects", "~/.claude/projects", false),
        MountEntry::new("~/.claude/todos", "~/.claude/todos", false),
        MountEntry::new("~/.config/opencode", "~/.config/opencode", true),
        MountEntry::new("~/.local/share/opencode", "~/.local/share/opencode", false),
        MountEntry::new("~/.local/state/opencode", "~/.local/state/opencode", false),
        MountEntry::new("~/.gemini", "~/.gemini", true),
        MountEntry::new("~/.gemini/history", "~/.gemini/history", false),
        MountEntry::new("~/.gemini/tmp", "~/.gemini/tmp", false),
        MountEntry::new("~/.local/bin", "~/.local/bin", true),
        MountEntry::new("~/.config/shell", "~/.config/shell", true),
        MountEntry::new("~/.zshrc", "~/.zshrc", true),
        MountEntry::new("~/.config/zsh", "~/.config/zsh", true),
        MountEntry::new("~/.config/starship.toml", "~/.config/starship.toml", true),
        MountEntry::new("~/.config/starship", "~/.config/starship", true),
        MountEntry::new("~/.cargo", "~/.cargo", false),
    ]
}

impl Default for EnvironmentConfig {
    fn default() -> Self {
        Self {
            path: "~/.local/bin:/run/ai-env/bin:/usr/bin:/bin".into(),
            ssl_cert_file: "/run/ai-env/etc/ssl/certs/ca-bundle.crt".into(),
            cpath: "/run/ai-env/include".into(),
            library_path: "/run/ai-env/lib".into(),
            pkg_config_path: "/run/ai-env/lib/pkgconfig:/run/ai-env/share/pkgconfig".into(),
            lang: "C.UTF-8".into(),
            histfile: "~/.cache/zsh_history".into(),
            default_term: "xterm-256color".into(),
            nix_remote: "daemon".into(),
            extra_env: BTreeMap::new(),
        }
    }
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            base_dir: "~/.config/sops-nix/secrets".into(),
            entries: BTreeMap::from([
                ("ANTHROPIC_API_KEY".into(), "ai/anthropic-api-key/default".into()),
                ("GEMINI_API_KEY".into(), "ai/gemini-api-key/default".into()),
                ("OPENROUTER_API_KEY".into(), "ai/openrouter-api-key/sandbox".into()),
            ]),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            serve_port: 4096,
            serve_host: "127.0.0.1".into(),
            listen_host: "127.0.0.1".into(),
            startup_timeout_secs: 15,
            poll_interval_ms: 250,
        }
    }
}

impl AgentServe {
    /// Creates a client/server definition for an agent.
    pub fn new(command: impl Into<String>, attach: impl Into<String>) -> Self {
        Self { command: command.into(), attach: attach.into() }
    }
}

impl AgentConfig {
    /// Creates an interactive-only agent definition.
    pub fn new(command: impl Into<String>) -> Self { Self { command: command.into(), serve: None } }

    /// Creates an agent definition with client/server commands.
    pub fn with_serve(command: impl Into<String>, serve: AgentServe) -> Self {
        Self { command: command.into(), serve: Some(serve) }
    }
}

impl Config {
    /// Resolves the config path, preferring an explicit CLI value and then XDG
    /// config home.
    pub fn resolve_path(explicit: Option<&Path>, home: &Path) -> PathBuf {
        if let Some(path) = explicit {
            return expand_path(path, home);
        }

        if let Some(xdg_config_home) = std::env::var_os("XDG_CONFIG_HOME") {
            let base = PathBuf::from(xdg_config_home);
            if !base.as_os_str().is_empty() {
                return expand_path(&base, home).join("agentbox/config.yaml");
            }
        }

        home.join(".config/agentbox/config.yaml")
    }

    /// Loads a YAML config file, falling back to built-in defaults when it does
    /// not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            tracing::debug!("No config file at {}; using defaults", path.display());
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)
            .with_context(|_| error::ReadConfigSnafu { path: path.to_path_buf() })?;
        let config: Self = serde_yaml::from_str(&content)
            .with_context(|_| error::ParseConfigSnafu { path: path.to_path_buf() })?;
        config.validate()?;
        Ok(config)
    }

    /// Writes the minimal or complete built-in YAML template to `path`.
    pub fn write_default(path: &Path, force: bool, full: bool) -> Result<()> {
        if path.exists() && !force {
            return Err(error::Error::ConfigAlreadyExists { path: path.to_path_buf() });
        }

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|_| error::CreateDirSnafu { path: parent.to_path_buf() })?;
        }

        let template = if full { FULL_CONFIG_TEMPLATE } else { DEFAULT_CONFIG_TEMPLATE };
        fs::write(path, template)
            .with_context(|_| error::WriteConfigSnafu { path: path.to_path_buf() })
    }

    /// Validates values that would otherwise produce confusing Podman failures.
    pub fn validate(&self) -> Result<()> {
        snafu::ensure!(
            !self.general.container_prefix.is_empty(),
            error::InvalidConfigSnafu { message: "general.container_prefix must not be empty" }
        );
        snafu::ensure!(
            !self.general.ai_env_path.is_empty(),
            error::InvalidConfigSnafu { message: "general.ai_env_path must not be empty" }
        );
        snafu::ensure!(
            !self.general.default_agent.is_empty(),
            error::InvalidConfigSnafu { message: "general.default_agent must not be empty" }
        );
        snafu::ensure!(
            self.general.default_agent == "shell"
                || self.agents.contains_key(&self.general.default_agent),
            error::InvalidConfigSnafu {
                message: "general.default_agent must be `shell` or a defined agent"
            }
        );
        snafu::ensure!(
            !self.general.ocelot_path.is_empty(),
            error::InvalidConfigSnafu { message: "general.ocelot_path must not be empty" }
        );
        snafu::ensure!(
            self.container.workdir.starts_with('/'),
            error::InvalidConfigSnafu { message: "container.workdir must be an absolute path" }
        );
        snafu::ensure!(
            !self.container.memory_limit.is_empty(),
            error::InvalidConfigSnafu { message: "container.memory_limit must not be empty" }
        );
        snafu::ensure!(
            self.container.memory_swap.as_ref().is_none_or(|value| !value.is_empty()),
            error::InvalidConfigSnafu { message: "container.memory_swap must not be empty" }
        );
        snafu::ensure!(
            self.container.cpus.as_ref().is_none_or(|value| !value.is_empty()),
            error::InvalidConfigSnafu { message: "container.cpus must not be empty" }
        );
        snafu::ensure!(
            !self.container.tmp_size.is_empty() && !self.container.var_tmp_size.is_empty(),
            error::InvalidConfigSnafu { message: "container tmpfs sizes must not be empty" }
        );
        snafu::ensure!(
            self.container.ulimits.keys().all(|name| {
                !name.is_empty() && name.chars().all(|character| character.is_ascii_alphanumeric())
            }),
            error::InvalidConfigSnafu { message: "container.ulimits keys must be alphanumeric" }
        );
        snafu::ensure!(
            !self.server.listen_host.is_empty(),
            error::InvalidConfigSnafu { message: "server.listen_host must not be empty" }
        );
        snafu::ensure!(
            self.server.serve_port > 0,
            error::InvalidConfigSnafu { message: "server.serve_port must be greater than zero" }
        );
        snafu::ensure!(
            self.server.startup_timeout_secs > 0,
            error::InvalidConfigSnafu {
                message: "server.startup_timeout_secs must be greater than zero"
            }
        );
        snafu::ensure!(
            self.server.poll_interval_ms > 0,
            error::InvalidConfigSnafu {
                message: "server.poll_interval_ms must be greater than zero"
            }
        );
        for (name, agent) in &self.agents {
            if let Some(serve) = &agent.serve {
                validate_command(&serve.command, &format!("agents.{name}.serve.command"))?;
                validate_command(&serve.attach, &format!("agents.{name}.serve.attach"))?;
            }
            validate_command(&agent.command, &format!("agents.{name}.command"))?;
        }
        Ok(())
    }
}

/// Expands `~` and environment variables in a string using `home`.
#[inline]
pub fn expand_str(input: &str, home: &Path) -> String {
    let home = home.to_string_lossy().into_owned();
    shellexpand::full_with_context_no_errors(input, || Some(home), |key| std::env::var(key).ok())
        .into_owned()
}

/// Expands `~` and environment variables in a path-like string using `home`.
#[inline]
pub fn expand_path(input: &Path, home: &Path) -> PathBuf {
    PathBuf::from(expand_str(&input.to_string_lossy(), home))
}

fn validate_command(command: &str, field: &str) -> Result<()> {
    let words = split_command(command)?;
    snafu::ensure!(
        !words.is_empty(),
        error::InvalidConfigSnafu { message: format!("{field} must not be empty") }
    );
    Ok(())
}

/// Splits a configured command string into shell-quoted words.
pub fn split_command(command: &str) -> Result<Vec<String>> {
    shlex::split(command).with_context(|| error::InvalidConfigSnafu {
        message: format!("could not parse command `{command}`"),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::{
        Config, DEFAULT_CONFIG_TEMPLATE, FULL_CONFIG_TEMPLATE, MountEntry, expand_path,
        split_command,
    };

    #[test]
    fn full_template_parses_to_default_config() {
        let parsed: Config =
            serde_yaml::from_slice(FULL_CONFIG_TEMPLATE).expect("full template should parse");
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn minimal_template_parses_with_fewer_mounts() {
        let parsed: Config =
            serde_yaml::from_slice(DEFAULT_CONFIG_TEMPLATE).expect("minimal template should parse");
        assert!(parsed.mounts.len() < Config::default().mounts.len());
        assert_eq!(parsed.general, Config::default().general);
        assert_eq!(parsed.agents, Config::default().agents);
    }

    #[test]
    fn write_default_selects_template() {
        let root =
            std::env::temp_dir().join(format!("agentbox-template-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&root));
        fs::create_dir_all(&root).expect("create root");
        let path = root.join("config.yaml");

        Config::write_default(&path, false, false).expect("write minimal template");
        assert_eq!(fs::read(&path).expect("read minimal"), DEFAULT_CONFIG_TEMPLATE.to_vec());

        Config::write_default(&path, true, true).expect("write full template");
        assert_eq!(fs::read(&path).expect("read full"), FULL_CONFIG_TEMPLATE.to_vec());

        drop(fs::remove_dir_all(&root));
    }

    #[test]
    fn read_only_defaults_to_true() {
        let config: Config = serde_yaml::from_str(
            r"
mounts:
  - host: /host/path
    container: /container/path
",
        )
        .expect("mount without read_only should parse");
        assert!(config.mounts[0].read_only);
        assert!(MountEntry::default().read_only);
    }

    #[test]
    fn mounts_parse_as_a_flat_list() {
        let config: Config = serde_yaml::from_str(
            r"
mounts:
  - host: /host/path
    container: /container/path
    read_only: true
",
        )
        .expect("flat mounts should parse");

        assert_eq!(config.mounts.len(), 1);
        assert_eq!(config.mounts[0].host, "/host/path");
        assert_eq!(config.mounts[0].container, "/container/path");
        assert!(config.mounts[0].read_only);
        assert!(Config::default().general.forbidden_dirs.contains(&"/".into()));
    }

    #[test]
    fn partial_config_merges_with_defaults() {
        let partial = r"
server:
  opencode_port: 5000
general:
  container_prefix: custom
";
        let config: Config = serde_yaml::from_str(partial).expect("partial config should parse");
        assert_eq!(config.server.serve_port, 5000);
        assert_eq!(config.general.container_prefix, "custom");
        assert_eq!(config.general.ai_env_path, Config::default().general.ai_env_path);
        assert_eq!(config.agents.len(), 3);
        assert_eq!(config.general.default_agent, "shell");
        assert_eq!(
            config.agents["opencode"].serve.as_ref().expect("opencode serve").command,
            "opencode serve"
        );
    }

    #[test]
    fn new_server_keys_parse_and_keep_aliases() {
        let config: Config = serde_yaml::from_str(
            r"
server:
  serve_port: 6000
  serve_host: 0.0.0.0
general:
  default_agent: gemini
",
        )
        .expect("new keys should parse");
        assert_eq!(config.server.serve_port, 6000);
        assert_eq!(config.server.serve_host, "0.0.0.0");
        assert_eq!(config.server.listen_host, "127.0.0.1");
        assert_eq!(config.general.default_agent, "gemini");
    }

    #[test]
    fn agent_defaults_resume_native_sessions() {
        let agents = &Config::default().agents;
        assert_eq!(agents["claude"].command, "claude --resume");
        assert_eq!(agents["gemini"].command, "gemini --resume");
        assert!(agents["claude"].serve.is_none());
        assert!(agents["gemini"].serve.is_none());
    }

    #[test]
    fn expand_path_resolves_tilde_and_environment_variables() {
        let home = Path::new("/home/test-user");
        assert_eq!(
            expand_path(Path::new("~/.agents"), home),
            PathBuf::from("/home/test-user/.agents")
        );

        let path = std::env::var("PATH").expect("PATH should be set");
        assert_eq!(expand_path(Path::new("$PATH"), home), PathBuf::from(path));
    }

    #[test]
    fn validate_rejects_empty_prefix() {
        let mut config = Config::default();
        config.general.container_prefix.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_requires_absolute_workdir() {
        let mut config = Config::default();
        config.container.workdir = "workspace".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_listen_host() {
        let mut config = Config::default();
        config.server.listen_host.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_allows_loopback_listen_host() {
        let mut config = Config::default();
        config.server.listen_host = "127.0.0.1".into();
        assert!(config.validate().is_ok());

        config.server.listen_host = "::1".into();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_unknown_default_agent() {
        let mut config = Config::default();
        config.general.default_agent = "missing".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn split_command_honors_shell_quoting() {
        let words =
            split_command(r#"claude --resume "--model claude's-choice" --append 'two words'"#)
                .expect("command should parse");
        assert_eq!(
            words,
            ["claude", "--resume", "--model claude's-choice", "--append", "two words"]
        );
    }

    #[test]
    fn split_command_rejects_unbalanced_quotes() {
        assert!(split_command("claude --resume \"unclosed").is_err());
    }

    #[test]
    fn validate_rejects_unbalanced_quotes_in_agent_command() {
        let mut config = Config::default();
        config.agents.get_mut("claude").expect("claude agent").command =
            "claude --resume \"unclosed".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_whitespace_only_agent_command() {
        let mut config = Config::default();
        config.agents.get_mut("claude").expect("claude agent").command = "   ".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_whitespace_only_serve_command() {
        let mut config = Config::default();
        let serve = config
            .agents
            .get_mut("opencode")
            .expect("opencode agent")
            .serve
            .as_mut()
            .expect("opencode serve");
        serve.command = "\t ".into();
        assert!(config.validate().is_err());
    }
}
