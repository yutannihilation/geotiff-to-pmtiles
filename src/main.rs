mod cli;
mod cover_tile;
mod header;

use std::process::ExitCode;

use clap::Parser;
use cli::{Cli, Commands};

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::DumpHeader { input } => header::dump_header(&input),
        Commands::CoverTile { input, src_crs } => cover_tile::cover_tile(&input, src_crs.as_deref()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
