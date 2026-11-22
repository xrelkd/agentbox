use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use snafu::ResultExt;

use crate::error::{self, Result};

/// Default YAML template emitted by `agentbox config init`.
pub const DEFAULT_CONFIG_TEMPLATE: &str = r#"
general:
  ai_env_path: "~/.local/share/agentbox-env"
  ai_env_mount_path: "/run/ai-env"
  container_prefix: "agentbox"
  runtime_base_dir: "/tmp"
  forbidden_dirs: ["/", "/tmp"]
  shell: "/run/ai-env/bin/zsh"
  podman_binary: "podman"

container:
  hostname_prefix: "sandbox"
  workdir: "/workspace"
  read_only: true
  no_new_privileges: true
  drop_all_caps: true
  pids_limit: 1024
  memory_limit: "12g"
  var_tmp_size: "2g"
  cache_tmpfs_size: "2g"
  rootfs_overlay: true

nix:
  enable_daemon_mount: true
  store_path: "/nix/store"
  daemon_socket_dir: "/nix/var/nix/daemon-socket"
  profiles_dir: "/nix/var/nix/profiles"
  etc_nix_dir: "/etc/nix"
  mount_kernel_modules: true
  kernel_modules_dir: "/run/current-system/kernel-modules/lib/modules"
  kernel_modules_mount_dir: "/lib/modules"
  host_config_dir: "~/.config/nix"
  host_cache_dir: "~/.cache/nix"

mounts:
  state_dirs:
    - host: "~/.agents"
      container: "~/.agents"
    - host: "~/.config/opencode"
      container: "~/.config/opencode"
    - host: "~/.local/share/opencode"
      container: "~/.local/share/opencode"
    - host: "~/.local/state/opencode"
      container: "~/.local/state/opencode"
    - host: "~/.claude"
      container: "~/.claude"
    - host: "~/.gemini"
      container: "~/.gemini"
  optional_ro_mounts:
    - host: "~/.local/bin"
      container: "~/.local/bin"
      read_only: true
      skip_if_missing: true
    - host: "~/.zshrc"
      container: "~/.zshrc"
      read_only: true
      skip_if_missing: true
    - host: "~/.config/zsh"
      container: "~/.config/zsh"
      read_only: true
      skip_if_missing: true
    - host: "~/.config/starship.toml"
      container: "~/.config/starship.toml"
      read_only: true
      skip_if_missing: true
    - host: "~/.config/starship"
      container: "~/.config/starship"
      read_only: true
      skip_if_missing: true
  extra_mounts: []

environment:
  path: "~/.local/bin:/run/ai-env/bin:/usr/bin:/bin"
  ssl_cert_file: "/run/ai-env/etc/ssl/certs/ca-bundle.crt"
  cpath: "/run/ai-env/include"
  library_path: "/run/ai-env/lib"
  pkg_config_path: "/run/ai-env/lib/pkgconfig:/run/ai-env/share/pkgconfig"
  lang: "C.UTF-8"
  histfile: "~/.cache/zsh_history"
  default_term: "xterm-256color"
  nix_remote: "daemon"
  extra_env: {}

secrets:
  base_dir: "~/.config/sops-nix/secrets"
  entries:
    ANTHROPIC_API_KEY: "ai/anthropic-api-key/default"
    GEMINI_API_KEY: "ai/gemini-api-key/default"
    OPENROUTER_API_KEY: "ai/openrouter-api-key/sandbox"

server:
  default_tmux_session: "main"
  opencode_port: 4096
  opencode_host: "127.0.0.1"
  startup_timeout_secs: 15
  poll_interval_ms: 250

agents:
  oc-tui:
    command: "opencode"
    default_session: "opencode"
  claude:
    command: "claude"
    default_session: "claude"
  gemini:
    command: "gemini"
    default_session: "gemini"
"#;

