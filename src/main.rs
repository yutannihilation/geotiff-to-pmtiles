mod cli;
mod convert;
mod resample;

use std::process::ExitCode;

use clap::Parser;
use cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = convert::convert(
        &cli.input,
        &cli.output,
        cli.src_crs.as_deref(),
        cli.nodeta.as_deref(),
        cli.min_zoom,
        cli.max_zoom,
        cli.resampling,
        cli.cache_mb,
    );

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
