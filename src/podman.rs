use std::{
    ffi::OsString,
    fmt,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt};

use crate::error::{self, Result};

/// Host-to-container bind mount.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mount {
    /// Host source path.
    pub host: PathBuf,
    /// Container destination path.
    pub container: PathBuf,
    /// Whether the mount is read-only.
    pub read_only: bool,
    /// Extra Podman mount options.
    pub options: String,
}

impl Mount {
    /// Creates a bind mount.
    pub const fn new(host: PathBuf, container: PathBuf, read_only: bool) -> Self {
        Self { host, container, read_only, options: String::new() }
    }

    /// Creates a bind mount with extra Podman options.
    pub fn with_options(
        host: PathBuf,
        container: PathBuf,
        read_only: bool,
        options: impl Into<String>,
    ) -> Self {
        Self { host, container, read_only, options: options.into() }
    }

    fn to_arg(&self) -> OsString {
        let mode = if self.read_only { "ro" } else { "rw" };
        let options = if self.options.is_empty() {
            mode.to_owned()
        } else {
            format!("{mode},{}", self.options)
        };
        format!("{}:{}:{}", self.host.display(), self.container.display(), options).into()
    }
}

/// Container tmpfs declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tmpfs {
    /// Container mount point.
    pub target: PathBuf,
    /// Podman tmpfs options.
    pub options: String,
}

impl Tmpfs {
    /// Creates a tmpfs declaration.
    pub fn new(target: PathBuf, options: impl Into<String>) -> Self {
        Self { target, options: options.into() }
    }

    fn to_arg(&self) -> OsString { format!("{}:{}", self.target.display(), self.options).into() }
}

/// Parsed container state from `podman inspect`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContainerStatus {
    /// Container was created but never started.
    Created,
    /// Container is currently running.
    Running,
    /// Container is paused.
    Paused,
    /// Container exited.
    Exited,
    /// Container is stopped.
    Stopped,
    /// Container is being removed.
    Removing,
    /// Unrecognized Podman state.
    Other(String),
}

impl ContainerStatus {
    /// Returns true only for the running state.
    pub fn is_running(&self) -> bool { *self == Self::Running }
}

impl fmt::Display for ContainerStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => formatter.write_str("created"),
            Self::Running => formatter.write_str("running"),
            Self::Paused => formatter.write_str("paused"),
            Self::Exited => formatter.write_str("exited"),
            Self::Stopped => formatter.write_str("stopped"),
            Self::Removing => formatter.write_str("removing"),
            Self::Other(value) => formatter.write_str(value),
        }
    }
}

impl From<&str> for ContainerStatus {
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "created" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "exited" => Self::Exited,
            "stopped" => Self::Stopped,
            "removing" => Self::Removing,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// Brief container entry from `podman ps`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ContainerInfo {
    /// Short container ID.
    pub id: String,
    /// Container name.
    pub name: String,
    /// Raw status text from Podman.
    pub status: String,
    /// Machine-readable container state.
    pub state: String,
    /// Exit code of a stopped container.
    pub exit_code: String,
    /// Human-readable creation time.
    pub created: String,
    /// Human-readable time since the container started.
    pub running_for: String,
    /// Published port mappings from Podman.
    pub ports: String,
}

#[derive(Debug, Deserialize)]
struct InspectMount {
    #[serde(rename = "Destination")]
    destination: PathBuf,
    #[serde(rename = "Source")]
    source: PathBuf,
}