/// Top-level agentbox configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Config {
    /// General runtime paths and process settings.
    pub general: GeneralConfig,
    /// Container isolation, resources, and rootfs settings.
    pub container: ContainerConfig,
    /// Nix store/daemon and kernel-module mount settings.
    pub nix: NixConfig,
    /// Host-to-container mount declarations.
    pub mounts: MountsConfig,
    /// Environment variables injected into the container.
    pub environment: EnvironmentConfig,
    /// SOPS secret files injected as environment variables.
    pub secrets: SecretsConfig,
    /// `OpenCode` server startup and polling settings.
    pub server: ServerConfig,
    /// Agent commands and their default tmux session names.
    pub agents: BTreeMap<String, AgentConfig>,
}

/// General runtime paths and process settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Host path to the Nix-produced AI environment, resolved through symlinks.
    pub ai_env_path: String,
    /// Container path where `ai_env_path` is mounted read-only.
    pub ai_env_mount_path: String,
    /// Prefix used for generated container, temp, and metadata directory names.
    pub container_prefix: String,
    /// Host base directory for temporary and metadata directories.
    pub runtime_base_dir: String,
    /// Project directories that are rejected before any container is created.
    pub forbidden_dirs: Vec<String>,
    /// Preferred container shell; falls back to bash when it is unavailable.
    pub shell: String,
    /// Podman executable used for all container operations.
    pub podman_binary: String,
}

/// Container isolation, resources, and rootfs settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    /// Size of the `/var/tmp` tmpfs.
    pub var_tmp_size: String,
    /// Size of the `$HOME/.cache` tmpfs.
    pub cache_tmpfs_size: String,
    /// Mount the generated rootfs as an overlay (`O`).
    pub rootfs_overlay: bool,
}

/// Nix store/daemon and kernel-module mount settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    /// Host Nix configuration directory created and mounted read-only.
    pub host_config_dir: String,
    /// Host Nix cache directory created before the container starts.
    pub host_cache_dir: String,
}

/// A single host-to-container mount declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct MountEntry {
    /// Host source path; supports `~` and environment variables.
    pub host: String,
    /// Container destination path; supports `~` and environment variables.
    pub container: String,
    /// Mount read-only instead of read-write.
    pub read_only: bool,
    /// Skip the mount when the host path is missing.
    pub skip_if_missing: bool,
}

/// Host-to-container mount declarations.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct MountsConfig {
    /// State directories created on the host and mounted read-write.
    pub state_dirs: Vec<MountEntry>,
    /// Tool/config paths mounted read-only when present on the host.
    pub optional_ro_mounts: Vec<MountEntry>,
    /// Additional user-defined mounts.
    pub extra_mounts: Vec<MountEntry>,
}

/// Environment variables injected into the container.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct SecretsConfig {
    /// Host base directory containing SOPS secret files.
    pub base_dir: String,
    /// Mapping from environment variable name to path relative to `base_dir`.
    pub entries: BTreeMap<String, String>,
}

/// `OpenCode` server startup and polling settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ServerConfig {
    /// tmux session created when the container starts.
    pub default_tmux_session: String,
    /// Port used by `opencode serve`.
    pub opencode_port: u16,
    /// Host used by `opencode serve` and `opencode attach`.
    pub opencode_host: String,
    /// Maximum wait time for the `OpenCode` server.
    pub startup_timeout_secs: u64,
    /// Delay between readiness probes.
    pub poll_interval_ms: u64,
}

/// Agent command and default tmux session name.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct AgentConfig {
    /// Command executed inside the tmux session.
    pub command: String,
    /// Session name used when no explicit session is provided.
    pub default_session: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            container: ContainerConfig::default(),
            nix: NixConfig::default(),
            mounts: MountsConfig::default(),
            environment: EnvironmentConfig::default(),
            secrets: SecretsConfig::default(),
            server: ServerConfig::default(),
            agents: BTreeMap::from([
                ("claude".into(), AgentConfig::new("claude", "claude")),
                ("gemini".into(), AgentConfig::new("gemini", "gemini")),
                ("oc-tui".into(), AgentConfig::new("opencode", "opencode")),
            ]),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            ai_env_path: "~/.local/share/agentbox-env".into(),
            ai_env_mount_path: "/run/ai-env".into(),
            container_prefix: "agentbox".into(),
            runtime_base_dir: "/tmp".into(),
            forbidden_dirs: vec!["/".into(), "/tmp".into()],
            shell: "/run/ai-env/bin/zsh".into(),
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
            memory_limit: "12g".into(),
            var_tmp_size: "2g".into(),
            cache_tmpfs_size: "2g".into(),
            rootfs_overlay: true,
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
            mount_kernel_modules: true,
            kernel_modules_dir: "/run/current-system/kernel-modules/lib/modules".into(),
            kernel_modules_mount_dir: "/lib/modules".into(),
            host_config_dir: "~/.config/nix".into(),
            host_cache_dir: "~/.cache/nix".into(),
        }
    }
}

