use std::{
    env, fs,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use snafu::ResultExt;

use crate::{
    config::{self, Config},
    error::{self, Result},
    podman::{ContainerSpec, Mount, PodmanClient, TcpProbe, Tmpfs, shell_quote},
    rootfs,
    workspace::ProjectIdentity,
};

/// Orchestrates project setup and every agentbox subcommand.
pub struct App {
    config: Config,
    identity: ProjectIdentity,
    podman: PodmanClient,
    home: PathBuf,
}

impl App {
    /// Builds an application context from loaded configuration and project
    /// identity.
    pub fn new(config: Config, identity: ProjectIdentity, home: PathBuf) -> Self {
        let podman = PodmanClient::new(config.general.podman_binary.clone());
        Self { config, identity, podman, home }
    }

    /// Prints the host temporary directory for the current project.
    pub fn temporary_dir(&self) {
        println!("{}", self.identity.tmp_dir.display());
    }

    /// Lists agentbox-managed containers.
    pub fn list(&self) -> Result<()> {
        let prefix = &self.config.general.container_prefix;
        let containers = self.podman.list_containers(prefix)?;
        println!("{:<48}\tSTATUS", "NAME");
        for container in containers {
            println!("{:<48}\t{}", container.name, container.status);
        }
        Ok(())
    }

    /// Stops the project container and removes its temporary directories.
    pub fn stop(&self) -> Result<()> {
        let name = &self.identity.container_name;
        if self.podman.container_exists(name)? {
            tracing::info!("Stopping and removing container: {name}");
            self.podman.stop_and_remove(name)?;
        } else {
            tracing::info!("No container found for the current project: {name}");
        }

        for directory in [&self.identity.meta_dir, &self.identity.tmp_dir] {
            if directory.exists() {
                fs::remove_dir_all(directory)
                    .with_context(|_| error::RemoveDirSnafu { path: directory.clone() })?;
                tracing::info!("Cleaned up temporary directory: {}", directory.display());
            }
        }
        Ok(())
    }

    /// Attaches to the `OpenCode` server running inside the container.
    pub fn opencode(&self) -> Result<()> {
        self.start_container()?;
        let name = &self.identity.container_name;
        let probe = TcpProbe {
            shell: self.init_shell(),
            host: self.config.server.opencode_host.clone(),
            port: self.config.server.opencode_port,
            timeout: Duration::from_secs(self.config.server.startup_timeout_secs),
            interval: Duration::from_millis(self.config.server.poll_interval_ms),
        };

        tracing::info!("Waiting for opencode server in {name} to be ready...");
        self.podman.wait_for_tcp_ready(name, &probe)?;
        tracing::info!("Attaching to the opencode server...");
        let url = self.config.opencode_url();
        let workdir = self.expand_config_path(&self.config.container.workdir);
        self.podman.exec_interactive(name, &["opencode", "attach", url.as_str()], Some(&workdir))
    }

    /// Creates or reattaches to an agent tmux session.
    pub fn agent(&self, agent_name: &str, session: Option<String>) -> Result<()> {
        let agent = self
            .config
            .agents
            .get(agent_name)
            .ok_or_else(|| error::Error::UnknownAgent { name: agent_name.to_owned() })?;
        let session = session.unwrap_or_else(|| agent.default_session.clone());
        let command = agent.command.clone();

        self.start_container()?;
        self.ensure_tmux_session(&session, Some(&command))?;
        self.attach_tmux(&session)
    }

    /// Attaches to a plain shell tmux session.
    pub fn shell(&self, session: Option<String>) -> Result<()> {
        let session = session.unwrap_or_else(|| self.config.server.default_tmux_session.clone());
        self.start_container()?;
        self.ensure_tmux_session(&session, None)?;
        self.attach_tmux(&session)
    }

    /// Lists tmux sessions inside the project container.
    pub fn sessions(&self) -> Result<()> {
        self.start_container()?;
        let output = self.podman.exec_capture(
            &self.identity.container_name,
            &["tmux", "list-sessions"],
            None,
        )?;
        print!("{output}");
        Ok(())
    }

    fn attach_tmux(&self, session: &str) -> Result<()> {
        self.podman.exec_interactive(
            &self.identity.container_name,
            &["tmux", "attach", "-t", session],
            None,
        )
    }

    fn ensure_tmux_session(&self, session: &str, command: Option<&str>) -> Result<()> {
        let name = &self.identity.container_name;
        if self.podman.exec(name, &["tmux", "has-session", "-t", session], None).is_ok() {
            tracing::info!("Reconnecting to existing Agent session [{session}]...");
            return Ok(());
        }

        tracing::info!("Creating independent Agent session [{session}]...");
        let workdir = &self.config.container.workdir;
        self.podman.exec(
            name,
            &["tmux", "new-session", "-d", "-s", session, "-c", workdir],
            None,
        )?;
        if let Some(command) = command {
            tracing::info!("Starting `{command}` in session [{session}]...");
            self.podman.exec(
                name,
                &["tmux", "send-keys", "-t", session, command, "Enter"],
                None,
            )?;
        }
        Ok(())
    }

    fn start_container(&self) -> Result<()> {
        let name = &self.identity.container_name;
        if self.podman.container_exists(name)? {
            match self.podman.container_status(name)? {
                crate::podman::ContainerStatus::Running => {
                    tracing::debug!("Container {name} is already running");
                    return Ok(());
                }
                status => {
                    tracing::info!("Removing stale container {name} (status: {status})");
                    self.podman.stop_and_remove(name)?;
                }
            }
        }

        tracing::info!(
            "Initializing zero-image isolated container: {name} (UID={}, GID={})...",
            self.identity.host.uid,
            self.identity.host.gid
        );

        let ai_env_path = self.ai_env_path()?;
        let shell = rootfs::prepare(&self.config, &self.identity, &ai_env_path)?;
        let spec = self.build_spec(&ai_env_path, &shell)?;
        self.podman.run_detached(&spec)
    }

    fn ai_env_path(&self) -> Result<PathBuf> {
        let path = self.expand_config_path(&self.config.general.ai_env_path);
        let canonical = fs::canonicalize(&path)
            .with_context(|_| error::ResolveAiEnvSnafu { path: path.clone() })?;
        tracing::debug!("Resolved AI environment path: {}", canonical.display());
        Ok(canonical)
    }

    fn build_spec(&self, ai_env_path: &Path, shell: &str) -> Result<ContainerSpec> {
        let config = &self.config;
        let identity = &self.identity;
        let workdir = self.expand_config_path(&config.container.workdir);
        let init_shell = self.init_shell();

        let mut builder = ContainerSpec::builder(identity.container_name.clone())
            .hostname(format!("{}-{}", config.container.hostname_prefix, identity.name))
            .workdir(workdir)
            .user(identity.host.uid, identity.host.gid)
            .rootfs(identity.rootfs_dir.clone())
            .rootfs_overlay(config.container.rootfs_overlay)
            .init_shell(init_shell)
            .init_script(self.init_script())
            .read_only(config.container.read_only)
            .no_new_privileges(config.container.no_new_privileges)
            .drop_all_caps(config.container.drop_all_caps)
            .pids_limit(config.container.pids_limit)
            .memory_limit(config.container.memory_limit.clone());

        for mount in self.build_mounts(ai_env_path) {
            builder = builder.mount(mount);
        }
        for tmpfs in self.build_tmpfs() {
            builder = builder.tmpfs(tmpfs);
        }
        for (key, value) in self.build_env(shell) {
            builder = builder.env(key, value);
        }
        builder.build()
    }

    fn build_mounts(&self, ai_env_path: &Path) -> Vec<Mount> {
        let config = &self.config;
        let mut mounts = Vec::new();
        let mut add = |host: PathBuf, container: PathBuf, read_only: bool| {
            mounts.push(Mount::new(host, container, read_only));
        };

        add(self.identity.tmp_dir.clone(), PathBuf::from("/tmp"), false);

        let store = self.expand_config_path(&config.nix.store_path);
        add(store.clone(), store, true);

        if config.nix.enable_daemon_mount {
            let socket_dir = self.expand_config_path(&config.nix.daemon_socket_dir);
            if is_socket(&socket_dir.join("socket")) {
                let container_socket = self.expand_config_path(&config.nix.daemon_socket_dir);
                add(socket_dir, container_socket, false);
                let profiles = self.expand_config_path(&config.nix.profiles_dir);
                add(profiles.clone(), profiles, true);
                let etc_nix = self.expand_config_path(&config.nix.etc_nix_dir);
                add(etc_nix.clone(), etc_nix, true);
            }
        }

        let mount_path = PathBuf::from(&config.general.ai_env_mount_path);
        add(ai_env_path.to_path_buf(), mount_path, true);

        if config.nix.mount_kernel_modules
            && let Some(release) = kernel_release()
        {
            let host_modules = self.expand_config_path(&format!(
                "{}/{}",
                config.nix.kernel_modules_dir.trim_end_matches('/'),
                release
            ));
            if host_modules.is_dir() {
                let container_modules = PathBuf::from(format!(
                    "{}/{}",
                    config.nix.kernel_modules_mount_dir.trim_end_matches('/'),
                    release
                ));
                add(host_modules, container_modules, true);
            }
        }

        for entry in &config.mounts.state_dirs {
            add(
                self.expand_config_path(&entry.host),
                self.expand_config_path(&entry.container),
                entry.read_only,
            );
        }
        for entry in &config.mounts.optional_ro_mounts {
            let host = self.expand_config_path(&entry.host);
            if host.exists() {
                add(host, self.expand_config_path(&entry.container), entry.read_only);
            }
        }
        for entry in &config.mounts.extra_mounts {
            let host = self.expand_config_path(&entry.host);
            if entry.skip_if_missing && !host.exists() {
                tracing::debug!("Skipping missing mount source {}", host.display());
                continue;
            }
            add(host, self.expand_config_path(&entry.container), entry.read_only);
        }

        add(
            self.identity.project_dir.clone(),
            self.expand_config_path(&config.container.workdir),
            false,
        );
        let config_dir = self.expand_config_path(&config.nix.host_config_dir);
        add(config_dir.clone(), config_dir, true);

        mounts
    }

    fn build_tmpfs(&self) -> Vec<Tmpfs> {
        let config = &self.config;
        vec![
            Tmpfs::new(
                PathBuf::from("/var/tmp"),
                format!("rw,nosuid,size={}", config.container.var_tmp_size),
            ),
            Tmpfs::new(
                self.home.join(".cache"),
                format!("rw,nosuid,size={}", config.container.cache_tmpfs_size),
            ),
        ]
    }

    fn build_env(&self, shell: &str) -> Vec<(String, String)> {
        let config = &self.config;
        let mut variables = Vec::new();

        push_env(&mut variables, "HOME", self.home.display().to_string());
        push_env(&mut variables, "USER", self.identity.host.username.clone());
        push_env(&mut variables, "SHELL", shell.to_owned());
        push_env(
            &mut variables,
            "HISTFILE",
            self.expand_config_path(&config.environment.histfile).display().to_string(),
        );
        push_env(&mut variables, "PATH", self.expand_config_value(&config.environment.path));
        push_env(
            &mut variables,
            "SSL_CERT_FILE",
            self.expand_config_value(&config.environment.ssl_cert_file),
        );
        push_env(
            &mut variables,
            "NIX_SSL_CERT_FILE",
            self.expand_config_value(&config.environment.ssl_cert_file),
        );
        push_env(&mut variables, "CPATH", self.expand_config_value(&config.environment.cpath));
        push_env(
            &mut variables,
            "LIBRARY_PATH",
            self.expand_config_value(&config.environment.library_path),
        );
        push_env(
            &mut variables,
            "PKG_CONFIG_PATH",
            self.expand_config_value(&config.environment.pkg_config_path),
        );
        push_env(
            &mut variables,
            "NIX_REMOTE",
            self.expand_config_value(&config.environment.nix_remote),
        );
        push_env(&mut variables, "LANG", config.environment.lang.clone());
        push_env(&mut variables, "LC_ALL", config.environment.lang.clone());
        push_env(
            &mut variables,
            "TERM",
            env::var("TERM").unwrap_or_else(|_| config.environment.default_term.clone()),
        );

        for (key, value) in &config.environment.extra_env {
            variables.push((key.clone(), self.expand_config_value(value)));
        }

        let secrets_base = self.expand_config_path(&config.secrets.base_dir);
        for (key, relative) in &config.secrets.entries {
            let path = secrets_base.join(relative);
            match fs::read_to_string(&path) {
                Ok(value) => {
                    let value = value.trim_end_matches(['\n', '\r']);
                    variables.push((key.clone(), value.to_owned()));
                }
                Err(error) => {
                    tracing::debug!("Skipping unreadable secret {}: {error}", path.display());
                }
            }
        }

        variables
    }

    fn init_script(&self) -> String {
        let config = &self.config;
        let session = shell_quote(&config.server.default_tmux_session);
        let workdir = shell_quote(&config.container.workdir);
        let host = shell_quote(&config.server.opencode_host);
        format!(
            "tmux new-session -d -s {session} -c {workdir}\nexec opencode serve --port {} \
             --hostname {host}",
            config.server.opencode_port
        )
    }

    #[inline]
    fn init_shell(&self) -> String {
        let mount = self.config.general.ai_env_mount_path.trim_end_matches('/');
        format!("{mount}/bin/bash")
    }

    #[inline]
    fn expand_config_path(&self, raw: &str) -> PathBuf {
        config::expand_path(Path::new(raw), &self.home)
    }

    #[inline]
    fn expand_config_value(&self, raw: &str) -> String { config::expand_str(raw, &self.home) }
}

#[inline]
fn push_env(variables: &mut Vec<(String, String)>, key: &str, value: String) {
    variables.push((key.to_owned(), value));
}

#[inline]
fn is_socket(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
}

fn kernel_release() -> Option<String> {
    if let Ok(content) = fs::read_to_string("/proc/sys/kernel/osrelease") {
        let release = content.trim();
        if !release.is_empty() {
            return Some(release.to_owned());
        }
    }

    let output = Command::new("uname").arg("-r").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let release = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if release.is_empty() { None } else { Some(release) }
}