/// Type-safe description of a `podman run` invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(clippy::struct_excessive_bools, reason = "container flags map directly to Podman options")]
pub struct ContainerSpec {
    name: String,
    hostname: String,
    workdir: PathBuf,
    uid: u32,
    gid: u32,
    rootfs: PathBuf,
    rootfs_overlay: bool,
    init_shell: String,
    init_script: String,
    use_init: bool,
    read_only: bool,
    no_new_privileges: bool,
    drop_all_caps: bool,
    pids_limit: Option<u32>,
    memory_limit: Option<String>,
    memory_swap: Option<String>,
    cpus: Option<String>,
    ulimits: Vec<(String, String)>,
    inherit_proxy: bool,
    mounts: Vec<Mount>,
    tmpfs: Vec<Tmpfs>,
    env: Vec<(String, String)>,
    env_file: Option<PathBuf>,
}

/// Builds a [`ContainerSpec`] without exposing raw Podman arguments.
#[derive(Clone, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "builder mirrors the explicit Podman option set")]
pub struct ContainerSpecBuilder {
    name: Option<String>,
    hostname: Option<String>,
    workdir: Option<PathBuf>,
    uid: Option<u32>,
    gid: Option<u32>,
    rootfs: Option<PathBuf>,
    rootfs_overlay: bool,
    init_shell: Option<String>,
    init_script: Option<String>,
    use_init: bool,
    read_only: bool,
    no_new_privileges: bool,
    drop_all_caps: bool,
    pids_limit: Option<u32>,
    memory_limit: Option<String>,
    memory_swap: Option<String>,
    cpus: Option<String>,
    ulimits: Vec<(String, String)>,
    inherit_proxy: bool,
    mounts: Vec<Mount>,
    tmpfs: Vec<Tmpfs>,
    env: Vec<(String, String)>,
    env_file: Option<PathBuf>,
}

/// Readiness probe executed inside the container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TcpProbe {
    /// Shell used for the `/dev/tcp` probe.
    pub shell: String,
    /// Target host.
    pub host: String,
    /// Target port.
    pub port: u16,
    /// Maximum total wait time.
    pub timeout: Duration,
    /// Delay between probes.
    pub interval: Duration,
}

impl TcpProbe {
    fn script(&self) -> String {
        format!("exec 3<>/dev/tcp/{}/{}", shell_quote(&self.host), self.port)
    }
}

/// Flag set used when building `podman exec` arguments.
#[derive(Clone, Copy)]
enum ExecFlags {
    Plain,
    Interactive,
    Detached,
}

/// Thin wrapper around every Podman invocation used by agentbox.
#[derive(Clone, Debug)]
pub struct PodmanClient {
    binary: String,
}

impl PodmanClient {
    /// Creates a client for the configured Podman executable.
    pub fn new(binary: impl Into<String>) -> Self { Self { binary: binary.into() } }

    /// Returns true when a container with the given name exists.
    pub fn container_exists(&self, name: &str) -> Result<bool> {
        let args = os_args(&["container", "exists", name]);
        let output = self.run(&args, true)?;
        Ok(output.status.success())
    }

    /// Returns the current state of a container.
    pub fn container_status(&self, name: &str) -> Result<ContainerStatus> {
        let args = os_args(&["inspect", "-f", "{{.State.Status}}", name]);
        let output = self.run(&args, false)?;
        let status = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        snafu::ensure!(
            !status.is_empty(),
            error::ParsePodmanOutputSnafu { output: "empty container status" }
        );
        Ok(ContainerStatus::from(status.as_str()))
    }

    /// Returns the host source of a bind mount inside a container.
    pub fn container_mount_source(
        &self,
        name: &str,
        destination: &Path,
    ) -> Result<Option<PathBuf>> {
        let args = os_args(&["inspect", "--format", "{{json .Mounts}}", name]);
        let output = self.run(&args, false)?;
        let mounts =
            serde_json::from_slice::<Option<Vec<InspectMount>>>(&output.stdout).map_err(|_| {
                error::Error::ParsePodmanOutput {
                    output: String::from_utf8_lossy(&output.stdout).into_owned(),
                }
            })?;
        Ok(mounts
            .unwrap_or_default()
            .into_iter()
            .find(|mount| mount.destination == destination)
            .map(|mount| mount.source))
    }

