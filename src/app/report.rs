use snafu::ResultExt;

use crate::{
    app::App,
    error::{self, Result},
    output::OutputFormat,
    session::ContainerPlan,
    ui::{
        self,
        report::{ContainerRow, ListReport, MountSummary, ProjectReport},
    },
};

impl App {
    /// Lists agentbox-managed containers and the current project directory.
    pub fn list_containers(&self, format: OutputFormat, quiet: bool) -> Result<()> {
        let rows = self.collect_rows()?;
        if quiet {
            for row in &rows {
                println!("{}", row.info.name);
            }
            return Ok(());
        }

        let project = self.project_report(&rows);
        match format {
            OutputFormat::Table => print!("{}", ui::table::render_list(&rows, &project)),
            OutputFormat::Json => {
                let report = ListReport { containers: rows, project };
                let json =
                    serde_json::to_string_pretty(&report).context(error::SerializeOutputSnafu)?;
                println!("{json}");
            }
        }
        Ok(())
    }

    /// Prints derived information about the current project directory.
    pub fn print_status(&self, format: OutputFormat, quiet: bool) -> Result<()> {
        if quiet {
            println!("{}", self.identity.container_name);
            return Ok(());
        }

        let project = self.project_report(&self.collect_rows()?);
        match format {
            OutputFormat::Table => print!("{}", ui::table::render_status(&project)),
            OutputFormat::Json => {
                let json =
                    serde_json::to_string_pretty(&project).context(error::SerializeOutputSnafu)?;
                println!("{json}");
            }
        }
        Ok(())
    }

    fn collect_rows(&self) -> Result<Vec<ContainerRow>> {
        let prefix = &self.config.general.container_prefix;
        Ok(self
            .podman
            .list_containers(prefix)?
            .into_iter()
            .map(|info| ContainerRow { current: info.name == self.identity.container_name, info })
            .collect())
    }

    pub(super) fn project_report(&self, rows: &[ContainerRow]) -> ProjectReport {
        let (container_state, container_exit_code) =
            rows.iter().find(|row| row.current).map_or_else(
                || (String::from("absent"), String::new()),
                |row| {
                    let state =
                        if row.info.state.is_empty() { &row.info.status } else { &row.info.state };
                    (state.clone(), row.info.exit_code.clone())
                },
            );

        let ai_env = self.expand_config_path(&self.config.general.ai_env_path);
        let plan =
            ContainerPlan::new(&self.config, &self.identity, &self.home, &ai_env, self.shell());
        let mounts = plan
            .mounts()
            .iter()
            .map(|mount| MountSummary {
                host: mount.host.display().to_string(),
                container: mount.container.display().to_string(),
                read_only: mount.read_only,
            })
            .collect();
        let mut env = plan.env_keys().cloned().collect::<Vec<_>>();
        env.extend(self.config.secrets.entries.keys().cloned());

        ProjectReport {
            dir: self.identity.project_dir.display().to_string(),
            name: self.identity.name.clone(),
            hash: self.identity.hash.clone(),
            container: self.identity.container_name.clone(),
            container_state,
            container_exit_code,
            hostname: format!("{}-{}", self.config.container.hostname_prefix, self.identity.name),
            home: self.identity.home_dir.display().to_string(),
            tmp: self.identity.tmp_dir.display().to_string(),
            meta: self.identity.meta_dir.display().to_string(),
            rootfs: self.identity.rootfs_dir.display().to_string(),
            config: self.config_path.display().to_string(),
            user: self.identity.host.username.clone(),
            uid: self.identity.host.uid,
            gid: self.identity.host.gid,
            shell: self.shell().to_owned(),
            init: self.runtime().mode.as_str().to_owned(),
            mounts,
            env,
        }
    }
}
