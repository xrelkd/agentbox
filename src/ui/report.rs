use serde::Serialize;

use crate::podman::ContainerInfo;

/// A container row annotated with its relationship to the current project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ContainerRow {
    #[serde(flatten)]
    pub info: ContainerInfo,
    pub current: bool,
}

/// Read-only summary of a planned bind mount.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MountSummary {
    pub host: String,
    pub container: String,
    pub read_only: bool,
}

/// Derived facts about the current project directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectReport {
    pub dir: String,
    pub name: String,
    pub hash: String,
    pub container: String,
    pub container_state: String,
    pub container_exit_code: String,
    pub hostname: String,
    pub home: String,
    pub tmp: String,
    pub meta: String,
    pub rootfs: String,
    pub config: String,
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    pub shell: String,
    pub init: String,
    pub mounts: Vec<MountSummary>,
    pub env: Vec<String>,
}

/// Payload emitted by `agentbox list --format json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ListReport {
    pub containers: Vec<ContainerRow>,
    pub project: ProjectReport,
}