    /// Starts a detached container from a validated specification.
    pub fn run_detached(&self, spec: &ContainerSpec) -> Result<()> {
        let args = spec.build_args();
        drop(self.run(&args, false)?);
        Ok(())
    }

    /// Starts a command inside a container without waiting for it to finish.
    pub fn exec_detached(
        &self,
        name: &str,
        command: &[&str],
        workdir: Option<&Path>,
    ) -> Result<()> {
        let args = Self::exec_args(name, command, workdir, ExecFlags::Detached);
        let output = self.run(&args, true)?;
        if output.status.success() {
            return Ok(());
        }
        Err(command_failure("podman exec", &output))
    }

    /// Replaces the current process with an interactive `podman exec`.
    pub fn exec_interactive(
        &self,
        name: &str,
        command: &[&str],
        workdir: Option<&Path>,
    ) -> Result<()> {
        let args = Self::exec_args(name, command, workdir, ExecFlags::Interactive);
        tracing::debug!("Attaching with: podman {}", os_args_to_string(&args));
        let error = Command::new(&self.binary).args(args).exec();
        Err(error::Error::ExecReplace { program: self.binary.clone(), source: error })
    }

    /// Stops and removes a container, tolerating a missing or already stopped
    /// container.
    pub fn stop_and_remove(&self, name: &str) -> Result<()> {
        let stop_args = os_args(&["stop", name]);
        let stop_output = self.run(&stop_args, true)?;
        if !stop_output.status.success() {
            tracing::warn!(
                "podman stop {name} returned {}: {}",
                stop_output.status,
                String::from_utf8_lossy(&stop_output.stderr).trim()
            );
        }

        let remove_args = os_args(&["rm", "-f", name]);
        let remove_output = self.run(&remove_args, true)?;
        if !remove_output.status.success() {
            tracing::warn!(
                "podman rm -f {name} returned {}: {}",
                remove_output.status,
                String::from_utf8_lossy(&remove_output.stderr).trim()
            );
        }

        Ok(())
    }

    /// Lists agentbox containers.
    pub fn list_containers(&self, prefix: &str) -> Result<Vec<ContainerInfo>> {
        let format = concat!(
            "{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.State}}\t",
            "{{.ExitCode}}\t{{.CreatedHuman}}\t{{.RunningFor}}\t{{.Ports}}"
        );
        let filter = format!("name=^{prefix}-");
        let args = os_args(&["ps", "-a", "--filter", &filter, "--format", format]);
        let output = self.run(&args, false)?;
        Ok(output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter_map(|line| {
                let line = String::from_utf8_lossy(line);
                let mut fields = line.split('\t');
                let id = fields.next()?;
                let name = fields.next()?;
                let status = fields.next()?;
                Some(ContainerInfo {
                    id: id.trim().to_owned(),
                    name: name.trim().to_owned(),
                    status: status.trim().to_owned(),
                    state: fields.next().unwrap_or_default().trim().to_owned(),
                    exit_code: fields.next().unwrap_or_default().trim().to_owned(),
                    created: fields.next().unwrap_or_default().trim().to_owned(),
                    running_for: fields.next().unwrap_or_default().trim().to_owned(),
                    ports: fields.next().unwrap_or_default().trim().to_owned(),
                })
            })
            .collect())
    }

    /// Returns combined container logs, tolerating a missing container.
    pub fn logs(&self, name: &str) -> Result<String> {
        let args = os_args(&["logs", name]);
        let output = self.run(&args, true)?;
        let mut logs = String::from_utf8_lossy(&output.stdout).into_owned();
        logs.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok(logs)
    }

