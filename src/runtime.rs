use std::path::Path;

use crate::{
    config::{self, Config},
    consts::OCELOT_CONFIG_PATH,
    podman::shell_quote,
    rootfs,
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
    let (_, host) = rootfs::resolve_tool(&config.general.ocelot_path, &ai_env);
    rootfs::is_executable(&host)
}
