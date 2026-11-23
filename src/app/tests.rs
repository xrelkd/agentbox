use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use super::{App, agent_candidates};
use crate::{
    config::{AgentServe, Config},
    error::Error,
    podman::{ContainerInfo, PodmanClient},
    session::TargetRequirement,
    ui::{
        report::{ContainerRow, ListReport, MountSummary, ProjectReport},
        table::{render_list, render_status},
    },
    workspace::{HostIdentity, ProjectIdentity},
};

fn app() -> App {
    let home = PathBuf::from("/home/tester");
    let identity = ProjectIdentity {
        project_dir: PathBuf::from("/work/project"),
        name: "project".into(),
        hash: "deadbeef".into(),
        container_name: "agentbox-project-deadbeef".into(),
        runtime_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef"),
        tmp_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/tmp"),
        home_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/home"),
        meta_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/meta"),
        rootfs_dir: PathBuf::from("/tmp/agentbox/1000-project-deadbeef/meta/rootfs"),
        host: HostIdentity { uid: 1000, gid: 1000, username: "tester".into(), home: home.clone() },
    };
    let mut config = Config::default();
    config.nix.enable_daemon_mount = false;
    config.nix.mount_kernel_modules = false;
    App::new(config, identity, home, PathBuf::from("/home/tester/.config/agentbox/config.yaml"))
}

fn fake_podman(directory: &Path, body: &str) -> String {
    let binary = directory.join("podman");
    fs::write(&binary, format!("#!/bin/sh\n{body}\n")).expect("write fake podman");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("set executable");
    binary.to_string_lossy().into_owned()
}

fn use_fake_podman(app: &mut App, directory: &Path, body: &str) {
    let binary = fake_podman(directory, body);
    app.config.general.podman_binary = binary.clone();
    app.podman = PodmanClient::new(binary);
}

#[test]
fn agent_candidates_include_shell_and_are_sorted() {
    assert_eq!(agent_candidates(&Config::default()), ["claude", "gemini", "opencode", "shell"]);
}

#[test]
fn agent_choice_uses_default_without_tty() {
    assert_eq!(app().resolve_agent_name().expect("resolve agent"), "shell");
}

#[test]
fn shell_selection_is_lazy() {
    let mut app = app();
    app.config.general.shell = "/definitely/missing-shell".into();
    app.config.general.ai_env_path = "/definitely/missing-env".into();
    assert!(app.shell.get().is_none());
    assert_eq!(app.shell(), "/bin/sh");
    assert!(app.shell.get().is_some());
}

#[test]
fn secret_env_file_is_private_and_omits_values_from_args() {
    let root = env::temp_dir().join(format!("agentbox-env-file-test-{}", std::process::id()));
    drop(fs::remove_dir_all(&root));
    fs::create_dir_all(root.join("secrets")).expect("create secrets dir");
    fs::write(root.join("secrets/token"), "s3cr3t\n").expect("write secret");

    let mut app = app();
    app.config.secrets.base_dir = root.join("secrets").to_string_lossy().into_owned();
    app.config.secrets.entries.clear();
    drop(app.config.secrets.entries.insert("TEST_TOKEN".into(), "token".into()));
    app.identity.tmp_dir = root.join("tmp");
    fs::create_dir_all(&app.identity.tmp_dir).expect("create tmp dir");

    let path = app.write_secret_env_file().expect("write env file");
    let content = fs::read_to_string(&path).expect("read env file");
    assert_eq!(content, "TEST_TOKEN=s3cr3t\n");
    assert_eq!(fs::metadata(&path).expect("metadata").permissions().mode() & 0o777, 0o600);

    let report = app.project_report(&[]);
    assert!(report.env.contains(&"TEST_TOKEN".to_owned()));
    let json = serde_json::to_string(&report).expect("serialize project report");
    assert!(!json.contains("s3cr3t"));

    drop(fs::remove_dir_all(&root));
}

#[test]
fn attach_command_uses_container_client_and_loopback_url() {
    let mut app = app();
    let serve = app.config.agents["opencode"].serve.as_ref().expect("opencode serve");
    assert_eq!(
        app.build_attach_command("opencode", serve).expect("attach command"),
        ["opencode", "attach", "http://localhost:4096"]
    );

    app.config.server.serve_port = 5000;
    assert_eq!(
        app.build_attach_command("opencode", serve)
            .expect("attach command")
            .last()
            .map(String::as_str),
        Some("http://localhost:5000")
    );
    assert!(
        app.build_attach_command("opencode", &AgentServe::new("opencode serve", "  ")).is_err()
    );
}

