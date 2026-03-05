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
        convert::ConvertOptions {
            src_crs: cli.src_crs.as_deref(),
            nodata: cli.nodata.as_deref(),
            min_zoom: cli.min_zoom,
            max_zoom: cli.max_zoom,
            resampling: cli.resampling,
            avif_quality: cli.avif_quality,
            avif_speed: cli.avif_speed,
        },
    );

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
