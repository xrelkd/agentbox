use clap::Args;

use crate::{app::App, error::Result};

#[derive(Args, Clone, Debug)]
pub struct ShellArgs {}

#[derive(Args, Clone, Debug)]
pub struct TmpArgs {}

#[derive(Args, Clone, Debug)]
pub struct HomeArgs {}

impl ShellArgs {
    pub fn run(app: &App) -> Result<()> { app.attach_shell() }
}

impl TmpArgs {
    pub fn run(app: &App) { app.print_temporary_dir(); }
}

impl HomeArgs {
    pub fn run(app: &App) { app.print_home_dir(); }
}
