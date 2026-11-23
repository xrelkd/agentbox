use std::path::Path;

use crate::{
    config::{self, Config},
    consts::OCELOT_CONFIG_PATH,
    session::toolchain::{is_executable, resolve_tool},
    shell::shell_quote,
};

/// Init strategy selected for a container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitMode {
    /// `ocelot supervise` is PID 1 and owns the long-lived services.
    Ocelot,
    /// Podman's host init is PID 1 and services start on demand.
    Fallback,
}

impl InitMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ocelot => "ocelot",
            Self::Fallback => "fallback",
        }
    }
}

/// Runtime facts returned when a container is started.
#[derive(Clone, Debug)]
pub struct Runtime {
    pub mode: InitMode,
    pub shell: String,
    pub probe_shell: String,
}

impl Runtime {
    /// Detects the init strategy and resolves the shells used inside the
    /// container.
    pub fn detect(config: &Config, home: &Path, shell: &str) -> Self {
        let mode =
            if ocelot_available(config, home) { InitMode::Ocelot } else { InitMode::Fallback };
        Self { mode, shell: shell.to_owned(), probe_shell: Self::init_shell() }
    }

    /// Returns the shell available from the AI environment mount.
    pub fn init_shell() -> String { format!("{}/bin/bash", config::AI_ENV_MOUNT_PATH) }
}

/// Renders the PID 1 command for the selected init strategy.
pub fn init_script_for(shell: &str, mode: InitMode, ocelot_path: &str) -> String {
    match mode {
        InitMode::Ocelot => {
            let file = shell_quote(OCELOT_CONFIG_PATH);
            let ocelot = shell_quote(ocelot_path);
            format!("exec {ocelot} supervise run --file {file}")
        }
        InitMode::Fallback => {
            let shell = shell_quote(shell);
            format!("exec {shell} -c {}", shell_quote("exec sleep infinity"))
        }
    }
}

fn ocelot_available(config: &Config, home: &Path) -> bool {
    let ai_env = config::expand_path(Path::new(&config.general.ai_env_path), home);
    let (_, host) = resolve_tool(&config.general.ocelot_path, &ai_env);
    is_executable(&host)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{InitMode, Runtime, init_script_for};
    use crate::{config::Config, session::resolve_tool};

    #[test]
    fn ocelot_init_script_only_execs_supervise() {
        let config = Config::default();
        let (ocelot, _) = resolve_tool(&config.general.ocelot_path, Path::new("/opt/ai-env"));
        let script = init_script_for("/run/ai-env/bin/bash", InitMode::Ocelot, &ocelot);

        assert_eq!(
            script,
            "exec '/run/ai-env/bin/ocelot' supervise run --file '/etc/agentbox/ocelot.yaml'"
        );
        assert!(!script.contains("mkdir"));
        assert!(!script.contains("cat >"));
    }

    #[test]
    fn fallback_init_script_uses_keepalive() {
        let script =
            init_script_for("/run/ai-env/bin/bash", InitMode::Fallback, "/run/ai-env/bin/ocelot");

        assert_eq!(script, "exec '/run/ai-env/bin/bash' -c 'exec sleep infinity'");
    }

    #[test]
    fn probe_uses_bash_regardless_of_configured_shell() {
        let config = Config::default();
        let home = PathBuf::from("/home/tester");
        let runtime = Runtime::detect(&config, &home, "/bin/sh");
        assert_eq!(runtime.probe_shell, "/run/ai-env/bin/bash");
    }
}
