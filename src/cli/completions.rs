use clap::{Args, CommandFactory};
use clap_complete::{Shell, generate};

use crate::{cli::Cli, consts::CLI_PROGRAM_NAME};

#[derive(Clone, Debug, Args)]
pub struct CompletionsArgs {
    /// Shell to generate completions for.
    pub shell: Shell,
}

impl CompletionsArgs {
    pub fn run(&self) {
        let mut command = Cli::command();
        generate(self.shell, &mut command, CLI_PROGRAM_NAME, &mut std::io::stdout());
    }
}