#[test]
fn attach_requires_an_already_running_container() {
    let directory = env::temp_dir().join(format!("agentbox-attach-test-{}", std::process::id()));
    drop(fs::remove_dir_all(&directory));
    fs::create_dir_all(&directory).expect("create temp dir");
    let binary = directory.join("podman");
    fs::write(&binary, "#!/bin/sh\nexit 1\n").expect("write fake podman");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("set executable");

    let mut config = Config::default();
    config.general.podman_binary = binary.to_string_lossy().into_owned();
    let identity = app().identity;
    let app =
        App::new(config, identity, PathBuf::from("/home/tester"), PathBuf::from("/config.yaml"));

    let error = app.require_running_container().expect_err("attach must not start a container");
    assert!(matches!(error, Error::ContainerNotRunning { .. }));

    drop(fs::remove_dir_all(&directory));
}

#[test]
fn write_ocelot_config_places_yaml_in_rootfs() {
    let root = env::temp_dir().join(format!("agentbox-ocelot-config-test-{}", std::process::id()));
    drop(fs::remove_dir_all(&root));
    fs::create_dir_all(&root).expect("create temp rootfs");

    let mut app = app();
    app.identity.rootfs_dir = root.clone();
    app.write_ocelot_config(Path::new("/opt/ai-env")).expect("write ocelot config");

    let written = fs::read_to_string(root.join("etc/agentbox/ocelot.yaml"))
        .expect("read generated ocelot config");
    let value: serde_yaml::Value = serde_yaml::from_str(&written).expect("parse yaml");
    assert_eq!(value["version"].as_str(), Some("1.0"));
    assert_eq!(value["processes"]["keeper"]["program"].as_str(), Some("/run/ai-env/bin/ocelot"));

    fs::remove_dir_all(&root).expect("cleanup temp rootfs");
}

fn container_row(name: &str, current: bool) -> ContainerRow {
    ContainerRow {
        info: ContainerInfo {
            id: "abc123".into(),
            name: name.into(),
            status: "Up 2 minutes".into(),
            state: "running".into(),
            exit_code: String::new(),
            created: "2 minutes ago".into(),
            running_for: "2 minutes".into(),
            ports: String::new(),
        },
        current,
    }
}

fn project_report() -> ProjectReport {
    ProjectReport {
        dir: "/work/project".into(),
        name: "project".into(),
        hash: "deadbeef".into(),
        container: "agentbox-project-deadbeef".into(),
        container_state: "running".into(),
        container_exit_code: String::new(),
        hostname: "sandbox-project".into(),
        home: "/tmp/agentbox/1000-project-deadbeef/home".into(),
        tmp: "/tmp/agentbox/1000-project-deadbeef/tmp".into(),
        meta: "/tmp/agentbox/1000-project-deadbeef/meta".into(),
        rootfs: "/tmp/agentbox/1000-project-deadbeef/meta/rootfs".into(),
        config: "/home/tester/.config/agentbox/config.yaml".into(),
        user: "tester".into(),
        uid: 1000,
        gid: 1000,
        shell: "zsh".into(),
        init: "ocelot".into(),
        mounts: vec![
            MountSummary {
                host: "/work/project".into(),
                container: "/workspace".into(),
                read_only: false,
            },
            MountSummary {
                host: "/nix/store".into(),
                container: "/nix/store".into(),
                read_only: true,
            },
        ],
        env: vec!["HOME".into(), "PATH".into(), "TEST_TOKEN".into()],
    }
}

fn compact_output(output: &str) -> String {
    output.chars().filter(|ch| !ch.is_whitespace()).collect()
}

#[test]
fn list_table_keeps_columns_and_adds_current_project() {
    let rows = vec![
        container_row("agentbox-project-deadbeef", true),
        container_row("agentbox-other-12345678", false),
    ];
    let output = compact_output(&render_list(&rows, &project_report()));

    assert!(output.starts_with("NAME"));
    assert!(output.contains("STATUS"));
    assert!(output.contains("PORTS"));
    assert!(output.contains("agentbox-other-12345678"));
    assert!(output.contains("CURRENTPROJECT"));
    assert!(output.contains("/tmp/agentbox/1000-project-deadbeef/home"));
    assert!(output.contains("/tmp/agentbox/1000-project-deadbeef/tmp"));
    assert!(output.contains("agentbox-project-deadbeef(running)"));
}

#[test]
fn status_table_reports_identity_mounts_and_env_names() {
    let output = compact_output(&render_status(&project_report()));

    assert!(output.contains("/work/project"));
    assert!(output.contains("deadbeef"));
    assert!(output.contains("sandbox-project"));
    assert!(output.contains("MOUNTS"));
    assert!(output.contains("MODEHOSTCONTAINER"));
    assert!(output.contains("ro/nix/store/nix/store"));
    assert!(output.contains("rw/work/project/workspace"));
    assert!(output.contains("TEST_TOKEN"));
}

