use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

use snafu::ResultExt;

use crate::{
    config::{self, Config},
    error::{self, Result},
    workspace::ProjectIdentity,
};

/// Creates the zero-image rootfs skeleton and returns the selected container
/// shell.
pub fn prepare(config: &Config, identity: &ProjectIdentity, ai_env_path: &Path) -> Result<String> {
    let home = &identity.host.home;
    for mount in &config.mounts.state_dirs {
        let host = config::expand_path(Path::new(&mount.host), home);
        create_dir_all(&host)?;
    }

    create_dir_all(&config::expand_path(Path::new(&config.nix.host_config_dir), home))?;
    create_dir_all(&config::expand_path(Path::new(&config.nix.host_cache_dir), home))?;

    create_dir_all(&identity.tmp_dir)?;
    fs::set_permissions(&identity.tmp_dir, fs::Permissions::from_mode(0o1777))
        .with_context(|_| error::SetPermissionsSnafu { path: identity.tmp_dir.clone() })?;

    let rootfs = &identity.rootfs_dir;
    let workdir = Path::new(&config.container.workdir);
    let relative_workdir = workdir.strip_prefix("/").unwrap_or(workdir);
    for directory in ["bin", "usr/bin", "etc/ssl/certs", "run", "tmp", "var/tmp"] {
        create_dir_all(&rootfs.join(directory))?;
    }
    if !relative_workdir.as_os_str().is_empty() {
        create_dir_all(&rootfs.join(relative_workdir))?;
    }

    let env_mount = config.general.ai_env_mount_path.trim_end_matches('/');
    ensure_symlink(rootfs.join("bin/sh"), PathBuf::from(format!("{env_mount}/bin/sh")))?;
    ensure_symlink(rootfs.join("usr/bin/env"), PathBuf::from(format!("{env_mount}/bin/env")))?;
    ensure_symlink(
        rootfs.join("etc/ssl/certs/ca-bundle.crt"),
        PathBuf::from(format!("{env_mount}/etc/ssl/certs/ca-bundle.crt")),
    )?;
    ensure_symlink(
        rootfs.join("etc/protocols"),
        PathBuf::from(format!("{env_mount}/etc/protocols")),
    )?;
    ensure_symlink(
        rootfs.join("etc/services"),
        PathBuf::from(format!("{env_mount}/etc/services")),
    )?;

    let shell = select_shell(config, ai_env_path, env_mount);
    write_file(
        &rootfs.join("etc/passwd"),
        &format!(
            "root:x:0:0:root:/root:/bin/sh\n{username}:x:{uid}:{gid}:{username}:{home}:{shell}\\
             nnobody:x:65534:65534:Nobody:/:/bin/sh\n",
            username = identity.host.username,
            uid = identity.host.uid,
            gid = identity.host.gid,
            home = home.display(),
        ),
    )?;
    write_file(
        &rootfs.join("etc/group"),
        &format!(
            "root:x:0:\nusers:x:{gid}:{username}\nnogroup:x:65534:\n",
            gid = identity.host.gid,
            username = identity.host.username,
        ),
    )?;

    tracing::debug!("Prepared rootfs at {}", rootfs.display());
    Ok(shell)
}

/// Maps a container path under the AI environment mount back to its host path.
pub fn container_path_to_host(
    container_path: &str,
    env_mount: &str,
    ai_env_path: &Path,
) -> PathBuf {
    let path = Path::new(container_path);
    match path.strip_prefix(env_mount.trim_end_matches('/')) {
        Ok(relative) if !relative.as_os_str().is_empty() => ai_env_path.join(relative),
        _ => path.to_path_buf(),
    }
}

fn select_shell(config: &Config, ai_env_path: &Path, env_mount: &str) -> String {
    let configured = &config.general.shell;
    let host_configured = container_path_to_host(configured, env_mount, ai_env_path);
    if is_executable(&host_configured) {
        return configured.clone();
    }

    let fallback = format!("{env_mount}/bin/bash");
    let host_fallback = container_path_to_host(&fallback, env_mount, ai_env_path);
    if is_executable(&host_fallback) {
        tracing::warn!(
            "Configured shell {} is unavailable; falling back to {fallback}",
            host_configured.display()
        );
        return fallback;
    }

    tracing::warn!(
        "Neither {} nor {} is executable; falling back to /bin/sh",
        host_configured.display(),
        host_fallback.display()
    );
    String::from("/bin/sh")
}

