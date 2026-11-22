use std::{
    env, fs,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use snafu::{OptionExt, ResultExt};

use crate::{
    config::{self, AgentServe, Config, SessionBackend},
    error::{self, Result},
    ocelot,
    podman::{ContainerSpec, Mount, PodmanClient, TcpProbe, Tmpfs, shell_quote},
    rootfs,
    workspace::ProjectIdentity,
};

const OCELOT_CONFIG_PATH: &str = "/etc/agentbox/ocelot.yaml";
const SERVE_MARKER_PREFIX: &str = "/var/tmp/agentbox-serve-";

/// Init strategy selected for a container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitMode {
    /// `ocelot supervise` is PID 1 and owns the long-lived services.
    Ocelot,
    /// Podman's host init is PID 1 and services start on demand.
    Fallback,
}

/// Runtime facts returned when a container is started.
#[derive(Clone, Debug)]
struct Runtime {
    mode: InitMode,
    shell: String,
}

/// Orchestrates project setup and every agentbox subcommand.
pub struct App {
    config: Config,
    identity: ProjectIdentity,
    podman: PodmanClient,
    home: PathBuf,
    shell: String,
}

impl App {
    /// Builds an application context from loaded configuration and project
    /// identity.
    pub fn new(config: Config, identity: ProjectIdentity, home: PathBuf) -> Self {
        let ai_env = config::expand_path(Path::new(&config.general.ai_env_path), &home);
        let shell = rootfs::select_shell(&config, &ai_env, &config.general.ai_env_mount_path);
        let podman = PodmanClient::new(config.general.podman_binary.clone());
        Self { config, identity, podman, home, shell }
    }

    /// Prints the host temporary directory for the current project.
    pub fn temporary_dir(&self) {
        println!("{}", self.identity.tmp_dir.display());
    }

