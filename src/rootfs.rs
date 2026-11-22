use std::{
    fs::{self, Permissions},
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
    create_private_dir(&identity.meta_dir)?;
    create_private_dir(&identity.home_dir)?;

    let nix_config = &config::expand_path(Path::new(&config.nix.host_config_dir), home);
    create_dir_all(nix_config)?;
    create_persistent_home_path(&identity.home_dir, &config.nix.host_config_dir, home, true)?;
    create_dir_all(&config::expand_path(Path::new(&config.nix.host_cache_dir), home))?;

    for mount in &config.mounts {
        create_persistent_home_path(&identity.home_dir, &mount.container, home, false)?;
    }

    create_private_dir(&identity.tmp_dir)?;

    let rootfs = &identity.rootfs_dir;
    let workdir = Path::new(&config.container.workdir);
    let relative_workdir = workdir.strip_prefix("/").unwrap_or(workdir);
    for directory in ["bin", "usr/bin", "etc/ssl/certs", "run", "tmp", "var/tmp"] {
        create_dir_all(&rootfs.join(directory))?;
    }
    if !relative_workdir.as_os_str().is_empty() {
        create_dir_all(&rootfs.join(relative_workdir))?;
    }
    let relative_home = home.strip_prefix("/").unwrap_or(home);
    if !relative_home.as_os_str().is_empty() {
        create_dir_all(&rootfs.join(relative_home))?;
    }

    let env_mount = config::AI_ENV_MOUNT_PATH;
    ensure_symlink(rootfs.join("bin/sh"), &PathBuf::from(format!("{env_mount}/bin/sh")))?;
    ensure_symlink(rootfs.join("usr/bin/env"), &PathBuf::from(format!("{env_mount}/bin/env")))?;
    ensure_symlink(
        rootfs.join("etc/ssl/certs/ca-bundle.crt"),
        &PathBuf::from(format!("{env_mount}/etc/ssl/certs/ca-bundle.crt")),
    )?;
    ensure_symlink(
        rootfs.join("etc/protocols"),
        &PathBuf::from(format!("{env_mount}/etc/protocols")),
    )?;
    ensure_symlink(
        rootfs.join("etc/services"),
        &PathBuf::from(format!("{env_mount}/etc/services")),
    )?;

    let shell = select_shell(config, ai_env_path);
    let passwd = format!(
        r"root:x:0:0:root:/root:/bin/sh
{username}:x:{uid}:{gid}:{username}:{home}:{shell}
nobody:x:65534:65534:Nobody:/:/bin/sh
",
        username = identity.host.username,
        uid = identity.host.uid,
        gid = identity.host.gid,
        home = home.display(),
    );
    write_file(&rootfs.join("etc/passwd"), &passwd)?;
    let group = format!(
        r"root:x:0:
users:x:{gid}:{username}
nogroup:x:65534:
",
        gid = identity.host.gid,
        username = identity.host.username,
    );
    write_file(&rootfs.join("etc/group"), &group)?;

    tracing::debug!("Prepared rootfs at {}", rootfs.display());
    Ok(shell)
}

/// Maps a container path under the AI environment mount back to its host path.
pub fn container_path_to_host(container_path: &str, ai_env_path: &Path) -> PathBuf {
    let path = Path::new(container_path);
    match path.strip_prefix(config::AI_ENV_MOUNT_PATH) {
        Ok(relative) if !relative.as_os_str().is_empty() => ai_env_path.join(relative),
        _ => path.to_path_buf(),
    }
}

/// Resolves a configured tool name or path into its container path and host
/// executable.
pub fn resolve_tool(configured: &str, ai_env_path: &Path) -> (String, PathBuf) {
    if Path::new(configured).is_absolute() {
        (configured.to_owned(), container_path_to_host(configured, ai_env_path))
    } else {
        (
            format!("{}/bin/{configured}", config::AI_ENV_MOUNT_PATH),
            ai_env_path.join("bin").join(configured),
        )
    }
}

