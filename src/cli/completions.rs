use clap::{Args, CommandFactory};
use clap_complete::{Shell, generate};

use super::Cli;
use crate::consts::CLI_PROGRAM_NAME;

#[derive(Args, Clone, Debug)]
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
