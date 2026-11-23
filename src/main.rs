mod app;
mod cli;
mod config;
mod consts;
mod container_plan;
mod error;
mod ocelot;
mod output;
mod podman;
mod rootfs;
mod runtime;
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
    let directive = log_directive(verbose, env::var("RUST_LOG").ok());
    let filter = EnvFilter::new(directive);
    drop(fmt().with_env_filter(filter).with_target(false).without_time().try_init());
}

fn log_directive(verbose: u8, rust_log: Option<String>) -> String {
    match rust_log {
        Some(value) if !value.trim().is_empty() => value,
        _ => match verbose {
            0 => "info".to_owned(),
            1 => "debug".to_owned(),
            _ => "trace".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::log_directive;

    #[test]
    fn rust_log_overrides_verbosity() {
        assert_eq!(log_directive(0, Some("warn".to_owned())), "warn");
    }

    #[test]
    fn blank_rust_log_falls_back_to_verbosity() {
        assert_eq!(log_directive(0, None), "info");
        assert_eq!(log_directive(1, None), "debug");
        assert_eq!(log_directive(2, Some("  ".to_owned())), "trace");
    }
}
