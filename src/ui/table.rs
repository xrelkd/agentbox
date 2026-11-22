use comfy_table::{Cell, ContentArrangement, Table, presets::NOTHING};

use crate::app::{ContainerRow, ProjectReport};

const TABLE_WIDTH: u16 = 120;

pub fn render_list(rows: &[ContainerRow], project: &ProjectReport) -> String {
    let mut table = Table::new();
    let _ = table
        .load_style(NOTHING)
        .set_width(TABLE_WIDTH)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["NAME", "STATUS", "PORTS"]);

    for row in rows {
        let ports = if row.info.ports.is_empty() { "-" } else { &row.info.ports };
        let _ = table.add_row(vec![
            Cell::new(&row.info.name),
            Cell::new(&row.info.status),
            Cell::new(ports),
        ]);
    }
    for column in table.column_iter_mut() {
        let _ = column.set_padding((0, 1));
    }

    let mut output = table.to_string();
    output.push_str("\nCURRENT PROJECT\n");
    output.push_str(&render_rows(&[
        ("Dir", project.dir.clone()),
        ("Container", format!("{} ({})", project.container, state_label(project))),
        ("Home", project.home.clone()),
        ("Tmp", project.tmp.clone()),
    ]));
    output
}

pub fn render_status(project: &ProjectReport) -> String {
    let read_only = project.mounts.iter().filter(|mount| mount.read_only).count();
    let writable = project.mounts.len() - read_only;

    let mut output = String::from("CURRENT PROJECT\n");
    output.push_str(&render_rows(&[
        ("Dir", project.dir.clone()),
        ("Name", project.name.clone()),
        ("Hash", project.hash.clone()),
        ("Container", format!("{} ({})", project.container, state_label(project))),
        ("Hostname", project.hostname.clone()),
        ("Home", project.home.clone()),
        ("Tmp", project.tmp.clone()),
        ("Meta", project.meta.clone()),
        ("Rootfs", project.rootfs.clone()),
        ("Config", project.config.clone()),
        ("User", format!("{} ({}:{})", project.user, project.uid, project.gid)),
        ("Shell", project.shell.clone()),
        ("Init", project.init.clone()),
        ("Mounts", format!("{} ({} ro, {} rw)", project.mounts.len(), read_only, writable)),
        ("Env", format!("{} ({})", project.env.len(), project.env.join(", "))),
    ]));
    output
}

fn render_rows(rows: &[(&str, String)]) -> String {
    let mut table = Table::new();
    let _ = table
        .load_style(NOTHING)
        .set_width(TABLE_WIDTH)
        .set_content_arrangement(ContentArrangement::Dynamic);
    for (label, value) in rows {
        let _ = table.add_row(vec![Cell::new(format!("  {label}")), Cell::new(value)]);
    }
    for column in table.column_iter_mut() {
        let _ = column.set_padding((0, 0));
    }
    table.to_string()
}

fn state_label(project: &ProjectReport) -> String {
    if project.container_exit_code.is_empty() {
        project.container_state.clone()
    } else {
        format!("{} ({})", project.container_state, project.container_exit_code)
    }
}
