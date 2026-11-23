use std::{
    env, fs,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    config::{self, Config},
    podman::{Mount, Tmpfs},
    workspace::ProjectIdentity,
};

/// Deterministic mounts, tmpfs declarations, and environment variables for a
/// project container.
pub struct ContainerPlan {
    mounts: Vec<Mount>,
    tmpfs: Vec<Tmpfs>,
    env: Vec<(String, String)>,
}

impl ContainerPlan {
    /// Builds the full container plan from configuration and project identity.
    pub fn new(
        config: &Config,
        identity: &ProjectIdentity,
        home: &Path,
        ai_env_path: &Path,
        shell: &str,
    ) -> Self {
        Self {
            mounts: build_mounts(config, identity, home, ai_env_path),
            tmpfs: build_tmpfs(config),
            env: build_env(config, identity, home, shell),
        }
    }

    /// Planned bind mounts, ordered from broadest to most specific.
    pub fn mounts(&self) -> &[Mount] { &self.mounts }

    /// Planned tmpfs mounts.
    pub fn tmpfs(&self) -> &[Tmpfs] { &self.tmpfs }

    /// Planned environment variables.
    pub fn env(&self) -> &[(String, String)] { &self.env }

    /// Names of the planned environment variables.
    pub fn env_keys(&self) -> impl Iterator<Item = &String> { self.env.iter().map(|(key, _)| key) }
}

fn build_mounts(
    config: &Config,
    identity: &ProjectIdentity,
    home: &Path,
    ai_env_path: &Path,
) -> Vec<Mount> {
    let mut mounts = Vec::new();
    let mut add = |host: PathBuf, container: PathBuf, read_only: bool| {
        let mount = if read_only {
            Mount::new(host, container, true)
        } else {
            Mount::with_options(host, container, false, "nosuid,nodev")
        };
        mounts.push(mount);
    };

    add(identity.home_dir.clone(), home.to_path_buf(), false);

    let store = expand_path(&config.nix.store_path, home);
    add(store.clone(), store, true);

    if config.nix.enable_daemon_mount {
        let socket_dir = expand_path(&config.nix.daemon_socket_dir, home);
        if is_socket(&socket_dir.join("socket")) {
            let container_socket = expand_path(&config.nix.daemon_socket_dir, home);
            add(socket_dir, container_socket, false);
            let profiles = expand_path(&config.nix.profiles_dir, home);
            add(profiles.clone(), profiles, true);
            let etc_nix = expand_path(&config.nix.etc_nix_dir, home);
            add(etc_nix.clone(), etc_nix, true);
        }
    }

    let mount_path = PathBuf::from(config::AI_ENV_MOUNT_PATH);
    add(ai_env_path.to_path_buf(), mount_path, true);

    if config.nix.mount_kernel_modules
        && let Some(release) = kernel_release()
    {
        let host_modules = expand_path(
            &format!("{}/{}", config.nix.kernel_modules_dir.trim_end_matches('/'), release),
            home,
        );
        if host_modules.is_dir() {
            let container_modules = PathBuf::from(format!(
                "{}/{}",
                config.nix.kernel_modules_mount_dir.trim_end_matches('/'),
                release
            ));
            add(host_modules, container_modules, true);
        }
    }

    for entry in &config.mounts {
        let host = expand_path(&entry.host, home);
        if !host.exists() {
            tracing::warn!("Skipping missing mount source {}", host.display());
            continue;
        }
        add(host, expand_path(&entry.container, home), entry.read_only);
    }

    add(identity.project_dir.clone(), expand_path(&config.container.workdir, home), false);
    let config_dir = expand_path(&config.nix.host_config_dir, home);
    let relative_config_dir = config_dir.strip_prefix(home).unwrap_or(&config_dir);
    add(identity.home_dir.join(relative_config_dir), config_dir, true);

    mounts
}

fn build_tmpfs(config: &Config) -> Vec<Tmpfs> {
    vec![
        Tmpfs::new(
            PathBuf::from("/tmp"),
            format!("rw,nosuid,nodev,mode=1777,size={}", config.container.tmp_size),
        ),
        Tmpfs::new(
            PathBuf::from("/var/tmp"),
            format!("rw,nosuid,nodev,noexec,mode=1777,size={}", config.container.var_tmp_size),
        ),
    ]
}

fn build_env(
    config: &Config,
    identity: &ProjectIdentity,
    home: &Path,
    shell: &str,
) -> Vec<(String, String)> {
    let mut variables = Vec::new();

    push_env(&mut variables, "HOME", home.display().to_string());
    push_env(&mut variables, "USER", identity.host.username.clone());
    push_env(&mut variables, "SHELL", shell.to_owned());
    push_env(
        &mut variables,
        "HISTFILE",
        expand_path(&config.environment.histfile, home).display().to_string(),
    );
    push_env(&mut variables, "PATH", expand_value(&config.environment.path, home));
    push_env(
        &mut variables,
        "SSL_CERT_FILE",
        expand_value(&config.environment.ssl_cert_file, home),
    );
    push_env(
        &mut variables,
        "NIX_SSL_CERT_FILE",
        expand_value(&config.environment.ssl_cert_file, home),
    );
    push_env(&mut variables, "CPATH", expand_value(&config.environment.cpath, home));
    push_env(&mut variables, "LIBRARY_PATH", expand_value(&config.environment.library_path, home));
    push_env(
        &mut variables,
        "PKG_CONFIG_PATH",
        expand_value(&config.environment.pkg_config_path, home),
    );
    push_env(&mut variables, "NIX_REMOTE", expand_value(&config.environment.nix_remote, home));
    push_env(&mut variables, "LANG", config.environment.lang.clone());
    push_env(&mut variables, "LC_ALL", config.environment.lang.clone());
    push_env(
        &mut variables,
        "TERM",
        env::var("TERM").unwrap_or_else(|_| config.environment.default_term.clone()),
    );

    variables.extend(
        config
            .environment
            .extra_env
            .iter()
            .map(|(key, value)| (key.clone(), expand_value(value, home))),
    );

    variables
}

#[inline]
fn expand_path(raw: &str, home: &Path) -> PathBuf { config::expand_path(Path::new(raw), home) }

#[inline]
fn expand_value(raw: &str, home: &Path) -> String { config::expand_str(raw, home) }

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
