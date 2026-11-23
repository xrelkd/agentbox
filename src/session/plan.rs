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
#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::{Path, PathBuf},
    };

    use super::ContainerPlan;
    use crate::{
        config::{Config, MountEntry},
        workspace::{HostIdentity, ProjectIdentity},
    };

    fn identity() -> ProjectIdentity {
        let home = PathBuf::from("/home/tester");
        ProjectIdentity {
            project_dir: PathBuf::from("/work/project"),
            name: "project".into(),
            hash: "deadbeef".into(),
            container_name: "agentbox-project-deadbeef".into(),
            runtime_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef"),
            tmp_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/tmp"),
            home_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/home"),
            meta_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/meta"),
            rootfs_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/meta/rootfs"),
            host: HostIdentity { uid: 1000, gid: 1000, username: "tester".into(), home },
        }
    }

    fn plan(config: &Config, home: &Path, ai_env_path: &Path) -> ContainerPlan {
        ContainerPlan::new(config, &identity(), home, ai_env_path, "zsh")
    }

    #[test]
    fn home_mount_precedes_nested_state_mounts() {
        let home = PathBuf::from("/home/tester");
        let temp = env::temp_dir();
        let config = Config {
            mounts: vec![MountEntry::new(temp.to_string_lossy(), "/home/tester/.agents", true)],
            ..Default::default()
        };
        let plan = plan(&config, &home, Path::new("/nix/store/ai-env"));
        let mounts = plan.mounts();
        let home_index =
            mounts.iter().position(|mount| mount.container == home).expect("home mount");
        let agents_index = mounts
            .iter()
            .position(|mount| mount.container == home.join(".agents"))
            .expect("state mount");

        assert_eq!(mounts[home_index].host, identity().home_dir);
        assert!(home_index < agents_index);
    }

    #[test]
    fn missing_mount_source_is_skipped() {
        let home = PathBuf::from("/home/tester");
        let missing =
            env::temp_dir().join(format!("agentbox-missing-mount-{}", std::process::id()));
        drop(fs::remove_dir_all(&missing));
        let temp = env::temp_dir();
        let config = Config {
            mounts: vec![
                MountEntry::new(missing.to_string_lossy(), "/home/tester/missing", false),
                MountEntry::new(temp.to_string_lossy(), "/home/tester/present", true),
            ],
            ..Default::default()
        };

        let plan = plan(&config, &home, Path::new("/nix/store/ai-env"));
        let mounts = plan.mounts();
        assert!(!mounts.iter().any(|mount| mount.container == home.join("missing")));
        assert!(mounts.iter().any(|mount| mount.container == home.join("present")));
    }

    #[test]
    fn temporary_directories_are_sized_tmpfs_mounts() {
        let plan =
            plan(&Config::default(), Path::new("/home/tester"), Path::new("/nix/store/ai-env"));
        let tmpfs = plan.tmpfs();
        assert_eq!(tmpfs.len(), 2);
        assert_eq!(tmpfs[0].target, PathBuf::from("/tmp"));
        assert_eq!(tmpfs[0].options, "rw,nosuid,nodev,mode=1777,size=2g");
        assert_eq!(tmpfs[1].target, PathBuf::from("/var/tmp"));
        assert_eq!(tmpfs[1].options, "rw,nosuid,nodev,noexec,mode=1777,size=2g");
    }

    #[test]
    fn configuration_directories_are_mounted_read_only() {
        let home = PathBuf::from("/home/tester");
        let temp = env::temp_dir();
        let config = Config {
            mounts: vec![MountEntry::new(temp.to_string_lossy(), "/home/tester/.claude", true)],
            ..Default::default()
        };
        let plan = plan(&config, &home, Path::new("/nix/store/ai-env"));
        let mounts = plan.mounts();
        let config_mount = mounts
            .iter()
            .find(|mount| mount.container == Path::new("/home/tester/.claude"))
            .expect("claude config mount");
        assert!(config_mount.read_only);

        let nix = mounts
            .iter()
            .find(|mount| mount.container == Path::new("/home/tester/.config/nix"))
            .expect("nix config mount");
        assert!(nix.read_only);
        assert!(nix.host.starts_with(&identity().home_dir));
    }
}
