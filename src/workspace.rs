use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};
use snafu::{OptionExt, ResultExt};

use crate::{
    config::{self, Config},
    error::{self, Result},
};

/// Host user identity injected into the generated rootfs and container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostIdentity {
    /// Numeric host user ID.
    pub uid: u32,
    /// Numeric host group ID.
    pub gid: u32,
    /// Host user name.
    pub username: String,
    /// Host home directory.
    pub home: PathBuf,
}

impl HostIdentity {
    /// Detects uid, gid, and username via `id`, using the supplied home
    /// directory.
    pub fn detect(home: PathBuf) -> Result<Self> {
        let uid = parse_id(&id_output("-u")?, "uid")?;
        let gid = parse_id(&id_output("-g")?, "gid")?;
        let username = id_output("-un")?;
        Ok(Self { uid, gid, username, home })
    }
}

/// Stable identity and derived paths for the current project directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectIdentity {
    /// Canonicalized project directory.
    pub project_dir: PathBuf,
    /// Sanitized project directory name.
    pub name: String,
    /// First eight hex digits of the canonical path SHA-256 digest.
    pub hash: String,
    /// Generated Podman container name.
    pub container_name: String,
    /// Per-project runtime directory containing `home`, `tmp`, and `meta`.
    pub runtime_dir: PathBuf,
    /// Host temporary directory mounted at `/tmp` in the container.
    pub tmp_dir: PathBuf,
    /// Host directory mounted as the persistent container home directory.
    pub home_dir: PathBuf,
    /// Host metadata directory containing the generated rootfs.
    pub meta_dir: PathBuf,
    /// Generated rootfs directory.
    pub rootfs_dir: PathBuf,
    /// Host identity used for this project.
    pub host: HostIdentity,
}

struct RuntimePaths {
    runtime: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
    meta: PathBuf,
}

impl ProjectIdentity {
    fn from_parts(
        project_dir: PathBuf,
        name: String,
        hash: String,
        container_name: String,
        host: HostIdentity,
        paths: RuntimePaths,
    ) -> Self {
        let rootfs_dir = paths.meta.join("rootfs");
        Self {
            project_dir,
            name,
            hash,
            container_name,
            runtime_dir: paths.runtime,
            home_dir: paths.home,
            tmp_dir: paths.tmp,
            meta_dir: paths.meta,
            rootfs_dir,
            host,
        }
    }

    /// Resolves the current directory into a stable project identity.
    pub fn resolve(home: &Path, config: &Config) -> Result<Self> {
        let current = env::current_dir().context(error::CurrentDirectorySnafu)?;
        let canonical = fs::canonicalize(&current)
            .with_context(|_| error::CanonicalizeProjectSnafu { path: current.clone() })?;
        Self::resolve_from(&canonical, home, config)
    }

    /// Resolves an explicit project directory without applying start-time
    /// restrictions.
    pub fn resolve_from(project_dir: &Path, home: &Path, config: &Config) -> Result<Self> {
        let name = sanitize_project_name(
            project_dir.file_name().and_then(OsStr::to_str).unwrap_or("project"),
        );
        let hash = project_hash(project_dir);
        let base = config::expand_path(Path::new(&config.general.runtime_base_dir), home);
        let host = HostIdentity::detect(home.to_path_buf())?;
        let container_name = format!("{}-{}-{}", config.general.container_prefix, name, hash);
        let paths = runtime_directories(&base, host.uid, &name, &hash);

        Ok(Self::from_parts(project_dir.to_path_buf(), name, hash, container_name, host, paths))
    }

    /// Builds the identity of an existing container selected outside its
    /// project directory.
    pub fn from_container(
        container_name: &str,
        project_dir: PathBuf,
        host: &HostIdentity,
        home: &Path,
        config: &Config,
    ) -> Result<Self> {
        let prefix = format!("{}-", config.general.container_prefix);
        let suffix = container_name
            .strip_prefix(&prefix)
            .with_context(|| error::ParseContainerNameSnafu { name: container_name.to_owned() })?;
        let (name, hash) = suffix
            .rsplit_once('-')
            .with_context(|| error::ParseContainerNameSnafu { name: container_name.to_owned() })?;
        snafu::ensure!(
            !name.is_empty()
                && hash.len() == 8
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
            error::ParseContainerNameSnafu { name: container_name.to_owned() }
        );

        let base = config::expand_path(Path::new(&config.general.runtime_base_dir), home);
        let paths = runtime_directories(&base, host.uid, name, hash);

        Ok(Self::from_parts(
            project_dir,
            name.to_owned(),
            hash.to_owned(),
            container_name.to_owned(),
            host.clone(),
            paths,
        ))
    }
}

