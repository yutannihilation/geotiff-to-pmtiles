mod cli;
mod convert;
mod cover_tile;
mod header;
mod resample;

use std::process::ExitCode;

use clap::Parser;
use cli::{Cli, Commands};

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::DumpHeader { input } => header::dump_header(&input),
        Commands::CoverTile { input, src_crs } => {
            cover_tile::cover_tile(&input, src_crs.as_deref())
        }
        Commands::ResampleTiles {
            input,
            src_crs,
            resampling,
        } => resample::resample_tiles(&input, src_crs.as_deref(), resampling),
        Commands::Convert {
            input,
            output,
            src_crs,
            min_zoom,
            max_zoom,
            resampling,
        } => convert::convert(
            &input,
            &output,
            src_crs.as_deref(),
            min_zoom,
            max_zoom,
            resampling,
        ),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
