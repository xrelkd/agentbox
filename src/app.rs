use std::{
    fmt::Write,
    fs::{self, Permissions},
    io::{self, IsTerminal},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use indicatif::ProgressBar;
use snafu::{OptionExt, ResultExt};

use crate::{
    config::{self, AgentServe, Config},
    consts::{CONTAINER_LOOPBACK, OCELOT_CONFIG_PATH, SERVE_MARKER_PREFIX},
    error::{self, Result},
    podman::{ContainerSpec, ContainerStatus, PodmanClient, TcpProbe},
    rootfs,
    session::{
        ContainerPlan, InitMode, Runtime, TargetRequirement, init_script_for, render_config,
        resolve_tool, select_container, select_shell,
    },
    shell::shell_quote,
    ui,
    workspace::{self, ProjectIdentity},
};

mod report;

#[cfg(test)]
mod tests;

fn agent_candidates(config: &Config) -> Vec<String> {
    let mut names = config.agents.keys().cloned().collect::<Vec<_>>();
    names.push("shell".to_owned());
    names.sort();
    names.dedup();
    names
}

/// Orchestrates project setup and every agentbox subcommand.
pub struct App {
    config: Config,
    identity: ProjectIdentity,
    podman: PodmanClient,
    home: PathBuf,
    shell: OnceLock<String>,
    config_path: PathBuf,
}

impl App {
    /// Builds an application context from loaded configuration and project
    /// identity.
    pub fn new(
        config: Config,
        identity: ProjectIdentity,
        home: PathBuf,
        config_path: PathBuf,
    ) -> Self {
        let podman = PodmanClient::new(config.general.podman_binary.clone());
        Self { config, identity, podman, home, shell: OnceLock::new(), config_path }
    }

    fn shell(&self) -> &str {
        self.shell.get_or_init(|| {
            let ai_env =
                config::expand_path(Path::new(&self.config.general.ai_env_path), &self.home);
            select_shell(&self.config, &ai_env)
        })
    }

    /// Prints the host temporary directory for the current project.
    pub fn print_temporary_dir(&self) {
        println!("{}", self.identity.tmp_dir.display());
    }

    /// Prints the host directory mounted as the persistent container home.
    pub fn print_home_dir(&self) {
        println!("{}", self.identity.home_dir.display());
    }

    /// Resolves the container a non-start command should operate on.
    pub fn resolve_target(
        &self,
        explicit: Option<&str>,
        requirement: TargetRequirement,
    ) -> Result<Self> {
        let candidates = self.podman.list_containers(&self.config.general.container_prefix)?;
        let selected =
            select_container(candidates, explicit, &self.identity.container_name, requirement)?;

        let workdir = self.expand_config_path(&self.config.container.workdir);
        let project_dir =
            self.podman.container_mount_source(&selected.name, &workdir)?.with_context(|| {
                error::ProjectDirectoryNotFoundSnafu { name: selected.name.clone() }
            })?;
        let identity = ProjectIdentity::from_container(
            &selected.name,
            project_dir,
            &self.identity.host,
            &self.home,
            &self.config,
        )?;
        Ok(Self::new(self.config.clone(), identity, self.home.clone(), self.config_path.clone()))
    }

    /// Stops the project container and removes its transient directories.
    pub fn stop_project(&self, purge_home: bool) -> Result<()> {
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

        if self.identity.runtime_dir.exists() {
            match fs::remove_dir(&self.identity.runtime_dir) {
                Ok(()) => {
                    tracing::info!("Cleaned up directory: {}", self.identity.runtime_dir.display());
                }
                Err(source) if source.kind() == io::ErrorKind::DirectoryNotEmpty => {}
                Err(source) => {
                    tracing::debug!(
                        "Could not remove {}: {source}",
                        self.identity.runtime_dir.display()
                    );
                }
            }
        }
        Ok(())
    }

    /// Starts the container, optionally attaching to an agent afterwards.
    pub fn start(&self, agent: Option<&str>, attach: bool) -> Result<()> {
        workspace::validate_forbidden_dir(&self.identity.project_dir, &self.home, &self.config)?;
        let runtime = self.start_container()?;
        if runtime.mode == InitMode::Fallback
            && let Some(name) = self.startup_serve_agent(agent)
        {
            let serve = self
                .config
                .agents
                .get(&name)
                .and_then(|configured| configured.serve.clone())
                .with_context(|| error::AgentHasNoServeSnafu { name: name.clone() })?;
            self.ensure_fallback_serve(&name, &serve)?;
        }
        if !attach {
            return Ok(());
        }
        self.launch_agent_named(agent)
    }

    pub fn launch_agent_named(&self, requested: Option<&str>) -> Result<()> {
        let name = match requested {
            Some(name) => name.to_owned(),
            None => self.resolve_agent_name()?,
        };
        if name == "shell" { self.attach_shell() } else { self.launch_agent(&name) }
    }

    fn resolve_agent_name(&self) -> Result<String> {
        let candidates = agent_candidates(&self.config);
        ui::fuzzy::select_or_default("Agent: ", &candidates, &self.config.general.default_agent)
    }

    /// Prints the agent selected by the default chain or interactive picker.
    pub fn print_agent_choice(&self) -> Result<()> {
        println!("{}", self.resolve_agent_name()?);
        Ok(())
    }

    /// Attaches to an agent by name, using its server when one is configured.
    pub fn launch_agent(&self, agent_name: &str) -> Result<()> {
        let has_serve =
            self.config.agents.get(agent_name).and_then(|agent| agent.serve.as_ref()).is_some();
        if has_serve { self.attach_server(agent_name) } else { self.attach_agent(agent_name) }
    }

    /// Attaches to a configured agent server from inside the container.
    fn attach_server(&self, agent_name: &str) -> Result<()> {
        let serve = self
            .config
            .agents
            .get(agent_name)
            .with_context(|| error::UnknownAgentSnafu { name: agent_name.to_owned() })?
            .serve
            .as_ref()
            .with_context(|| error::AgentHasNoServeSnafu { name: agent_name.to_owned() })?;
        let runtime = self.require_running_container()?;
        self.wait_for_server(agent_name, &runtime)?;

        let command = self.build_attach_command(agent_name, serve)?;
        let arguments = command.iter().map(String::as_str).collect::<Vec<_>>();
        let workdir = self.expand_config_path(&self.config.container.workdir);

        tracing::info!("Attaching to the {agent_name} server from inside the container...");
        self.podman.exec_interactive(&self.identity.container_name, &arguments, Some(&workdir))
    }

    /// Attaches to an interactive agent session in the running container.
    fn attach_agent(&self, agent_name: &str) -> Result<()> {
        let agent = self
            .config
            .agents
            .get(agent_name)
            .with_context(|| error::UnknownAgentSnafu { name: agent_name.to_owned() })?;
        drop(self.require_running_container()?);
        let name = &self.identity.container_name;
        let workdir = self.expand_config_path(&self.config.container.workdir);
        let arguments = config::split_command(&agent.command)?;
        let command = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        self.podman.exec_interactive(name, &command, Some(&workdir))
    }

    /// Attaches to a plain interactive shell.
    pub fn attach_shell(&self) -> Result<()> {
        let runtime = self.require_running_container()?;
        let name = &self.identity.container_name;
        let workdir = self.expand_config_path(&self.config.container.workdir);
        let arguments = [runtime.shell.as_str(), "-i"];
        self.podman.exec_interactive(name, &arguments, Some(&workdir))
    }

    fn wait_for_server(&self, agent_name: &str, runtime: &Runtime) -> Result<()> {
        let name = &self.identity.container_name;
        let probe = TcpProbe {
            shell: runtime.probe_shell.clone(),
            host: self.config.server.serve_host.clone(),
            port: self.config.server.serve_port,
            timeout: Duration::from_secs(self.config.server.startup_timeout_secs),
            interval: Duration::from_millis(self.config.server.poll_interval_ms),
        };
        let spinner = io::stderr().is_terminal().then(|| {
            let bar = ProgressBar::new_spinner();
            bar.set_message(format!("Waiting for {agent_name} server in {name}..."));
            bar.enable_steady_tick(Duration::from_millis(100));
            bar
        });
        let result = self.podman.wait_for_tcp_ready(name, &probe);
        if let Some(bar) = spinner {
            if result.is_ok() {
                bar.finish_with_message(format!("{agent_name} server in {name} ready"));
            } else {
                bar.finish_and_clear();
            }
        }
        result
    }

    fn require_running_container(&self) -> Result<Runtime> {
        let name = &self.identity.container_name;
        snafu::ensure!(
            self.podman.container_exists(name)?,
            error::ContainerNotRunningSnafu { name: name.clone(), status: "missing".to_owned() }
        );
        let status = self.podman.container_status(name)?;
        snafu::ensure!(
            status == ContainerStatus::Running,
            error::ContainerNotRunningSnafu { name: name.clone(), status: status.to_string() }
        );
        Ok(self.runtime())
    }

    fn startup_serve_agent(&self, requested: Option<&str>) -> Option<String> {
        [requested, Some(self.config.general.default_agent.as_str()), Some("opencode")]
            .into_iter()
            .flatten()
            .find(|name| self.config.agents.get(*name).is_some_and(|agent| agent.serve.is_some()))
            .map(str::to_owned)
    }

    fn start_container(&self) -> Result<Runtime> {
        let name = &self.identity.container_name;
        let runtime = self.runtime();
        if self.podman.container_exists(name)? {
            match self.podman.container_status(name)? {
                ContainerStatus::Running => {
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
            self.write_ocelot_config(&ai_env_path)?;
        }
        let spec = self.build_spec(&ai_env_path, runtime.mode)?;
        self.podman.run_detached(&spec)?;
        Ok(runtime)
    }

    fn runtime(&self) -> Runtime { Runtime::detect(&self.config, &self.home, self.shell()) }

    fn build_attach_command(&self, agent_name: &str, serve: &AgentServe) -> Result<Vec<String>> {
        let mut command = config::split_command(&serve.attach)?;
        snafu::ensure!(
            !command.is_empty(),
            error::InvalidConfigSnafu {
                message: format!("agents.{agent_name}.serve.attach must not be empty")
            }
        );
        command.push(format!("http://{CONTAINER_LOOPBACK}:{}", self.config.server.serve_port));
        Ok(command)
    }

    fn write_ocelot_config(&self, ai_env_path: &Path) -> Result<()> {
        let (ocelot, _) = resolve_tool(&self.config.general.ocelot_path, ai_env_path);
        let yaml = render_config(&self.config, &ocelot)?;
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
        let mut arguments = config::split_command(&serve.command)?;
        arguments.push("--port".into());
        arguments.push(self.config.server.serve_port.to_string());
        arguments.push("--hostname".into());
        arguments.push(self.config.server.listen_host.clone());
        let serve_command =
            arguments.iter().map(|argument| shell_quote(argument)).collect::<Vec<_>>().join(" ");
        let script = format!("if [ ! -f {marker} ]; then touch {marker}; exec {serve_command}; fi");
        let shell = Runtime::init_shell();
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
        let init_shell = Runtime::init_shell();
        let (ocelot, _) = resolve_tool(&config.general.ocelot_path, ai_env_path);
        let init_script = init_script_for(&init_shell, mode, &ocelot);
        let plan = ContainerPlan::new(config, identity, &self.home, ai_env_path, self.shell());

        {
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
                .memory_limit(config.container.memory_limit.clone())
                .inherit_proxy(config.container.inherit_proxy);
            if let Some(limit) = &config.container.memory_swap {
                builder = builder.memory_swap(limit);
            }
            if let Some(cpus) = &config.container.cpus {
                builder = builder.cpus(cpus);
            }
            for (name, value) in &config.container.ulimits {
                builder = builder.ulimit(name, value);
            }

            for mount in plan.mounts() {
                builder = builder.mount(mount.clone());
            }
            for tmpfs in plan.tmpfs() {
                builder = builder.tmpfs(tmpfs.clone());
            }
            for (key, value) in plan.env() {
                builder = builder.env(key.clone(), value.clone());
            }
            builder = builder.env_file(self.write_secret_env_file()?);
            builder.build()
        }
    }

    fn write_secret_env_file(&self) -> Result<PathBuf> {
        let path = self.identity.tmp_dir.join("secrets.env");
        let secrets_base = self.expand_config_path(&self.config.secrets.base_dir);
        let mut content = String::new();
        for (key, relative) in &self.config.secrets.entries {
            config::validate_secret_key(key)?;
            let secret_path = secrets_base.join(relative);
            let Ok(value) = fs::read_to_string(&secret_path) else {
                tracing::debug!("Skipping unreadable secret {}", secret_path.display());
                continue;
            };
            let value = value.trim_end_matches(['\n', '\r']);
            if value.contains(['\n', '\r']) {
                tracing::warn!("Skipping multi-line secret {key}");
                continue;
            }
            let _ = writeln!(content, "{key}={value}");
        }
        fs::write(&path, content).with_context(|_| error::WriteFileSnafu { path: path.clone() })?;
        fs::set_permissions(&path, Permissions::from_mode(0o600))
            .with_context(|_| error::SetPermissionsSnafu { path: path.clone() })?;
        Ok(path)
    }

    #[inline]
    fn expand_config_path(&self, raw: &str) -> PathBuf {
        config::expand_path(Path::new(raw), &self.home)
    }
}