fn runtime_directories(base: &Path, uid: u32, name: &str, hash: &str) -> RuntimePaths {
    let runtime_dir = base.join(format!("{uid}-{name}-{hash}"));
    RuntimePaths {
        home: runtime_dir.join("home"),
        tmp: runtime_dir.join("tmp"),
        meta: runtime_dir.join("meta"),
        runtime: runtime_dir,
    }
}

/// Computes the stable eight-character project hash from a canonical absolute
/// path.
pub fn project_hash(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..4])
}

/// Keeps only `a-zA-Z0-9_-` from a directory name, with a stable fallback.
pub fn sanitize_project_name(name: &str) -> String {
    let filtered: String = name
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '_' || *character == '-'
        })
        .collect();
    if filtered.is_empty() { "project".into() } else { filtered }
}

/// Rejects configured forbidden directories and their descendants after
/// symlink resolution.
pub fn validate_forbidden_dir(project_dir: &Path, home: &Path, config: &Config) -> Result<()> {
    snafu::ensure!(
        project_dir != Path::new("/"),
        error::ForbiddenDirectorySnafu { path: project_dir.to_path_buf() }
    );
    for raw in &config.general.forbidden_dirs {
        let expanded = config::expand_path(Path::new(raw), home);
        let forbidden = fs::canonicalize(&expanded).unwrap_or(expanded);
        if forbidden == Path::new("/") {
            continue;
        }
        snafu::ensure!(
            !project_dir.starts_with(&forbidden),
            error::ForbiddenDirectorySnafu { path: project_dir.to_path_buf() }
        );
    }
    Ok(())
}