    /// Polls a TCP port from inside the container until it is ready.
    pub fn wait_for_tcp_ready(&self, name: &str, probe: &TcpProbe) -> Result<()> {
        let started = Instant::now();
        let script = probe.script();
        let command = [probe.shell.as_str(), "-c", script.as_str()];

        loop {
            let status = self.container_status(name)?;
            if !status.is_running() {
                return Err(error::Error::ContainerExited {
                    name: name.to_owned(),
                    status: status.to_string(),
                    logs: self.logs(name).unwrap_or_default(),
                });
            }

            let probe_output =
                self.run(&Self::exec_args(name, &command, None, ExecFlags::Plain), true)?;
            if probe_output.status.success() {
                return Ok(());
            }

            let elapsed = started.elapsed();
            if elapsed >= probe.timeout {
                return Err(error::Error::StartupTimeout {
                    name: name.to_owned(),
                    seconds: probe.timeout.as_secs(),
                    logs: self.logs(name).unwrap_or_default(),
                });
            }
            thread::sleep(probe.interval.min(probe.timeout.saturating_sub(elapsed)));
        }
    }

    fn exec_args(
        name: &str,
        command: &[&str],
        workdir: Option<&Path>,
        flags: ExecFlags,
    ) -> Vec<OsString> {
        let mut args = vec![OsString::from("exec")];
        match flags {
            ExecFlags::Plain => {}
            ExecFlags::Interactive => args.push(OsString::from("-it")),
            ExecFlags::Detached => args.push(OsString::from("-d")),
        }
        if let Some(directory) = workdir {
            args.push(OsString::from("-w"));
            args.push(directory.as_os_str().to_os_string());
        }
        args.push(OsString::from(name));
        args.extend(command.iter().map(OsString::from));
        args
    }

    fn run(&self, args: &[OsString], allow_failure: bool) -> Result<Output> {
        tracing::debug!("Running podman {}", redact_args(args));
        let output = Command::new(&self.binary)
            .args(args)
            .output()
            .with_context(|_| error::SpawnCommandSnafu { program: self.binary.clone() })?;
        if !allow_failure && !output.status.success() {
            return Err(command_failure(&self.binary, &output));
        }
        Ok(output)
    }
}

impl ContainerSpec {
    /// Starts building a container specification.
    pub fn builder(name: impl Into<String>) -> ContainerSpecBuilder {
        ContainerSpecBuilder::new(name)
    }

    fn build_args(&self) -> Vec<OsString> {
        let mut args = vec![OsString::from("run"), OsString::from("-d")];
        if self.use_init {
            args.push(OsString::from("--init"));
        }
        args.extend([
            OsString::from("--name"),
            OsString::from(&self.name),
            OsString::from("--hostname"),
            OsString::from(&self.hostname),
            OsString::from("--workdir"),
            self.workdir.as_os_str().to_os_string(),
            OsString::from("--userns=keep-id"),
            OsString::from("--user"),
            OsString::from(format!("{}:{}", self.uid, self.gid)),
        ]);

        if self.read_only {
            args.push(OsString::from("--read-only"));
        }
        if self.no_new_privileges {
            args.push(OsString::from("--security-opt=no-new-privileges"));
        }
        if self.drop_all_caps {
            args.push(OsString::from("--cap-drop=ALL"));
        }
        if let Some(limit) = self.pids_limit {
            args.push(OsString::from(format!("--pids-limit={limit}")));
        }
        if let Some(limit) = &self.memory_limit {
            args.push(OsString::from(format!("--memory={limit}")));
        }
        if let Some(limit) = &self.memory_swap {
            args.push(OsString::from(format!("--memory-swap={limit}")));
        }
        if let Some(cpus) = &self.cpus {
            args.push(OsString::from(format!("--cpus={cpus}")));
        }
        for (name, value) in &self.ulimits {
            args.push(OsString::from(format!("--ulimit={name}={value}")));
        }
        args.push(OsString::from(format!("--http-proxy={}", self.inherit_proxy)));
        for mount in &self.mounts {
            args.push(OsString::from("-v"));
            args.push(mount.to_arg());
        }
        for tmpfs in &self.tmpfs {
            args.push(OsString::from("--tmpfs"));
            args.push(tmpfs.to_arg());
        }
        for (key, value) in &self.env {
            args.push(OsString::from("-e"));
            args.push(OsString::from(format!("{key}={value}")));
        }
        if let Some(path) = &self.env_file {
            args.push(OsString::from("--env-file"));
            args.push(path.as_os_str().to_os_string());
        }
        let rootfs = if self.rootfs_overlay {
            format!("{}:O", self.rootfs.display())
        } else {
            self.rootfs.display().to_string()
        };
        args.push(OsString::from("--rootfs"));
        args.push(rootfs.into());
        args.push(OsString::from(&self.init_shell));
        args.push(OsString::from("-c"));
        args.push(OsString::from(&self.init_script));
        args
    }
}

