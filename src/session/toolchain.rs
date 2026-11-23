use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use crate::config::{self, Config};

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
        "Neither {} nor {} are executable; falling back to /bin/sh",
        host_configured.display(),
        host_fallback.display()
    );
    String::from("/bin/sh")
}

#[inline]
pub fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{container_path_to_host, resolve_tool};

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
}