fn id_output(flag: &str) -> Result<String> {
    let output = Command::new("id")
        .arg(flag)
        .output()
        .with_context(|_| error::SpawnCommandSnafu { program: String::from("id") })?;
    if !output.status.success() {
        return Err(error::Error::CommandFailed {
            program: "id".into(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[inline]
fn parse_id(value: &str, what: &'static str) -> Result<u32> {
    value.parse().with_context(|_| error::ParseValueSnafu { value: value.to_owned(), what })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::{ProjectIdentity, project_hash, sanitize_project_name, validate_forbidden_dir};
    use crate::config::Config;

    fn test_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("agentbox-{tag}-{}", std::process::id()))
    }

    #[test]
    fn sanitize_keeps_only_allowed_characters() {
        assert_eq!(sanitize_project_name("my project@42_1"), "myproject42_1");
        assert_eq!(sanitize_project_name("!!!"), "project");
    }

    #[test]
    fn hash_is_stable_for_canonical_symlink_and_relative_paths() {
        let root = test_root("hash");
        let real = root.join("real-project");
        let link = root.join("linked-project");

        drop(fs::remove_dir_all(&root));
        fs::create_dir_all(&real).expect("create real project");
        std::os::unix::fs::symlink(&real, &link).expect("create symlink");

        let canonical = fs::canonicalize(&real).expect("canonicalize real");
        let canonical_link = fs::canonicalize(&link).expect("canonicalize link");
        let relative = fs::canonicalize(real.join(".").join("..").join("real-project"))
            .expect("canonicalize relative");

        assert_eq!(canonical, canonical_link);
        assert_eq!(project_hash(&canonical), project_hash(&canonical_link));
        assert_eq!(project_hash(&canonical), project_hash(&relative));
        assert_eq!(project_hash(&canonical).len(), 8);
        assert!(!project_hash(&canonical).contains('\n'));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn hash_uses_canonical_path_string() {
        assert_eq!(project_hash(Path::new("/work/my-project")), "df89743e");
    }

    #[test]
    fn forbidden_directory_rejects_canonical_project_path() {
        let root = test_root("forbidden");
        let project = root.join("project");
        fs::create_dir_all(&project).expect("create project");
        let canonical = fs::canonicalize(&project).expect("canonicalize project");
        let mut config = Config::default();

        config.general.forbidden_dirs = vec!["/".into()];
        assert!(validate_forbidden_dir(&canonical, Path::new("/home/test"), &config).is_ok());

        config.general.forbidden_dirs.clear();
        assert!(validate_forbidden_dir(&canonical, Path::new("/home/test"), &config).is_ok());

        config.general.forbidden_dirs = vec![canonical.to_string_lossy().into_owned()];
        assert!(validate_forbidden_dir(&canonical, Path::new("/home/test"), &config).is_err());

        config.general.forbidden_dirs = vec![root.to_string_lossy().into_owned()];
        assert!(validate_forbidden_dir(&canonical, Path::new("/home/test"), &config).is_err());

        config.general.forbidden_dirs = vec!["/".into()];
        assert!(validate_forbidden_dir(Path::new("/"), Path::new("/home/test"), &config).is_err());

        config.general.forbidden_dirs.clear();
        assert!(validate_forbidden_dir(Path::new("/"), Path::new("/home/test"), &config).is_err());

        fs::remove_dir_all(root).expect("cleanup");
    }

    fn host_identity() -> super::HostIdentity {
        super::HostIdentity {
            uid: 1000,
            gid: 1000,
            username: "tester".into(),
            home: PathBuf::from("/home/tester"),
        }
    }

    #[test]
    fn identity_fields_are_independent_of_cwd() {
        let project_dir = PathBuf::from("/work/my-project");
        let hash = project_hash(&project_dir);

        let identity = ProjectIdentity {
            project_dir: project_dir.clone(),
            name: sanitize_project_name("my-project"),
            hash: hash.clone(),
            container_name: format!("agentbox-my-project-{hash}"),
            runtime_dir: PathBuf::from("/tmp/agentbox/1000-my-project-hash"),
            tmp_dir: PathBuf::from("/tmp/agentbox/1000-my-project-hash/tmp"),
            home_dir: PathBuf::from("/tmp/agentbox/1000-my-project-hash/home"),
            meta_dir: PathBuf::from("/tmp/agentbox/1000-my-project-hash/meta"),
            rootfs_dir: PathBuf::from("/tmp/agentbox/1000-my-project-hash/meta/rootfs"),
            host: host_identity(),
        };

        assert_eq!(identity.project_dir, project_dir);
        assert_eq!(identity.hash.len(), 8);
        assert!(identity.container_name.starts_with("agentbox-my-project-"));
        assert!(identity.runtime_dir.to_string_lossy().starts_with("/tmp/agentbox/1000-"));
        assert_eq!(identity.tmp_dir, identity.runtime_dir.join("tmp"));
        assert_eq!(identity.home_dir, identity.runtime_dir.join("home"));
        assert_eq!(identity.meta_dir, identity.runtime_dir.join("meta"));
        assert_eq!(identity.rootfs_dir, identity.meta_dir.join("rootfs"));
    }

    #[test]
    fn resolve_from_groups_runtime_directories_without_forbidden_check() {
        let mut config = Config::default();
        config.general.forbidden_dirs = vec!["/tmp".into()];
        let identity = ProjectIdentity::resolve_from(
            Path::new("/tmp/work/my-project"),
            Path::new("/home/tester"),
            &config,
        )
        .expect("resolve forbidden directory for non-start commands");

        assert_eq!(identity.name, "my-project");
        assert_eq!(identity.hash, project_hash(Path::new("/tmp/work/my-project")));
        assert_eq!(
            identity.runtime_dir,
            PathBuf::from(format!(
                "/tmp/agentbox/{}-my-project-{}",
                identity.host.uid, identity.hash
            ))
        );
        assert_eq!(identity.home_dir, identity.runtime_dir.join("home"));
        assert_eq!(identity.tmp_dir, identity.runtime_dir.join("tmp"));
        assert_eq!(identity.meta_dir, identity.runtime_dir.join("meta"));
        assert_eq!(identity.rootfs_dir, identity.meta_dir.join("rootfs"));
        assert_eq!(identity.container_name, format!("agentbox-my-project-{}", identity.hash));
    }

    #[test]
    fn from_container_parses_managed_container_names() {
        let config = Config::default();
        let host = host_identity();
        let identity = ProjectIdentity::from_container(
            "agentbox-my-project-abcd1234",
            PathBuf::from("/work/my-project"),
            &host,
            Path::new("/home/tester"),
            &config,
        )
        .expect("parse container name");

        assert_eq!(identity.name, "my-project");
        assert_eq!(identity.hash, "abcd1234");
        assert_eq!(identity.container_name, "agentbox-my-project-abcd1234");
        assert_eq!(identity.runtime_dir, PathBuf::from("/tmp/agentbox/1000-my-project-abcd1234"));
        assert_eq!(identity.project_dir, PathBuf::from("/work/my-project"));

        assert!(
            ProjectIdentity::from_container(
                "other-project-abcd1234",
                PathBuf::from("/work/other"),
                &host,
                Path::new("/home/tester"),
                &config,
            )
            .is_err()
        );
        assert!(
            ProjectIdentity::from_container(
                "agentbox-my-project-bad",
                PathBuf::from("/work/other"),
                &host,
                Path::new("/home/tester"),
                &config,
            )
            .is_err()
        );
    }
}