#[inline]
fn redact_args(args: &[OsString]) -> String {
    let mut redact = false;
    args.iter()
        .map(|argument| {
            let text = argument.to_string_lossy();
            if redact {
                redact = false;
                return "<redacted>".to_owned();
            }
            let owned = text.into_owned();
            if owned == "-e" {
                redact = true;
            }
            owned
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl ContainerSpecBuilder {
    /// Creates an empty builder for the named container.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            hostname: None,
            workdir: None,
            uid: None,
            gid: None,
            rootfs: None,
            rootfs_overlay: true,
            init_shell: None,
            init_script: None,
            use_init: false,
            read_only: false,
            no_new_privileges: false,
            drop_all_caps: false,
            pids_limit: None,
            memory_limit: None,
            memory_swap: None,
            cpus: None,
            ulimits: Vec::new(),
            inherit_proxy: false,
            mounts: Vec::new(),
            tmpfs: Vec::new(),
            env: Vec::new(),
            env_file: None,
        }
    }

    /// Sets the container hostname.
    pub fn hostname(mut self, hostname: impl Into<String>) -> Self {
        self.hostname = Some(hostname.into());
        self
    }

    /// Sets the container work directory.
    pub fn workdir(mut self, workdir: impl Into<PathBuf>) -> Self {
        self.workdir = Some(workdir.into());
        self
    }

    /// Sets the numeric user and group IDs.
    pub const fn user(mut self, uid: u32, gid: u32) -> Self {
        self.uid = Some(uid);
        self.gid = Some(gid);
        self
    }

    /// Sets the generated rootfs path.
    pub fn rootfs(mut self, rootfs: impl Into<PathBuf>) -> Self {
        self.rootfs = Some(rootfs.into());
        self
    }

    /// Controls whether the rootfs is mounted as an overlay.
    pub const fn rootfs_overlay(mut self, enabled: bool) -> Self {
        self.rootfs_overlay = enabled;
        self
    }

    /// Sets the shell used by the container init script.
    pub fn init_shell(mut self, shell: impl Into<String>) -> Self {
        self.init_shell = Some(shell.into());
        self
    }

    /// Sets the container init script.
    pub fn init_script(mut self, script: impl Into<String>) -> Self {
        self.init_script = Some(script.into());
        self
    }

    /// Enables Podman's host-provided init process for the fallback runtime.
    pub const fn init(mut self, enabled: bool) -> Self {
        self.use_init = enabled;
        self
    }

    /// Enables or disables a read-only root filesystem.
    pub const fn read_only(mut self, enabled: bool) -> Self {
        self.read_only = enabled;
        self
    }

    /// Enables or disables the no-new-privileges security option.
    pub const fn no_new_privileges(mut self, enabled: bool) -> Self {
        self.no_new_privileges = enabled;
        self
    }

    /// Enables or disables dropping all Linux capabilities.
    pub const fn drop_all_caps(mut self, enabled: bool) -> Self {
        self.drop_all_caps = enabled;
        self
    }

    /// Sets the container process limit.
    pub const fn pids_limit(mut self, limit: u32) -> Self {
        self.pids_limit = Some(limit);
        self
    }

    /// Sets the container memory limit.
    pub fn memory_limit(mut self, limit: impl Into<String>) -> Self {
        self.memory_limit = Some(limit.into());
        self
    }

    /// Sets the container memory-plus-swap limit.
    pub fn memory_swap(mut self, limit: impl Into<String>) -> Self {
        self.memory_swap = Some(limit.into());
        self
    }

    /// Sets the container CPU quota.
    pub fn cpus(mut self, cpus: impl Into<String>) -> Self {
        self.cpus = Some(cpus.into());
        self
    }

    /// Adds a container ulimit.
    pub fn ulimit(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.ulimits.push((name.into(), value.into()));
        self
    }

    /// Controls whether host proxy environment variables are inherited.
    pub const fn inherit_proxy(mut self, enabled: bool) -> Self {
        self.inherit_proxy = enabled;
        self
    }

    /// Sets a host env file read by Podman instead of passing values as args.
    pub fn env_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.env_file = Some(path.into());
        self
    }

    /// Adds a bind mount, replacing any previous mount with the same container
    /// path.
    pub fn mount(mut self, mount: Mount) -> Self {
        self.mounts.retain(|existing| existing.container != mount.container);
        self.mounts.push(mount);
        self
    }

    /// Adds a tmpfs declaration.
    pub fn tmpfs(mut self, tmpfs: Tmpfs) -> Self {
        self.tmpfs.push(tmpfs);
        self
    }

    /// Adds an environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Validates required fields and produces a [`ContainerSpec`].
    pub fn build(self) -> Result<ContainerSpec> {
        Ok(ContainerSpec {
            name: self.name.with_context(|| error::MissingFieldSnafu { field: "name" })?,
            hostname: self
                .hostname
                .with_context(|| error::MissingFieldSnafu { field: "hostname" })?,
            workdir: self.workdir.with_context(|| error::MissingFieldSnafu { field: "workdir" })?,
            uid: self.uid.with_context(|| error::MissingFieldSnafu { field: "uid" })?,
            gid: self.gid.with_context(|| error::MissingFieldSnafu { field: "gid" })?,
            rootfs: self.rootfs.with_context(|| error::MissingFieldSnafu { field: "rootfs" })?,
            rootfs_overlay: self.rootfs_overlay,
            init_shell: self
                .init_shell
                .with_context(|| error::MissingFieldSnafu { field: "init_shell" })?,
            init_script: self
                .init_script
                .with_context(|| error::MissingFieldSnafu { field: "init_script" })?,
            use_init: self.use_init,
            read_only: self.read_only,
            no_new_privileges: self.no_new_privileges,
            drop_all_caps: self.drop_all_caps,
            pids_limit: self.pids_limit,
            memory_limit: self.memory_limit,
            memory_swap: self.memory_swap,
            cpus: self.cpus,
            ulimits: self.ulimits,
            inherit_proxy: self.inherit_proxy,
            mounts: self.mounts,
            tmpfs: self.tmpfs,
            env: self.env,
            env_file: self.env_file,
        })
    }
}

