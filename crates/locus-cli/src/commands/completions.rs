//! `locus completions` — generate shell completions (U-014).

use std::io;

use anyhow::Result;
use clap::CommandFactory;
use clap::Parser;
use clap_complete::Shell;

use crate::Cli;

/// Generate shell completion scripts
#[derive(Parser, Debug)]
pub struct CompletionsCmd {
    /// Shell to generate completions for
    #[arg(value_enum)]
    pub shell: Shell,
}

impl CompletionsCmd {
    pub fn run(self) -> Result<()> {
        let mut cmd = Cli::command();
        let name = cmd.get_name().to_string();
        clap_complete::generate(self.shell, &mut cmd, name, &mut io::stdout());
        Ok(())
    }
}