#[test]
fn list_json_contains_containers_and_project() {
    let report = ListReport {
        containers: vec![container_row("agentbox-project-deadbeef", true)],
        project: project_report(),
    };
    let json = serde_json::to_string(&report).expect("serialize list report");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse list report");

    assert_eq!(value["containers"][0]["name"], "agentbox-project-deadbeef");
    assert_eq!(value["containers"][0]["current"], true);
    assert_eq!(value["project"]["hash"], "deadbeef");
    assert_eq!(value["project"]["home"], "/tmp/agentbox/1000-project-deadbeef/home");
    assert_eq!(value["project"]["tmp"], "/tmp/agentbox/1000-project-deadbeef/tmp");
}

#[test]
fn resolve_target_uses_explicit_container_and_grouped_paths() {
    let directory = env::temp_dir().join(format!("agentbox-target-test-{}", std::process::id()));
    drop(fs::remove_dir_all(&directory));
    fs::create_dir_all(&directory).expect("create temp dir");
    let script = r#"case "$1" in
  ps) printf 'abc123\tagentbox-project-deadbeef\tUp 2 minutes\trunning\t0\t2 minutes ago\t2 minutes\t\n' ;;
  inspect) printf '%s\n' '[{"Destination":"/workspace","Source":"/work/other"}]' ;;
esac"#;
    let mut app = app();
    use_fake_podman(&mut app, &directory, script);

    let target = app
        .resolve_target(Some("agentbox-project-deadbeef"), TargetRequirement::Any)
        .expect("resolve target");

    assert_eq!(target.identity.container_name, "agentbox-project-deadbeef");
    assert_eq!(target.identity.project_dir, PathBuf::from("/work/other"));
    assert_eq!(target.identity.runtime_dir, PathBuf::from("/tmp/agentbox/1000-project-deadbeef"));
    assert!(target.identity.meta_dir.starts_with(&target.identity.runtime_dir));

    drop(fs::remove_dir_all(&directory));
}

#[test]
fn resolve_target_requires_choice_for_multiple_containers_without_tty() {
    let directory = env::temp_dir().join(format!("agentbox-ambiguous-test-{}", std::process::id()));
    drop(fs::remove_dir_all(&directory));
    fs::create_dir_all(&directory).expect("create temp dir");
    let script = r"printf 'a\tagentbox-one\tUp 1 minute\trunning\t0\t1 minute\t1 minute\t\n'
printf 'b\tagentbox-two\tUp 2 minutes\trunning\t0\t2 minutes\t2 minutes\t\n'";
    let mut app = app();
    use_fake_podman(&mut app, &directory, script);
    app.identity.container_name = "agentbox-absent-00000000".into();

    let error = app.resolve_target(None, TargetRequirement::Any).err().expect("ambiguous");
    assert!(matches!(error, Error::AmbiguousContainer { .. }));

    drop(fs::remove_dir_all(&directory));
}

#[test]
fn resolve_target_filters_stopped_containers_for_attach() {
    let directory = env::temp_dir().join(format!("agentbox-stopped-test-{}", std::process::id()));
    drop(fs::remove_dir_all(&directory));
    fs::create_dir_all(&directory).expect("create temp dir");
    let script = r"printf 'a\tagentbox-one\tExited\texited\t0\t1 minute\t1 minute\t\n'";
    let mut app = app();
    use_fake_podman(&mut app, &directory, script);

    let error =
        app.resolve_target(None, TargetRequirement::Running).err().expect("no running containers");
    assert!(matches!(error, Error::NoManagedContainers));

    drop(fs::remove_dir_all(&directory));
}

#[test]
fn start_rejects_forbidden_directory_before_running_podman() {
    let directory =
        env::temp_dir().join(format!("agentbox-forbidden-start-{}", std::process::id()));
    drop(fs::remove_dir_all(&directory));
    fs::create_dir_all(&directory).expect("create temp dir");
    let marker = directory.join("podman-ran");
    let script = format!("touch {}", marker.display());
    let mut app = app();
    use_fake_podman(&mut app, &directory, &script);
    app.config.general.forbidden_dirs = vec!["/tmp".into()];
    app.identity.project_dir = PathBuf::from("/tmp/work/project");

    let error = app.start(None, false).expect_err("forbidden directory");
    assert!(matches!(error, Error::ForbiddenDirectory { .. }));
    assert!(!marker.exists());

    drop(fs::remove_dir_all(&directory));
}
