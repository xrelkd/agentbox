use std::collections::BTreeMap;

use serde::Serialize;
use snafu::{OptionExt, ResultExt, ensure};

use crate::{
    config::{self, Config},
    error::{self, Result},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SuperviseConfig {
    version: &'static str,
    processes: BTreeMap<String, ProcessConfig>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessConfig {
    program: String,
    arguments: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[expect(
        clippy::zero_sized_map_values,
        reason = "ocelot expects a map keyed by dependency process name"
    )]
    depends_on: Option<BTreeMap<String, DependencyConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    readiness_probe: Option<ProbeConfig>,
    restart_policy: RestartPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    shutdown_signal: Option<ShutdownSignal>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
enum RestartPolicy {
    Always { backoff: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ShutdownSignal {
    Sigterm,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeConfig {
    handler: ProbeHandler,
    period: String,
    timeout: String,
    failure_threshold: i32,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ProbeHandler {
    TcpSocket { host: String, port: u16 },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DependencyConfig {
    condition: DependencyCondition,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
enum DependencyCondition {
    Started,
}

#[expect(
    clippy::zero_sized_map_values,
    reason = "ocelot expects a map keyed by dependency process name"
)]
fn dependencies() -> BTreeMap<String, DependencyConfig> {
    BTreeMap::from([(
        "keeper".into(),
        DependencyConfig { condition: DependencyCondition::Started },
    )])
}

/// Renders the hardcoded supervise configuration for the project container.
pub fn render_config(config: &Config) -> Result<String> {
    let serve = config
        .agents
        .get("opencode")
        .and_then(|agent| agent.serve.as_ref())
        .with_context(|| error::AgentHasNoServeSnafu { name: "opencode".to_owned() })?;
    let mut arguments = config::split_command(&serve.command);
    ensure!(
        !arguments.is_empty(),
        error::InvalidConfigSnafu { message: "agents.opencode.serve.command is empty".to_owned() }
    );

    let program = arguments.remove(0);
    let host = config.server.serve_host.clone();
    let port = config.server.serve_port;
    arguments.push("--port".into());
    arguments.push(port.to_string());
    arguments.push("--hostname".into());
    arguments.push(host.clone());

    let keeper = ProcessConfig {
        program: config.general.ocelot_path.clone(),
        arguments: vec!["idle".into()],
        working_directory: None,
        depends_on: None,
        readiness_probe: None,
        restart_policy: RestartPolicy::Always { backoff: "1s".into() },
        shutdown_signal: Some(ShutdownSignal::Sigterm),
    };
    let opencode = ProcessConfig {
        program,
        arguments,
        working_directory: Some(config.container.workdir.clone()),
        depends_on: Some(dependencies()),
        readiness_probe: Some(ProbeConfig {
            handler: ProbeHandler::TcpSocket { host, port },
            period: "1s".into(),
            timeout: "1s".into(),
            failure_threshold: 15,
        }),
        restart_policy: RestartPolicy::Always { backoff: "1s".into() },
        shutdown_signal: None,
    };
    let supervise = SuperviseConfig {
        version: "1.0",
        processes: BTreeMap::from([("keeper".into(), keeper), ("opencode".into(), opencode)]),
    };

    serde_yaml::to_string(&supervise).context(error::SerializeOcelotConfigSnafu)
}
