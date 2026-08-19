mod cli;
mod convert;
mod resample;
mod server;

use std::process::ExitCode;

use clap::Parser;
use cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let options = convert::ConvertOptions {
        src_crs: cli.src_crs.as_deref(),
        nodata: cli.nodata.as_deref(),
        min_zoom: cli.min_zoom,
        max_zoom: cli.max_zoom,
        resampling: cli.resampling,
        tile_format: cli.tile_format,
        avif_quality: cli.avif_quality,
        avif_speed: cli.avif_speed,
        png_compression: cli.png_compression,
    };

    let result = if cli.serve {
        server::serve(&cli.input, options, &cli.bind, cli.cache_mb)
    } else {
        convert::convert(&cli.input, &cli.output, options)
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
