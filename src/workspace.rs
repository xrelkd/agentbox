use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};
use snafu::ResultExt;

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

impl ProjectIdentity {
    /// Resolves the current directory into a stable project identity.
    pub fn resolve(home: &Path, config: &Config) -> Result<Self> {
        let current = env::current_dir().context(error::CurrentDirectorySnafu)?;
        let canonical = fs::canonicalize(&current)
            .with_context(|_| error::CanonicalizeProjectSnafu { path: current.clone() })?;

        validate_forbidden_dir(&canonical, home, config)?;

        let name = sanitize_project_name(
            canonical.file_name().and_then(OsStr::to_str).unwrap_or("project"),
        );
        let hash = project_hash(&canonical);
        let prefix = &config.general.container_prefix;
        let base = config::expand_path(Path::new(&config.general.runtime_base_dir), home);
        let host = HostIdentity::detect(home.to_path_buf())?;
        let container_name = format!("{prefix}-{name}-{hash}");
        let tmp_dir = base.join(format!("{prefix}-tmp-{}-{name}-{hash}", host.uid));
        let home_dir = base.join(format!("{prefix}-home-{}-{name}-{hash}", host.uid));
        let meta_dir = base.join(format!("{prefix}-meta-{}-{name}-{hash}", host.uid));

        Ok(Self {
            project_dir: canonical,
            name,
            hash,
            container_name,
            tmp_dir,
            home_dir,
            rootfs_dir: meta_dir.join("rootfs"),
            meta_dir,
            host,
        })
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

/// Rejects configured forbidden directories after symlink resolution.
pub fn validate_forbidden_dir(project_dir: &Path, home: &Path, config: &Config) -> Result<()> {
    for raw in &config.general.forbidden_dirs {
        let expanded = config::expand_path(Path::new(raw), home);
        let forbidden = fs::canonicalize(&expanded).unwrap_or(expanded);
        snafu::ensure!(
            project_dir != forbidden,
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
        let config = Config::default();

        assert!(validate_forbidden_dir(&canonical, Path::new("/home/test"), &config).is_ok());

        let mut strict = config;
        strict.general.forbidden_dirs = vec![canonical.to_string_lossy().into_owned()];
        assert!(validate_forbidden_dir(&canonical, Path::new("/home/test"), &strict).is_err());

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn identity_fields_are_independent_of_cwd() {
        let host = super::HostIdentity {
            uid: 1000,
            gid: 1000,
            username: "tester".into(),
            home: PathBuf::from("/home/tester"),
        };
        let project_dir = PathBuf::from("/work/my-project");
        let hash = project_hash(&project_dir);

        let identity = ProjectIdentity {
            project_dir: project_dir.clone(),
            name: sanitize_project_name("my-project"),
            hash: hash.clone(),
            container_name: format!("agentbox-my-project-{hash}"),
            tmp_dir: PathBuf::from("/tmp/agentbox-tmp-1000-my-project-hash"),
            home_dir: PathBuf::from("/tmp/agentbox-home-1000-my-project-hash"),
            meta_dir: PathBuf::from("/tmp/agentbox-meta-1000-my-project-hash"),
            rootfs_dir: PathBuf::from("/tmp/agentbox-meta-1000-my-project-hash/rootfs"),
            host,
        };

        assert_eq!(identity.project_dir, project_dir);
        assert_eq!(identity.hash.len(), 8);
        assert!(identity.container_name.starts_with("agentbox-my-project-"));
        assert!(identity.tmp_dir.to_string_lossy().starts_with("/tmp/agentbox-tmp-"));
        assert!(identity.home_dir.to_string_lossy().starts_with("/tmp/agentbox-home-"));
        assert!(identity.rootfs_dir.starts_with(&identity.meta_dir));
    }
}
