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

pub use crate::ui::report::{ContainerRow, ListReport, MountSummary, ProjectReport};
use crate::{
    config::{self, AgentServe, Config},
    consts::{CONTAINER_LOOPBACK, OCELOT_CONFIG_PATH, SERVE_MARKER_PREFIX},
    container_plan::ContainerPlan,
    error::{self, Result},
    ocelot,
    output::OutputFormat,
    podman::{ContainerInfo, ContainerSpec, ContainerStatus, PodmanClient, TcpProbe, shell_quote},
    rootfs,
    runtime::{InitMode, Runtime, init_script_for},
    ui,
    workspace::{self, ProjectIdentity},
};

/// Required state of a container selected outside its project directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetRequirement {
    /// Accept managed containers in any state.
    Any,
    /// Accept only running managed containers.
    Running,
}

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
            rootfs::select_shell(&self.config, &ai_env)
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

    /// Lists agentbox-managed containers and the current project directory.
    pub fn list_containers(&self, format: OutputFormat, quiet: bool) -> Result<()> {
        let rows = self.collect_rows()?;
        if quiet {
            for row in &rows {
                println!("{}", row.info.name);
            }
            return Ok(());
        }

        let project = self.project_report(&rows);
        match format {
            OutputFormat::Table => print!("{}", ui::table::render_list(&rows, &project)),
            OutputFormat::Json => {
                let report = ListReport { containers: rows, project };
                let json =
                    serde_json::to_string_pretty(&report).context(error::SerializeOutputSnafu)?;
                println!("{json}");
            }
        }
        Ok(())
    }

    /// Prints derived information about the current project directory.
    pub fn print_status(&self, format: OutputFormat, quiet: bool) -> Result<()> {
        if quiet {
            println!("{}", self.identity.container_name);
            return Ok(());
        }

        let project = self.project_report(&self.collect_rows()?);
        match format {
            OutputFormat::Table => print!("{}", ui::table::render_status(&project)),
            OutputFormat::Json => {
                let json =
                    serde_json::to_string_pretty(&project).context(error::SerializeOutputSnafu)?;
                println!("{json}");
            }
        }
        Ok(())
    }

    fn collect_rows(&self) -> Result<Vec<ContainerRow>> {
        let prefix = &self.config.general.container_prefix;
        Ok(self
            .podman
            .list_containers(prefix)?
            .into_iter()
            .map(|info| ContainerRow { current: info.name == self.identity.container_name, info })
            .collect())
    }

    /// Resolves the container a non-start command should operate on.
    pub fn resolve_target(
        &self,
        explicit: Option<&str>,
        requirement: TargetRequirement,
    ) -> Result<Self> {
        let mut candidates = self
            .podman
            .list_containers(&self.config.general.container_prefix)?
            .into_iter()
            .filter(|info| {
                requirement != TargetRequirement::Running || Self::container_is_running(info)
            })
            .collect::<Vec<_>>();
        snafu::ensure!(!candidates.is_empty(), error::NoManagedContainersSnafu);

        let selected = match explicit {
            Some(name) => candidates
                .into_iter()
                .find(|info| info.name == name)
                .with_context(|| error::ContainerNotFoundSnafu { name: name.to_owned() })?,
            None if candidates.iter().any(|info| info.name == self.identity.container_name) => {
                let current = self.identity.container_name.clone();
                candidates
                    .into_iter()
                    .find(|info| info.name == current)
                    .with_context(|| error::ContainerNotFoundSnafu { name: current })?
            }
            None => {
                if candidates.len() == 1 {
                    candidates.remove(0)
                } else if !io::stdin().is_terminal() {
                    let names = candidates
                        .iter()
                        .map(|info| info.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(error::Error::AmbiguousContainer { names });
                } else {
                    Self::pick_container(&candidates)?
                }
            }
        };

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

    fn pick_container(candidates: &[ContainerInfo]) -> Result<ContainerInfo> {
        let items = candidates
            .iter()
            .map(|info| {
                let status = if info.status.is_empty() { &info.state } else { &info.status };
                ui::fuzzy::PickerItem::new(format!("{}  [{status}]", info.name), info.name.clone())
            })
            .collect::<Vec<_>>();
        let selected = ui::fuzzy::select_value("Container: ", &items)?
            .with_context(|| error::ContainerPickerCancelledSnafu)?;
        candidates
            .iter()
            .find(|info| info.name == selected)
            .cloned()
            .with_context(|| error::ContainerNotFoundSnafu { name: selected })
    }

    fn container_is_running(info: &ContainerInfo) -> bool {
        ContainerStatus::from(info.state.as_str()).is_running()
            || info.status.to_ascii_lowercase().starts_with("up")
    }

    fn project_report(&self, rows: &[ContainerRow]) -> ProjectReport {
        let (container_state, container_exit_code) =
            rows.iter().find(|row| row.current).map_or_else(
                || (String::from("absent"), String::new()),
                |row| {
                    let state =
                        if row.info.state.is_empty() { &row.info.status } else { &row.info.state };
                    (state.clone(), row.info.exit_code.clone())
                },
            );

        let ai_env = self.expand_config_path(&self.config.general.ai_env_path);
        let plan =
            ContainerPlan::new(&self.config, &self.identity, &self.home, &ai_env, self.shell());
        let mounts = plan
            .mounts()
            .iter()
            .map(|mount| MountSummary {
                host: mount.host.display().to_string(),
                container: mount.container.display().to_string(),
                read_only: mount.read_only,
            })
            .collect();
        let mut env = plan.env_keys().cloned().collect::<Vec<_>>();
        env.extend(self.config.secrets.entries.keys().cloned());

        ProjectReport {
            dir: self.identity.project_dir.display().to_string(),
            name: self.identity.name.clone(),
            hash: self.identity.hash.clone(),
            container: self.identity.container_name.clone(),
            container_state,
            container_exit_code,
            hostname: format!("{}-{}", self.config.container.hostname_prefix, self.identity.name),
            home: self.identity.home_dir.display().to_string(),
            tmp: self.identity.tmp_dir.display().to_string(),
            meta: self.identity.meta_dir.display().to_string(),
            rootfs: self.identity.rootfs_dir.display().to_string(),
            config: self.config_path.display().to_string(),
            user: self.identity.host.username.clone(),
            uid: self.identity.host.uid,
            gid: self.identity.host.gid,
            shell: self.shell().to_owned(),
            init: self.runtime().mode.as_str().to_owned(),
            mounts,
            env,
        }
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
    pub fn attach_server(&self, agent_name: &str) -> Result<()> {
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
    pub fn attach_agent(&self, agent_name: &str) -> Result<()> {
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
        let (ocelot, _) = rootfs::resolve_tool(&self.config.general.ocelot_path, ai_env_path);
        let yaml = ocelot::render_config(&self.config, &ocelot)?;
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
        let (ocelot, _) = rootfs::resolve_tool(&config.general.ocelot_path, ai_env_path);
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
            snafu::ensure!(
                !key.is_empty()
                    && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                error::InvalidConfigSnafu {
                    message: format!("secrets.entries key `{key}` is not a valid env name")
                }
            );
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

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use super::{
        App, ContainerRow, InitMode, ListReport, MountSummary, ProjectReport, TargetRequirement,
        agent_candidates, init_script_for,
    };
    use crate::{
        config::{AgentServe, Config, MountEntry},
        container_plan::ContainerPlan,
        error::Error,
        ocelot::render_config,
        podman::{ContainerInfo, PodmanClient},
        ui::table::{render_list, render_status},
        workspace::{HostIdentity, ProjectIdentity},
    };

    fn app() -> App {
        let home = PathBuf::from("/home/tester");
        let identity = ProjectIdentity {
            project_dir: PathBuf::from("/work/project"),
            name: "project".into(),
            hash: "deadbeef".into(),
            container_name: "agentbox-project-deadbeef".into(),
            runtime_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef"),
            tmp_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/tmp"),
            home_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/home"),
            meta_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/meta"),
            rootfs_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/meta/rootfs"),
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
        App::new(config, identity, home, PathBuf::from("/home/tester/.config/agentbox/config.yaml"))
    }

    fn container_plan(app: &App, ai_env_path: &Path) -> ContainerPlan {
        ContainerPlan::new(&app.config, &app.identity, &app.home, ai_env_path, app.shell())
    }

    fn fake_podman(directory: &Path, body: &str) -> String {
        let binary = directory.join("podman");
        fs::write(&binary, format!("#!/bin/sh\n{body}\n")).expect("write fake podman");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("set executable");
        binary.to_string_lossy().into_owned()
    }

    fn use_fake_podman(app: &mut App, directory: &Path, body: &str) {
        let binary = fake_podman(directory, body);
        app.config.general.podman_binary = binary.clone();
        app.podman = PodmanClient::new(binary);
    }

    #[test]
    fn agent_candidates_include_shell_and_are_sorted() {
        assert_eq!(agent_candidates(&Config::default()), ["claude", "gemini", "opencode", "shell"]);
    }

    #[test]
    fn agent_choice_uses_default_without_tty() {
        assert_eq!(app().resolve_agent_name().expect("resolve agent"), "shell");
    }

    #[test]
    fn shell_selection_is_lazy() {
        let mut app = app();
        app.config.general.shell = "/definitely/missing-shell".into();
        app.config.general.ai_env_path = "/definitely/missing-env".into();
        assert!(app.shell.get().is_none());
        assert_eq!(app.shell(), "/bin/sh");
        assert!(app.shell.get().is_some());
    }

    #[test]
    fn home_mount_precedes_nested_state_mounts() {
        let mut app = app();
        let temp = env::temp_dir();
        app.config.mounts =
            vec![MountEntry::new(temp.to_string_lossy(), "/home/tester/.agents", true)];
        let plan = container_plan(&app, Path::new("/nix/store/ai-env"));
        let mounts = plan.mounts();
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
    fn missing_mount_source_is_skipped() {
        let mut app = app();
        let missing =
            env::temp_dir().join(format!("agentbox-missing-mount-{}", std::process::id()));
        drop(fs::remove_dir_all(&missing));
        let temp = env::temp_dir();
        app.config.mounts = vec![
            MountEntry::new(missing.to_string_lossy(), "/home/tester/missing", false),
            MountEntry::new(temp.to_string_lossy(), "/home/tester/present", true),
        ];

        let plan = container_plan(&app, Path::new("/nix/store/ai-env"));
        let mounts = plan.mounts();
        assert!(!mounts.iter().any(|mount| mount.container == app.home.join("missing")));
        assert!(mounts.iter().any(|mount| mount.container == app.home.join("present")));
    }

    #[test]
    fn temporary_directories_are_sized_tmpfs_mounts() {
        let app = app();
        let plan = container_plan(&app, Path::new("/nix/store/ai-env"));
        let tmpfs = plan.tmpfs();
        assert_eq!(tmpfs.len(), 2);
        assert_eq!(tmpfs[0].target, PathBuf::from("/tmp"));
        assert_eq!(tmpfs[0].options, "rw,nosuid,nodev,mode=1777,size=2g");
        assert_eq!(tmpfs[1].target, PathBuf::from("/var/tmp"));
        assert_eq!(tmpfs[1].options, "rw,nosuid,nodev,noexec,mode=1777,size=2g");
    }

    #[test]
    fn configuration_directories_are_mounted_read_only() {
        let mut app = app();
        let temp = env::temp_dir();
        app.config.mounts =
            vec![MountEntry::new(temp.to_string_lossy(), "/home/tester/.claude", true)];
        let plan = container_plan(&app, Path::new("/nix/store/ai-env"));
        let mounts = plan.mounts();
        let config = mounts
            .iter()
            .find(|mount| mount.container == Path::new("/home/tester/.claude"))
            .expect("claude config mount");
        assert!(config.read_only);

        let nix = mounts
            .iter()
            .find(|mount| mount.container == Path::new("/home/tester/.config/nix"))
            .expect("nix config mount");
        assert!(nix.read_only);
        assert!(nix.host.starts_with(&app.identity.home_dir));
    }

    #[test]
    fn secret_env_file_is_private_and_omits_values_from_args() {
        let root = env::temp_dir().join(format!("agentbox-env-file-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&root));
        fs::create_dir_all(root.join("secrets")).expect("create secrets dir");
        fs::write(root.join("secrets/token"), "s3cr3t\n").expect("write secret");

        let mut app = app();
        app.config.secrets.base_dir = root.join("secrets").to_string_lossy().into_owned();
        app.config.secrets.entries.clear();
        drop(app.config.secrets.entries.insert("TEST_TOKEN".into(), "token".into()));
        app.identity.tmp_dir = root.join("tmp");
        fs::create_dir_all(&app.identity.tmp_dir).expect("create tmp dir");

        let path = app.write_secret_env_file().expect("write env file");
        let content = fs::read_to_string(&path).expect("read env file");
        assert_eq!(content, "TEST_TOKEN=s3cr3t\n");
        assert_eq!(fs::metadata(&path).expect("metadata").permissions().mode() & 0o777, 0o600);

        let report = app.project_report(&[]);
        assert!(report.env.contains(&"TEST_TOKEN".to_owned()));
        let json = serde_json::to_string(&report).expect("serialize project report");
        assert!(!json.contains("s3cr3t"));

        drop(fs::remove_dir_all(&root));
    }

    #[test]
    fn ocelot_init_script_only_execs_supervise() {
        let config = Config::default();
        let (ocelot, _) =
            crate::rootfs::resolve_tool(&config.general.ocelot_path, Path::new("/opt/ai-env"));
        let script = init_script_for("/run/ai-env/bin/bash", InitMode::Ocelot, &ocelot);

        assert_eq!(
            script,
            "exec '/run/ai-env/bin/ocelot' supervise run --file '/etc/agentbox/ocelot.yaml'"
        );
        assert!(!script.contains("mkdir"));
        assert!(!script.contains("cat >"));
    }

    #[test]
    fn probe_uses_bash_regardless_of_configured_shell() {
        let mut config = Config::default();
        config.general.shell = "zsh".into();
        let home = PathBuf::from("/home/tester");
        let identity = app().identity;
        let app = App::new(config, identity, home, PathBuf::from("/config.yaml"));

        assert_eq!(app.runtime().probe_shell, "/run/ai-env/bin/bash");
    }

    #[test]
    fn attach_command_uses_container_client_and_loopback_url() {
        let mut app = app();
        let serve = app.config.agents["opencode"].serve.as_ref().expect("opencode serve");
        assert_eq!(
            app.build_attach_command("opencode", serve).expect("attach command"),
            ["opencode", "attach", "http://localhost:4096"]
        );

        app.config.server.serve_port = 5000;
        assert_eq!(
            app.build_attach_command("opencode", serve)
                .expect("attach command")
                .last()
                .map(String::as_str),
            Some("http://localhost:5000")
        );
        assert!(
            app.build_attach_command("opencode", &AgentServe::new("opencode serve", "  ")).is_err()
        );
    }

    #[test]
    fn attach_requires_an_already_running_container() {
        let directory =
            env::temp_dir().join(format!("agentbox-attach-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&directory));
        fs::create_dir_all(&directory).expect("create temp dir");
        let binary = directory.join("podman");
        fs::write(&binary, "#!/bin/sh\nexit 1\n").expect("write fake podman");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("set executable");

        let mut config = Config::default();
        config.general.podman_binary = binary.to_string_lossy().into_owned();
        let identity = app().identity;
        let app = App::new(
            config,
            identity,
            PathBuf::from("/home/tester"),
            PathBuf::from("/config.yaml"),
        );

        let error = app.require_running_container().expect_err("attach must not start a container");
        assert!(matches!(error, Error::ContainerNotRunning { .. }));

        drop(fs::remove_dir_all(&directory));
    }

    #[test]
    fn fallback_init_script_uses_keepalive() {
        let script =
            init_script_for("/run/ai-env/bin/bash", InitMode::Fallback, "/run/ai-env/bin/ocelot");

        assert_eq!(script, "exec '/run/ai-env/bin/bash' -c 'exec sleep infinity'");
    }

    #[test]
    fn write_ocelot_config_places_yaml_in_rootfs() {
        let root =
            env::temp_dir().join(format!("agentbox-ocelot-config-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&root));
        fs::create_dir_all(&root).expect("create temp rootfs");

        let mut app = app();
        app.identity.rootfs_dir = root.clone();
        app.write_ocelot_config(Path::new("/opt/ai-env")).expect("write ocelot config");

        let written = fs::read_to_string(root.join("etc/agentbox/ocelot.yaml"))
            .expect("read generated ocelot config");
        let value: serde_yaml::Value = serde_yaml::from_str(&written).expect("parse yaml");
        assert_eq!(value["version"].as_str(), Some("1.0"));
        assert_eq!(
            value["processes"]["keeper"]["program"].as_str(),
            Some("/run/ai-env/bin/ocelot")
        );

        fs::remove_dir_all(&root).expect("cleanup temp rootfs");
    }

    #[test]
    fn rendered_ocelot_config_uses_generic_server_settings() {
        let yaml =
            render_config(&Config::default(), "/run/ai-env/bin/ocelot").expect("ocelot config");
        let value: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("parse ocelot config");
        let opencode = &value["processes"]["opencode"];
        let arguments = opencode["arguments"].as_sequence().expect("arguments");

        assert_eq!(value["version"].as_str(), Some("1.0"));
        assert_eq!(
            value["processes"]["keeper"]["program"].as_str(),
            Some("/run/ai-env/bin/ocelot")
        );
        assert_eq!(value["processes"]["keeper"]["arguments"][0].as_str(), Some("idle"));
        assert_eq!(opencode["program"].as_str(), Some("opencode"));
        assert_eq!(opencode["workingDirectory"].as_str(), Some("/workspace"));
        assert_eq!(opencode["readinessProbe"]["handler"]["type"].as_str(), Some("tcpSocket"));
        assert_eq!(opencode["readinessProbe"]["handler"]["host"].as_str(), Some("127.0.0.1"));
        assert_eq!(opencode["readinessProbe"]["handler"]["port"].as_u64(), Some(4096));
        assert_eq!(opencode["dependsOn"]["keeper"]["condition"].as_str(), Some("Started"));
        assert!(arguments.iter().any(|argument| argument.as_str() == Some("--port")));
        assert!(arguments.iter().any(|argument| argument.as_str() == Some("4096")));
        assert!(arguments.iter().any(|argument| argument.as_str() == Some("--hostname")));
        assert!(arguments.iter().any(|argument| argument.as_str() == Some("127.0.0.1")));
    }

    fn container_row(name: &str, current: bool) -> ContainerRow {
        ContainerRow {
            info: ContainerInfo {
                id: "abc123".into(),
                name: name.into(),
                status: "Up 2 minutes".into(),
                state: "running".into(),
                exit_code: String::new(),
                created: "2 minutes ago".into(),
                running_for: "2 minutes".into(),
                ports: String::new(),
            },
            current,
        }
    }

    fn project_report() -> ProjectReport {
        ProjectReport {
            dir: "/work/project".into(),
            name: "project".into(),
            hash: "deadbeef".into(),
            container: "agentbox-project-deadbeef".into(),
            container_state: "running".into(),
            container_exit_code: String::new(),
            hostname: "sandbox-project".into(),
            home: "/tmp/agentbox/1000-project-deadbeef/home".into(),
            tmp: "/tmp/agentbox/1000-project-deadbeef/tmp".into(),
            meta: "/tmp/agentbox/1000-project-deadbeef/meta".into(),
            rootfs: "/tmp/agentbox/1000-project-deadbeef/meta/rootfs".into(),
            config: "/home/tester/.config/agentbox/config.yaml".into(),
            user: "tester".into(),
            uid: 1000,
            gid: 1000,
            shell: "zsh".into(),
            init: "ocelot".into(),
            mounts: vec![
                MountSummary {
                    host: "/work/project".into(),
                    container: "/workspace".into(),
                    read_only: false,
                },
                MountSummary {
                    host: "/nix/store".into(),
                    container: "/nix/store".into(),
                    read_only: true,
                },
            ],
            env: vec!["HOME".into(), "PATH".into(), "TEST_TOKEN".into()],
        }
    }

    fn compact_output(output: &str) -> String {
        output.chars().filter(|ch| !ch.is_whitespace()).collect()
    }

    #[test]
    fn list_table_keeps_columns_and_adds_current_project() {
        let rows = vec![
            container_row("agentbox-project-deadbeef", true),
            container_row("agentbox-other-12345678", false),
        ];
        let output = compact_output(&render_list(&rows, &project_report()));

        assert!(output.starts_with("NAME"));
        assert!(output.contains("STATUS"));
        assert!(output.contains("PORTS"));
        assert!(output.contains("agentbox-other-12345678"));
        assert!(output.contains("CURRENTPROJECT"));
        assert!(output.contains("/tmp/agentbox/1000-project-deadbeef/home"));
        assert!(output.contains("/tmp/agentbox/1000-project-deadbeef/tmp"));
        assert!(output.contains("agentbox-project-deadbeef(running)"));
    }

    #[test]
    fn status_table_reports_identity_mounts_and_env_names() {
        let output = compact_output(&render_status(&project_report()));

        assert!(output.contains("/work/project"));
        assert!(output.contains("deadbeef"));
        assert!(output.contains("sandbox-project"));
        assert!(output.contains("MOUNTS"));
        assert!(output.contains("MODEHOSTCONTAINER"));
        assert!(output.contains("ro/nix/store/nix/store"));
        assert!(output.contains("rw/work/project/workspace"));
        assert!(output.contains("TEST_TOKEN"));
    }

    #[test]
    fn list_json_contains_containers_and_project() {
        let report = ListReport {
            containers: vec![container_row("agentbox-project-deadbeef", true)],
            project: project_report(),
        };
        let json = serde_json::to_string(&report).expect("serialize list report");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parse list report");

        assert_eq!(value["containers"][0]["name"], "agentbox-project-deadbeef");
        assert_eq!(value["containers"][0]["current"], true);
        assert_eq!(value["project"]["hash"], "deadbeef");
        assert_eq!(value["project"]["home"], "/tmp/agentbox/1000-project-deadbeef/home");
        assert_eq!(value["project"]["tmp"], "/tmp/agentbox/1000-project-deadbeef/tmp");
    }

    #[test]
    fn resolve_target_uses_explicit_container_and_grouped_paths() {
        let directory =
            env::temp_dir().join(format!("agentbox-target-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&directory));
        fs::create_dir_all(&directory).expect("create temp dir");
        let script = r#"case "$1" in
  ps) printf 'abc123\tagentbox-project-deadbeef\tUp 2 minutes\trunning\t0\t2 minutes ago\t2 minutes\t\n' ;;
  inspect) printf '%s\n' '[{"Destination":"/workspace","Source":"/work/other"}]' ;;
esac"#;
        let mut app = app();
        use_fake_podman(&mut app, &directory, script);

        let target = app
            .resolve_target(Some("agentbox-project-deadbeef"), TargetRequirement::Any)
            .expect("resolve target");

        assert_eq!(target.identity.container_name, "agentbox-project-deadbeef");
        assert_eq!(target.identity.project_dir, PathBuf::from("/work/other"));
        assert_eq!(
            target.identity.runtime_dir,
            PathBuf::from("/tmp/agentbox/1000-project-deadbeef")
        );
        assert!(target.identity.meta_dir.starts_with(&target.identity.runtime_dir));

        drop(fs::remove_dir_all(&directory));
    }

    #[test]
    fn resolve_target_requires_choice_for_multiple_containers_without_tty() {
        let directory =
            env::temp_dir().join(format!("agentbox-ambiguous-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&directory));
        fs::create_dir_all(&directory).expect("create temp dir");
        let script = r"printf 'a\tagentbox-one\tUp 1 minute\trunning\t0\t1 minute\t1 minute\t\n'
printf 'b\tagentbox-two\tUp 2 minutes\trunning\t0\t2 minutes\t2 minutes\t\n'";
        let mut app = app();
        use_fake_podman(&mut app, &directory, script);
        app.identity.container_name = "agentbox-absent-00000000".into();

        let error = app.resolve_target(None, TargetRequirement::Any).err().expect("ambiguous");
        assert!(matches!(error, Error::AmbiguousContainer { .. }));

        drop(fs::remove_dir_all(&directory));
    }

    #[test]
    fn resolve_target_filters_stopped_containers_for_attach() {
        let directory =
            env::temp_dir().join(format!("agentbox-stopped-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&directory));
        fs::create_dir_all(&directory).expect("create temp dir");
        let script = r"printf 'a\tagentbox-one\tExited\texited\t0\t1 minute\t1 minute\t\n'";
        let mut app = app();
        use_fake_podman(&mut app, &directory, script);

        let error = app
            .resolve_target(None, TargetRequirement::Running)
            .err()
            .expect("no running containers");
        assert!(matches!(error, Error::NoManagedContainers));

        drop(fs::remove_dir_all(&directory));
    }

    #[test]
    fn start_rejects_forbidden_directory_before_running_podman() {
        let directory =
            env::temp_dir().join(format!("agentbox-forbidden-start-{}", std::process::id()));
        drop(fs::remove_dir_all(&directory));
        fs::create_dir_all(&directory).expect("create temp dir");
        let marker = directory.join("podman-ran");
        let script = format!("touch {}", marker.display());
        let mut app = app();
        use_fake_podman(&mut app, &directory, &script);
        app.config.general.forbidden_dirs = vec!["/tmp".into()];
        app.identity.project_dir = PathBuf::from("/tmp/work/project");

        let error = app.start(None, false).expect_err("forbidden directory");
        assert!(matches!(error, Error::ForbiddenDirectory { .. }));
        assert!(!marker.exists());

        drop(fs::remove_dir_all(&directory));
    }
}
