mod app;
mod cli;
mod config;
mod consts;
mod error;
mod ocelot;
mod podman;
mod rootfs;
mod ui;
mod workspace;

use std::{env, process::ExitCode};

use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt};

use crate::cli::Cli;

#[expect(dead_code, reason = "shadow-rs emits a superset of build metadata")]
mod build {
    include!(concat!(env!("OUT_DIR"), "/shadow.rs"));
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(source) => {
            tracing::error!("{source}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(verbose: u8) {
    let directive = match env::var("RUST_LOG") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => match verbose {
            0 => "info".to_owned(),
            1 => "debug".to_owned(),
            _ => "trace".to_owned(),
        },
    };
    let filter = EnvFilter::new(directive);
    drop(fmt().with_env_filter(filter).with_target(false).without_time().try_init());
}