impl MountEntry {
    /// Creates a mount entry with explicit flags.
    pub fn new(
        host: impl Into<String>,
        container: impl Into<String>,
        read_only: bool,
        skip_if_missing: bool,
    ) -> Self {
        Self { host: host.into(), container: container.into(), read_only, skip_if_missing }
    }
}

impl Default for MountsConfig {
    fn default() -> Self {
        Self {
            state_dirs: vec![
                MountEntry::new("~/.agents", "~/.agents", false, false),
                MountEntry::new("~/.config/opencode", "~/.config/opencode", false, false),
                MountEntry::new("~/.local/share/opencode", "~/.local/share/opencode", false, false),
                MountEntry::new("~/.local/state/opencode", "~/.local/state/opencode", false, false),
                MountEntry::new("~/.claude", "~/.claude", false, false),
                MountEntry::new("~/.gemini", "~/.gemini", false, false),
            ],
            optional_ro_mounts: vec![
                MountEntry::new("~/.local/bin", "~/.local/bin", true, true),
                MountEntry::new("~/.zshrc", "~/.zshrc", true, true),
                MountEntry::new("~/.config/zsh", "~/.config/zsh", true, true),
                MountEntry::new("~/.config/starship.toml", "~/.config/starship.toml", true, true),
                MountEntry::new("~/.config/starship", "~/.config/starship", true, true),
            ],
            extra_mounts: Vec::new(),
        }
    }
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
            default_tmux_session: "main".into(),
            opencode_port: 4096,
            opencode_host: "127.0.0.1".into(),
            startup_timeout_secs: 15,
            poll_interval_ms: 250,
        }
    }
}

impl AgentConfig {
    /// Creates an agent definition.
    pub fn new(command: impl Into<String>, default_session: impl Into<String>) -> Self {
        Self { command: command.into(), default_session: default_session.into() }
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

    /// Writes the built-in YAML template to `path`.
    pub fn write_default(path: &Path, force: bool) -> Result<()> {
        if path.exists() && !force {
            return Err(error::Error::ConfigAlreadyExists { path: path.to_path_buf() });
        }

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|_| error::CreateDirSnafu { path: parent.to_path_buf() })?;
        }

        fs::write(path, DEFAULT_CONFIG_TEMPLATE.trim_start())
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
            self.container.workdir.starts_with('/'),
            error::InvalidConfigSnafu { message: "container.workdir must be an absolute path" }
        );
        snafu::ensure!(
            self.server.opencode_port > 0,
            error::InvalidConfigSnafu { message: "server.opencode_port must be greater than zero" }
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
        Ok(())
    }

    /// Returns the URL used by `opencode attach`.
    pub fn opencode_url(&self) -> String {
        format!("http://{}:{}", self.server.opencode_host, self.server.opencode_port)
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Config, DEFAULT_CONFIG_TEMPLATE, expand_path};

    #[test]
    fn default_template_parses_to_default_config() {
        let parsed: Config =
            serde_yaml::from_str(DEFAULT_CONFIG_TEMPLATE).expect("default template should parse");
        assert_eq!(parsed, Config::default());
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
        assert_eq!(config.server.opencode_port, 5000);
        assert_eq!(config.general.container_prefix, "custom");
        assert_eq!(config.general.ai_env_path, Config::default().general.ai_env_path);
        assert_eq!(config.agents.len(), 3);
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
}