    /// Prints the host directory mounted as the persistent container home.
    pub fn home_dir(&self) {
        println!("{}", self.identity.home_dir.display());
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

    /// Stops the project container and removes its transient directories.
    pub fn stop(&self, purge_home: bool) -> Result<()> {
        let name = &self.identity.container_name;
        if self.podman.container_exists(name)? {
            tracing::info!("Stopping and removing container: {name}");
            self.podman.stop_and_remove(name)?;
        } else {
            tracing::info!("No container found for the current project: {name}");
        }

        let mut directories = vec![&self.identity.meta_dir, &self.identity.tmp_dir];
        if purge_home {
            directories.push(&self.identity.home_dir);
        }
        for directory in directories {
            if directory.exists() {
                fs::remove_dir_all(directory)
                    .with_context(|_| error::RemoveDirSnafu { path: directory.clone() })?;
                tracing::info!("Cleaned up directory: {}", directory.display());
            }
        }
        Ok(())
    }

    /// Starts the container without attaching, optionally attaching afterwards.
    pub fn start(&self, agent: Option<&str>, session: Option<String>, attach: bool) -> Result<()> {
        drop(self.start_container()?);
        if !attach {
            return Ok(());
        }
        match agent.unwrap_or(self.config.general.default_agent.as_str()) {
            "shell" => self.shell(session),
            name => self.launch(name, session),
        }
    }

    /// Launches an agent by name, using its server when one is configured.
    pub fn launch(&self, agent_name: &str, session: Option<String>) -> Result<()> {
        let has_serve =
            self.config.agents.get(agent_name).and_then(|agent| agent.serve.as_ref()).is_some();
        if has_serve { self.serve(agent_name) } else { self.agent(agent_name, session) }
    }

    /// Attaches to the `OpenCode` server managed by the container init.
    pub fn opencode(&self) -> Result<()> { self.serve("opencode") }

    /// Attaches to a configured agent server.
    pub fn serve(&self, agent_name: &str) -> Result<()> {
        let agent = self
            .config
            .agents
            .get(agent_name)
            .with_context(|| error::UnknownAgentSnafu { name: agent_name.to_owned() })?;
        let serve = agent
            .serve
            .clone()
            .with_context(|| error::AgentHasNoServeSnafu { name: agent_name.to_owned() })?;
        let runtime = self.start_container()?;
        let name = &self.identity.container_name;
        if runtime.mode == InitMode::Fallback {
            self.ensure_fallback_serve(agent_name, &serve)?;
        }

        let probe = TcpProbe {
            shell: runtime.shell,
            host: self.config.server.serve_host.clone(),
            port: self.config.server.serve_port,
            timeout: Duration::from_secs(self.config.server.startup_timeout_secs),
            interval: Duration::from_millis(self.config.server.poll_interval_ms),
        };
        tracing::info!("Waiting for {agent_name} server in {name} to be ready...");
        self.podman.wait_for_tcp_ready(name, &probe)?;

        let mut arguments = config::split_command(&serve.attach);
        arguments.push(self.config.serve_url());
        let command = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        tracing::info!("Attaching to the {agent_name} server...");
        let workdir = self.expand_config_path(&self.config.container.workdir);
        self.podman.exec_interactive(name, &command, Some(&workdir))
    }

    /// Creates or attaches to an interactive agent session.
    pub fn agent(&self, agent_name: &str, session: Option<String>) -> Result<()> {
        let agent = self
            .config
            .agents
            .get(agent_name)
            .with_context(|| error::UnknownAgentSnafu { name: agent_name.to_owned() })?;
        drop(self.start_container()?);
        let name = &self.identity.container_name;
        let workdir = self.expand_config_path(&self.config.container.workdir);

        match self.config.general.session_backend {
            SessionBackend::Direct => {
                let arguments = config::split_command(&agent.command);
                let command = arguments.iter().map(String::as_str).collect::<Vec<_>>();
                self.podman.exec_interactive(name, &command, Some(&workdir))
            }
            SessionBackend::Tmux => {
                let session = session.unwrap_or_else(|| agent.default_session.clone());
                let command = agent.command.clone();
                self.ensure_tmux_session(&session, Some(&command))?;
                self.attach_tmux(&session)
            }
        }
    }

    /// Attaches to a plain interactive shell.
    pub fn shell(&self, session: Option<String>) -> Result<()> {
        let runtime = self.start_container()?;
        let name = &self.identity.container_name;
        let workdir = self.expand_config_path(&self.config.container.workdir);

        match self.config.general.session_backend {
            SessionBackend::Direct => {
                let arguments = [runtime.shell.as_str(), "-i"];
                self.podman.exec_interactive(name, &arguments, Some(&workdir))
            }
            SessionBackend::Tmux => {
                let session =
                    session.unwrap_or_else(|| self.config.server.default_tmux_session.clone());
                self.ensure_tmux_session(&session, None)?;
                self.attach_tmux(&session)
            }
        }
    }

    /// Lists tmux sessions inside the project container.
    pub fn sessions(&self) -> Result<()> {
        if self.config.general.session_backend != SessionBackend::Tmux {
            return Err(error::Error::SessionsUnavailable);
        }
        drop(self.start_container()?);
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

    fn start_container(&self) -> Result<Runtime> {
        let name = &self.identity.container_name;
        let runtime = self.runtime();
        if self.podman.container_exists(name)? {
            match self.podman.container_status(name)? {
                crate::podman::ContainerStatus::Running => {
                    tracing::debug!("Container {name} is already running");
                    return Ok(runtime);
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
        match runtime.mode {
            InitMode::Ocelot => tracing::info!("Container init: ocelot supervise"),
            InitMode::Fallback => tracing::info!("Container init: podman --init keep-alive"),
        }

        let ai_env_path = self.ai_env_path()?;
        drop(rootfs::prepare(&self.config, &self.identity, &ai_env_path)?);
        if runtime.mode == InitMode::Ocelot {
            self.write_ocelot_config()?;
        }
        let spec = self.build_spec(&ai_env_path, runtime.mode)?;
        self.podman.run_detached(&spec)?;
        Ok(runtime)
    }

    fn runtime(&self) -> Runtime {
        let mode = if self.ocelot_available() { InitMode::Ocelot } else { InitMode::Fallback };
        Runtime { mode, shell: self.shell.clone() }
    }

    fn ocelot_available(&self) -> bool {
        let ai_env = self.expand_config_path(&self.config.general.ai_env_path);
        let host = rootfs::container_path_to_host(
            &self.config.general.ocelot_path,
            &self.config.general.ai_env_mount_path,
            &ai_env,
        );
        is_executable(&host)
    }

    fn write_ocelot_config(&self) -> Result<()> {
        let yaml = ocelot::render_config(&self.config)?;
        let target = self.identity.rootfs_dir.join(OCELOT_CONFIG_PATH.trim_start_matches('/'));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|_| error::CreateDirSnafu { path: parent.to_path_buf() })?;
        }
        fs::write(&target, yaml).with_context(|_| error::WriteFileSnafu { path: target })
    }

    fn ensure_fallback_serve(&self, agent_name: &str, serve: &AgentServe) -> Result<()> {
        let safe_name = agent_name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let marker = format!("{SERVE_MARKER_PREFIX}{safe_name}.started");
        let mut arguments = config::split_command(&serve.command);
        arguments.push("--port".into());
        arguments.push(self.config.server.serve_port.to_string());
        arguments.push("--hostname".into());
        arguments.push(self.config.server.serve_host.clone());
        let serve_command =
            arguments.iter().map(|argument| shell_quote(argument)).collect::<Vec<_>>().join(" ");
        let script = format!("if [ ! -f {marker} ]; then touch {marker}; exec {serve_command}; fi");
        let shell = self.init_shell();
        let command = [shell.as_str(), "-c", script.as_str()];
        let workdir = self.expand_config_path(&self.config.container.workdir);
        self.podman.exec_detached(&self.identity.container_name, &command, Some(&workdir))
    }

    fn ai_env_path(&self) -> Result<PathBuf> {
        let path = self.expand_config_path(&self.config.general.ai_env_path);
        let canonical = fs::canonicalize(&path)
            .with_context(|_| error::ResolveAiEnvSnafu { path: path.clone() })?;
        tracing::debug!("Resolved AI environment path: {}", canonical.display());
        Ok(canonical)
    }

    fn build_spec(&self, ai_env_path: &Path, mode: InitMode) -> Result<ContainerSpec> {
        let config = &self.config;
        let identity = &self.identity;
        let workdir = self.expand_config_path(&config.container.workdir);
        let init_shell = self.init_shell();
        let init_script = init_script_for(config, &init_shell, mode);

        let mut builder = ContainerSpec::builder(identity.container_name.clone())
            .hostname(format!("{}-{}", config.container.hostname_prefix, identity.name))
            .workdir(workdir)
            .user(identity.host.uid, identity.host.gid)
            .rootfs(identity.rootfs_dir.clone())
            .rootfs_overlay(config.container.rootfs_overlay)
            .init_shell(init_shell)
            .init_script(init_script)
            .init(mode == InitMode::Fallback)
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
        for (key, value) in self.build_env(&self.shell) {
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
        add(self.identity.home_dir.clone(), self.home.clone(), false);

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
        vec![Tmpfs::new(
            PathBuf::from("/var/tmp"),
            format!("rw,nosuid,size={}", self.config.container.var_tmp_size),
        )]
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
        push_env(&mut variables, "TMUX_TMPDIR", "/var/tmp".to_owned());

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
fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn init_script_for(config: &Config, init_shell: &str, mode: InitMode) -> String {
    match mode {
        InitMode::Ocelot => {
            let file = shell_quote(OCELOT_CONFIG_PATH);
            let ocelot = shell_quote(&config.general.ocelot_path);
            format!("exec {ocelot} supervise run --file {file}")
        }
        InitMode::Fallback => {
            let shell = shell_quote(init_shell);
            format!("exec {shell} -c {}", shell_quote("exec sleep infinity"))
        }
    }
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::{App, InitMode, init_script_for};
    use crate::{
        config::Config,
        ocelot::render_config,
        workspace::{HostIdentity, ProjectIdentity},
    };

    fn app() -> App {
        let home = PathBuf::from("/home/tester");
        let identity = ProjectIdentity {
            project_dir: PathBuf::from("/work/project"),
            name: "project".into(),
            hash: "deadbeef".into(),
            container_name: "agentbox-project-deadbeef".into(),
            tmp_dir: PathBuf::from("/tmp/agentbox-tmp-1000-project-deadbeef"),
            home_dir: PathBuf::from("/tmp/agentbox-home-1000-project-deadbeef"),
            meta_dir: PathBuf::from("/tmp/agentbox-meta-1000-project-deadbeef"),
            rootfs_dir: PathBuf::from("/tmp/agentbox-meta-1000-project-deadbeef/rootfs"),
            host: HostIdentity {
                uid: 1000,
                gid: 1000,
                username: "tester".into(),
                home: home.clone(),
            },
        };
        let mut config = Config::default();
        config.nix.enable_daemon_mount = false;
        config.nix.mount_kernel_modules = false;
        App::new(config, identity, home)
    }

    #[test]
    fn home_mount_precedes_nested_state_mounts() {
        let app = app();
        let mounts = app.build_mounts(Path::new("/nix/store/ai-env"));
        let home_index =
            mounts.iter().position(|mount| mount.container == app.home).expect("home mount");
        let agents_index = mounts
            .iter()
            .position(|mount| mount.container == app.home.join(".agents"))
            .expect("state mount");

        assert_eq!(mounts[home_index].host, app.identity.home_dir);
        assert!(home_index < agents_index);
    }

    #[test]
    fn only_var_tmp_uses_tmpfs() {
        let tmpfs = app().build_tmpfs();
        assert_eq!(tmpfs.len(), 1);
        assert_eq!(tmpfs[0].target, PathBuf::from("/var/tmp"));
    }

    #[test]
    fn tmux_sockets_are_kept_on_tmpfs() {
        let env = app().build_env("/bin/sh");
        assert!(env.contains(&("TMUX_TMPDIR".to_owned(), "/var/tmp".to_owned())));
    }

    #[test]
    fn ocelot_init_script_only_execs_supervise() {
        let config = Config::default();
        let script = init_script_for(&config, "/run/ai-env/bin/bash", InitMode::Ocelot);

        assert_eq!(
            script,
            "exec '/run/ai-env/bin/ocelot' supervise run --file '/etc/agentbox/ocelot.yaml'"
        );
        assert!(!script.contains("mkdir"));
        assert!(!script.contains("cat >"));
    }

    #[test]
    fn fallback_init_script_uses_keepalive() {
        let config = Config::default();
        let script = init_script_for(&config, "/run/ai-env/bin/bash", InitMode::Fallback);

        assert_eq!(script, "exec '/run/ai-env/bin/bash' -c 'exec sleep infinity'");
    }

    #[test]
    fn write_ocelot_config_places_yaml_in_rootfs() {
        let root = std::env::temp_dir()
            .join(format!("agentbox-ocelot-config-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&root));
        fs::create_dir_all(&root).expect("create temp rootfs");

        let mut app = app();
        app.identity.rootfs_dir = root.clone();
        app.write_ocelot_config().expect("write ocelot config");

        let written = fs::read_to_string(root.join("etc/agentbox/ocelot.yaml"))
            .expect("read generated ocelot config");
        let value: serde_yaml::Value = serde_yaml::from_str(&written).expect("parse yaml");
        assert_eq!(value["version"].as_str(), Some("1.0"));

        fs::remove_dir_all(&root).expect("cleanup temp rootfs");
    }

    #[test]
    fn rendered_ocelot_config_uses_generic_server_settings() {
        let yaml = render_config(&Config::default()).expect("ocelot config");
        let value: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("parse ocelot config");
        let opencode = &value["processes"]["opencode"];
        let arguments = opencode["arguments"].as_sequence().expect("arguments");

        assert_eq!(value["version"].as_str(), Some("1.0"));
        assert_eq!(value["processes"]["keeper"]["arguments"][0].as_str(), Some("idle"));
        assert_eq!(opencode["program"].as_str(), Some("opencode"));
        assert_eq!(opencode["workingDirectory"].as_str(), Some("/workspace"));
        assert_eq!(opencode["readinessProbe"]["handler"]["type"].as_str(), Some("tcpSocket"));
        assert_eq!(opencode["readinessProbe"]["handler"]["host"].as_str(), Some("127.0.0.1"));
        assert_eq!(opencode["readinessProbe"]["handler"]["port"].as_u64(), Some(4096));
        assert_eq!(opencode["dependsOn"]["keeper"]["condition"].as_str(), Some("Started"));
        assert!(arguments.iter().any(|argument| argument.as_str() == Some("--port")));
        assert!(arguments.iter().any(|argument| argument.as_str() == Some("4096")));
    }
}