/// Returns the configured container shell, with bash and `/bin/sh` fallbacks.
pub fn select_shell(config: &Config, ai_env_path: &Path) -> String {
    let (configured, host_configured) = resolve_tool(&config.general.shell, ai_env_path);
    if is_executable(&host_configured) {
        return configured;
    }

    let fallback = format!("{}/bin/bash", config::AI_ENV_MOUNT_PATH);
    let host_fallback = ai_env_path.join("bin/bash");
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

fn create_private_dir(path: &Path) -> Result<()> {
    create_dir_all(path)?;
    fs::set_permissions(path, Permissions::from_mode(0o700))
        .with_context(|_| error::SetPermissionsSnafu { path: path.to_path_buf() })
}

fn create_persistent_home_path(
    home_dir: &Path,
    raw_container_path: &str,
    home: &Path,
    include_target: bool,
) -> Result<()> {
    let container_path = config::expand_path(Path::new(raw_container_path), home);
    let Ok(relative) = container_path.strip_prefix(home) else {
        return Ok(());
    };
    let relative = if include_target {
        relative
    } else {
        let Some(parent) = relative.parent() else {
            return Ok(());
        };
        parent
    };
    if relative.as_os_str().is_empty() {
        return Ok(());
    }
    create_dir_all(&home_dir.join(relative))
}

#[inline]
fn write_file(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|_| error::WriteFileSnafu { path: path.to_path_buf() })
}

fn ensure_symlink(link: PathBuf, target: &Path) -> Result<()> {
    if let Ok(existing) = fs::read_link(&link)
        && existing == target
    {
        return Ok(());
    }

    let name = link
        .file_name()
        .map_or_else(|| "symlink".to_owned(), |value| value.to_string_lossy().into_owned());
    let temporary = link.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    if fs::symlink_metadata(&temporary).is_ok() {
        fs::remove_file(&temporary)
            .with_context(|_| error::RemoveFileSnafu { path: temporary.clone() })?;
    }
    symlink(target, &temporary).with_context(|_| error::CreateSymlinkSnafu {
        link: temporary.clone(),
        target: target.to_path_buf(),
    })?;
    fs::rename(&temporary, &link)
        .with_context(|_| error::RenameFileSnafu { from: temporary, to: link })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use super::{container_path_to_host, prepare, resolve_tool};
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
            home_dir: root.join("persistent-home"),
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
        assert_eq!(container_path_to_host("/run/ai-env/bin/zsh", &ai_env), ai_env.join("bin/zsh"));
        assert_eq!(container_path_to_host("/bin/sh", &ai_env), PathBuf::from("/bin/sh"));
    }

    #[test]
    fn resolve_tool_maps_bare_names_into_ai_env_bin() {
        let ai_env = PathBuf::from("/nix/store/abc-ai-env");
        assert_eq!(
            resolve_tool("zsh", &ai_env),
            (String::from("/run/ai-env/bin/zsh"), ai_env.join("bin/zsh"))
        );
        assert_eq!(
            resolve_tool("/run/ai-env/bin/ocelot", &ai_env),
            (String::from("/run/ai-env/bin/ocelot"), ai_env.join("bin/ocelot"))
        );
    }

    #[test]
    fn prepare_creates_expected_skeleton_and_accounts() {
        let root = test_root("prepare");
        drop(fs::remove_dir_all(&root));
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
        assert!(project.home_dir.join(".claude").is_dir());
        assert!(project.home_dir.join(".config/nix").is_dir());
        let relative_home = project.host.home.strip_prefix("/").unwrap_or(&project.host.home);
        assert!(project.rootfs_dir.join(relative_home).is_dir());
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
        assert!(passwd.lines().any(|line| line == "nobody:x:65534:65534:Nobody:/:/bin/sh"));
        let group = fs::read_to_string(project.rootfs_dir.join("etc/group")).expect("read group");
        assert!(group.contains("users:x:1000:tester"));
        assert!(group.lines().any(|line| line == "nogroup:x:65534:"));
        assert!(!root.join("home/.agents").is_dir());
        assert!(!root.join("agentbox-tmp").is_dir());

        fs::remove_dir_all(root).expect("cleanup");
    }
}
