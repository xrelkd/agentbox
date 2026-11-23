use std::io::{self, IsTerminal};

use snafu::OptionExt;

use crate::{
    error::{self, Result},
    podman::{ContainerInfo, ContainerStatus},
    ui,
};

/// Required state of a container selected outside its project directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetRequirement {
    /// Accept managed containers in any state.
    Any,
    /// Accept only running managed containers.
    Running,
}

/// Selects a managed container from the supplied candidates.
pub fn select_container(
    mut candidates: Vec<ContainerInfo>,
    explicit: Option<&str>,
    current_name: &str,
    requirement: TargetRequirement,
) -> Result<ContainerInfo> {
    candidates.retain(|info| requirement != TargetRequirement::Running || is_running(info));
    snafu::ensure!(!candidates.is_empty(), error::NoManagedContainersSnafu);

    match explicit {
        Some(name) => candidates
            .into_iter()
            .find(|info| info.name == name)
            .with_context(|| error::ContainerNotFoundSnafu { name: name.to_owned() }),
        None if candidates.iter().any(|info| info.name == current_name) => candidates
            .into_iter()
            .find(|info| info.name == current_name)
            .with_context(|| error::ContainerNotFoundSnafu { name: current_name.to_owned() }),
        None if candidates.len() == 1 => Ok(candidates.remove(0)),
        None if !io::stdin().is_terminal() => {
            let names =
                candidates.iter().map(|info| info.name.as_str()).collect::<Vec<_>>().join(", ");
            Err(error::Error::AmbiguousContainer { names })
        }
        None => pick_container(&candidates),
    }
}

fn pick_container(candidates: &[ContainerInfo]) -> Result<ContainerInfo> {
    let items = candidates
        .iter()
        .map(|info| {
            let status = if info.status.is_empty() { &info.state } else { &info.status };
            ui::fuzzy::PickerItem::new(format!("{}  [{status}]", info.name), info.name.clone())
        })
        .collect::<Vec<_>>();
    let selected = ui::fuzzy::select_value("Container: ", &items)?
        .with_context(|| error::ContainerPickerCancelledSnafu)?;
    candidates
        .iter()
        .find(|info| info.name == selected)
        .cloned()
        .with_context(|| error::ContainerNotFoundSnafu { name: selected })
}

fn is_running(info: &ContainerInfo) -> bool {
    ContainerStatus::from(info.state.as_str()).is_running()
        || info.status.to_ascii_lowercase().starts_with("up")
}