/// Quotes a value for safe use in a POSIX shell command.
#[inline]
pub fn shell_quote(value: &str) -> String { format!("'{}'", value.replace('\'', "'\\''")) }

#[inline]
fn os_args(arguments: &[&str]) -> Vec<OsString> { arguments.iter().map(OsString::from).collect() }

#[inline]
fn os_args_to_string(arguments: &[OsString]) -> String {
    arguments.iter().map(|argument| argument.to_string_lossy()).collect::<Vec<_>>().join(" ")
}

#[inline]
fn command_failure(program: &str, output: &Output) -> error::Error {
    error::Error::CommandFailed {
        program: program.to_owned(),
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        ffi::OsString,
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use super::{
        ContainerSpec, ContainerStatus, ExecFlags, Mount, PodmanClient, TcpProbe, redact_args,
        shell_quote,
    };
    use crate::error::Error;

    fn minimal_spec() -> ContainerSpec {
        ContainerSpec::builder("agentbox-test")
            .hostname("sandbox-test")
            .workdir(PathBuf::from("/workspace"))
            .user(1000, 1000)
            .rootfs(PathBuf::from("/tmp/rootfs"))
            .init_shell("/run/ai-env/bin/bash")
            .init_script("exec sleep infinity")
            .read_only(true)
            .no_new_privileges(true)
            .drop_all_caps(true)
            .pids_limit(1024)
            .memory_limit("12g")
            .memory_swap("24g")
            .cpus("2")
            .ulimit("core", "0")
            .ulimit("nofile", "4096:8192")
            .mount(Mount::with_options(
                PathBuf::from("/tmp/host"),
                PathBuf::from("/tmp"),
                false,
                "nosuid,nodev",
            ))
            .env("OPENROUTER_API_KEY", "super-secret")
            .build()
            .expect("spec should build")
    }

    #[test]
    fn builder_generates_expected_podman_args() {
        let spec = minimal_spec();
        let args = spec.build_args();
        let strings = args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>();

        assert!(strings.contains(&"--read-only".into()));
        assert!(strings.contains(&"--security-opt=no-new-privileges".into()));
        assert!(strings.contains(&"--cap-drop=ALL".into()));
        assert!(strings.contains(&"--pids-limit=1024".into()));
        assert!(strings.contains(&"--memory=12g".into()));
        assert!(strings.contains(&"--memory-swap=24g".into()));
        assert!(strings.contains(&"--cpus=2".into()));
        assert!(strings.contains(&"--ulimit=core=0".into()));
        assert!(strings.contains(&"--ulimit=nofile=4096:8192".into()));
        assert!(strings.contains(&"--http-proxy=false".into()));
        assert!(strings.contains(&"/tmp/host:/tmp:rw,nosuid,nodev".into()));
        assert!(!strings.contains(&"-p".into()));
        assert!(strings.contains(&"--rootfs".into()));
        assert!(strings.contains(&"/tmp/rootfs:O".into()));
        assert!(strings.contains(&"OPENROUTER_API_KEY=super-secret".into()));
    }

    #[test]
    fn init_flag_is_emitted_only_when_enabled() {
        let without_init = minimal_spec().build_args();
        assert!(!without_init.iter().any(|arg| arg == "--init"));

        let spec = ContainerSpec::builder("agentbox-init")
            .hostname("sandbox-init")
            .workdir(PathBuf::from("/workspace"))
            .user(1000, 1000)
            .rootfs(PathBuf::from("/tmp/rootfs"))
            .init_shell("/run/ai-env/bin/bash")
            .init_script("exec sleep infinity")
            .init(true)
            .build()
            .expect("spec should build");
        assert!(spec.build_args().iter().any(|arg| arg.to_string_lossy() == "--init"));
    }

    #[test]
    fn exec_modes_generate_expected_flags() {
        let interactive =
            PodmanClient::exec_args("container", &["zsh", "-i"], None, ExecFlags::Interactive);
        assert_eq!(interactive[0], "exec");
        assert_eq!(interactive[1], "-it");

        let detached =
            PodmanClient::exec_args("container", &["opencode", "serve"], None, ExecFlags::Detached);
        assert_eq!(detached[1], "-d");
        assert!(detached.contains(&OsString::from("opencode")));
    }

    #[test]
    fn redacted_args_hide_environment_values() {
        let spec = minimal_spec();
        let args = spec.build_args();
        let redacted = redact_args(&args);
        assert!(redacted.contains("<redacted>"));
        assert!(!redacted.contains("super-secret"));
    }

    #[test]
    fn env_file_and_proxy_flags_are_emitted() {
        let spec = ContainerSpec::builder("agentbox-env")
            .hostname("sandbox-env")
            .workdir(PathBuf::from("/workspace"))
            .user(1000, 1000)
            .rootfs(PathBuf::from("/tmp/rootfs"))
            .init_shell("/bin/sh")
            .init_script("exec sleep infinity")
            .env_file(PathBuf::from("/tmp/secrets.env"))
            .inherit_proxy(true)
            .build()
            .expect("spec should build");
        let args = spec.build_args();
        let strings = args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>();

        assert!(strings.contains(&"--env-file".into()));
        assert!(strings.contains(&"/tmp/secrets.env".into()));
        assert!(strings.contains(&"--http-proxy=true".into()));
    }

    #[test]
    fn builder_reports_missing_fields() {
        let result = ContainerSpec::builder("incomplete").build();
        assert!(matches!(result, Err(Error::MissingField { field: "hostname" })));
    }

    #[test]
    fn status_and_shell_quote_are_stable() {
        assert_eq!(ContainerStatus::from(" running\n"), ContainerStatus::Running);
        assert!(!ContainerStatus::Exited.is_running());
        assert_eq!(shell_quote("it's"), "'it'\\''s'");

        let probe = TcpProbe {
            shell: "/bin/sh".into(),
            host: "127.0.0.1".into(),
            port: 4096,
            timeout: std::time::Duration::from_secs(15),
            interval: std::time::Duration::from_millis(250),
        };
        assert_eq!(probe.script(), "exec 3<>/dev/tcp/'127.0.0.1'/4096");
    }

    #[test]
    fn list_containers_parses_rich_fields() {
        let directory =
            env::temp_dir().join(format!("agentbox-podman-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&directory));
        fs::create_dir_all(&directory).expect("create temp dir");
        let binary = directory.join("podman");
        fs::write(
            &binary,
            "#!/bin/sh\nprintf 'abc123\\tagentbox-demo\\tUp 2 minutes\\trunning\\t0\\t2 minutes \
             ago\\t2 minutes\\t127.0.0.1:39423->4096/tcp\\n'\n",
        )
        .expect("write fake podman");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("set executable");

        let client = PodmanClient::new(binary.to_string_lossy().into_owned());
        let containers = client.list_containers("agentbox").expect("list");
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].id, "abc123");
        assert_eq!(containers[0].name, "agentbox-demo");
        assert_eq!(containers[0].status, "Up 2 minutes");
        assert_eq!(containers[0].state, "running");
        assert_eq!(containers[0].exit_code, "0");
        assert_eq!(containers[0].created, "2 minutes ago");
        assert_eq!(containers[0].running_for, "2 minutes");
        assert_eq!(containers[0].ports, "127.0.0.1:39423->4096/tcp");

        drop(fs::remove_dir_all(&directory));
    }

    #[test]
    fn container_mount_source_reads_inspect_json() {
        let directory =
            env::temp_dir().join(format!("agentbox-inspect-test-{}", std::process::id()));
        drop(fs::remove_dir_all(&directory));
        fs::create_dir_all(&directory).expect("create temp dir");
        let binary = directory.join("podman");
        fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s\\n' \
             '[{\"Destination\":\"/workspace\",\"Source\":\"/work/project\"},{\"Destination\":\"/\
             nix/store\",\"Source\":\"/nix/store\"}]'\n",
        )
        .expect("write fake podman");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("set executable");

        let client = PodmanClient::new(binary.to_string_lossy().into_owned());
        assert_eq!(
            client
                .container_mount_source("agentbox-demo", Path::new("/workspace"))
                .expect("inspect mounts"),
            Some(PathBuf::from("/work/project"))
        );
        assert_eq!(
            client
                .container_mount_source("agentbox-demo", Path::new("/missing"))
                .expect("inspect mounts"),
            None
        );

        drop(fs::remove_dir_all(&directory));
    }
}
