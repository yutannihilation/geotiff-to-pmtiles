use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::cli::TileFormat;
use crate::convert::ConvertOptions;
use crate::convert::preview::PreviewRenderer;

#[derive(Clone)]
struct AppState {
    renderer: Arc<PreviewRenderer>,
    extension: &'static str,
    content_type: &'static str,
}

pub(crate) fn serve(
    input: &[String],
    options: ConvertOptions<'_>,
    bind: &str,
    cache_mb: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let address: SocketAddr = bind
        .parse()
        .map_err(|error| format!("invalid --bind address `{bind}`: {error}"))?;
    let cache_bytes = cache_mb
        .checked_mul(1024 * 1024)
        .ok_or("--cache-mb is too large")?;
    let renderer = Arc::new(PreviewRenderer::open(input, options, cache_bytes)?);
    let (extension, content_type) = match renderer.tile_format() {
        TileFormat::Avif => ("avif", "image/avif"),
        TileFormat::Png => ("png", "image/png"),
        TileFormat::WebpLossless | TileFormat::WebpLossy => ("webp", "image/webp"),
    };
    let state = AppState {
        renderer,
        extension,
        content_type,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run(address, state))
}

async fn run(address: SocketAddr, state: AppState) -> Result<(), Box<dyn std::error::Error>> {
    let extension = state.extension;
    let app = Router::new()
        .route("/", get(index))
        .route("/tiles/{z}/{x}/{y}", get(tile))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address).await?;
    let local_address = listener.local_addr()?;

    println!("Preview tile server listening on http://{local_address}");
    println!("XYZ template: http://{local_address}/tiles/{{z}}/{{x}}/{{y}}.{extension}");
    println!("Press Ctrl+C to stop.");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn index(State(state): State<AppState>) -> String {
    format!(
        "GeoTIFF preview server\n\nTile endpoint: /tiles/{{z}}/{{x}}/{{y}}.{}\n",
        state.extension
    )
}

async fn tile(
    State(state): State<AppState>,
    Path((z, x, y_component)): Path<(u8, u32, String)>,
) -> Response {
    let y = match parse_y(&y_component, state.extension) {
        Ok(y) => y,
        Err(status) => return status.into_response(),
    };
    if z > 31 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let dimension = 1_u64 << z;
    if u64::from(x) >= dimension || u64::from(y) >= dimension {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let renderer = Arc::clone(&state.renderer);
    let rendered = tokio::task::spawn_blocking(move || {
        renderer
            .render_tile(z, x, y)
            .map_err(|error| error.to_string())
    })
    .await;

    match rendered {
        Ok(Ok(Some(bytes))) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, state.content_type)
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(error)) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("tile worker failed: {error}"),
        )
            .into_response(),
    }
}

fn parse_y(component: &str, expected_extension: &str) -> Result<u32, StatusCode> {
    let (number, extension) = match component.rsplit_once('.') {
        Some(parts) => parts,
        None => (component, expected_extension),
    };
    if extension != expected_extension {
        return Err(StatusCode::NOT_FOUND);
    }
    number.parse().map_err(|_| StatusCode::BAD_REQUEST)
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("failed to listen for Ctrl+C: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_y_with_or_without_the_configured_extension() {
        assert_eq!(parse_y("12", "png"), Ok(12));
        assert_eq!(parse_y("12.png", "png"), Ok(12));
        assert_eq!(parse_y("12.avif", "png"), Err(StatusCode::NOT_FOUND));
        assert_eq!(parse_y("nope.png", "png"), Err(StatusCode::BAD_REQUEST));
    }
}