#[inline]
fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[inline]
fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|_| error::CreateDirSnafu { path: path.to_path_buf() })
}

#[inline]
fn write_file(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|_| error::WriteFileSnafu { path: path.to_path_buf() })
}

fn ensure_symlink(link: PathBuf, target: PathBuf) -> Result<()> {
    if let Ok(existing) = fs::read_link(&link)
        && existing == target
    {
        return Ok(());
    }

    if link.exists() || fs::symlink_metadata(&link).is_ok() {
        fs::remove_file(&link).with_context(|_| error::RemoveFileSnafu { path: link.clone() })?;
    }

    symlink(&target, &link).with_context(|_| error::CreateSymlinkSnafu { link, target })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use super::{container_path_to_host, prepare};
    use crate::{
        config::Config,
        workspace::{HostIdentity, ProjectIdentity},
    };

    fn test_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("agentbox-rootfs-{tag}-{}", std::process::id()))
    }

    fn identity(root: &Path) -> ProjectIdentity {
        ProjectIdentity {
            project_dir: root.join("project"),
            name: "project".into(),
            hash: "deadbeef".into(),
            container_name: "agentbox-project-deadbeef".into(),
            tmp_dir: root.join("tmp"),
            meta_dir: root.join("meta"),
            rootfs_dir: root.join("meta/rootfs"),
            host: HostIdentity {
                uid: 1000,
                gid: 1000,
                username: "tester".into(),
                home: root.join("home"),
            },
        }
    }

    #[test]
    fn container_path_to_host_uses_mount_prefix() {
        let ai_env = PathBuf::from("/nix/store/abc-ai-env");
        assert_eq!(
            container_path_to_host("/run/ai-env/bin/zsh", "/run/ai-env", &ai_env),
            ai_env.join("bin/zsh")
        );
        assert_eq!(
            container_path_to_host("/bin/sh", "/run/ai-env", &ai_env),
            PathBuf::from("/bin/sh")
        );
    }

    #[test]
    fn prepare_creates_expected_skeleton_and_accounts() {
        let root = test_root("prepare");
        let _ = fs::remove_dir_all(&root);
        let project = identity(&root);
        fs::create_dir_all(root.join("home")).expect("create home");
        fs::create_dir_all(root.join("ai-env/bin")).expect("create ai env bin");
        let zsh = root.join("ai-env/bin/zsh");
        fs::write(&zsh, "#!/bin/sh\n").expect("write zsh");
        fs::set_permissions(&zsh, fs::Permissions::from_mode(0o755)).expect("chmod zsh");

        let mut config = Config::default();
        config.general.ai_env_path = root.join("ai-env").to_string_lossy().into_owned();

        let shell = prepare(&config, &project, &root.join("ai-env")).expect("prepare rootfs");
        assert_eq!(shell, "/run/ai-env/bin/zsh");

        for directory in ["bin", "usr/bin", "etc/ssl/certs", "run", "tmp", "var/tmp", "workspace"] {
            assert!(project.rootfs_dir.join(directory).is_dir(), "missing {directory}");
        }
        assert!(project.tmp_dir.join(".").is_dir());
        assert_eq!(
            fs::read_link(project.rootfs_dir.join("bin/sh")).expect("sh symlink"),
            PathBuf::from("/run/ai-env/bin/sh")
        );
        assert_eq!(
            fs::read_link(project.rootfs_dir.join("etc/ssl/certs/ca-bundle.crt"))
                .expect("cert symlink"),
            PathBuf::from("/run/ai-env/etc/ssl/certs/ca-bundle.crt")
        );

        let passwd =
            fs::read_to_string(project.rootfs_dir.join("etc/passwd")).expect("read passwd");
        assert!(passwd.contains("tester:x:1000:1000:tester:"));
        assert!(passwd.contains("/run/ai-env/bin/zsh"));
        let group = fs::read_to_string(project.rootfs_dir.join("etc/group")).expect("read group");
        assert!(group.contains("users:x:1000:tester"));
        assert!(root.join("home/.agents").is_dir());
        assert!(!root.join("agentbox-tmp").is_dir());

        fs::remove_dir_all(root).expect("cleanup");
    }
}
