use clap::Args;

use crate::build;

#[derive(Args, Clone, Debug)]
pub struct VersionArgs {
    /// Print only client build metadata.
    #[arg(long)]
    pub client: bool,
}

impl VersionArgs {
    pub fn client_summary() -> String {
        format!(
            "agentbox {}\ncommit: {}\nbuilt: {}\nrustc: {}",
            build::PKG_VERSION,
            metadata(build::SHORT_COMMIT),
            metadata(build::BUILD_TIME),
            metadata(build::RUST_VERSION)
        )
    }

    pub fn run(&self) {
        if self.client {
            println!("{}", Self::client_summary());
        } else {
            println!("{}", build::CLAP_LONG_VERSION);
        }
    }
}

fn metadata(value: &str) -> &str { if value.trim().is_empty() { "unknown" } else { value } }

#[cfg(test)]
mod tests {
    use super::VersionArgs;

    #[test]
    fn client_summary_includes_build_metadata() {
        let summary = VersionArgs::client_summary();
        assert!(summary.starts_with("agentbox "));
        assert!(summary.contains("\ncommit: "));
        assert!(summary.contains("\nbuilt: "));
        assert!(summary.contains("\nrustc: "));
        assert!(summary.lines().all(|line| !line.ends_with(": ")));
    }
}
