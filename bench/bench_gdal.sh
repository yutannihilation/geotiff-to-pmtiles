#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PMTILES_BIN="$SCRIPT_DIR/bench-data/pmtiles"
GEOTIFF_BIN="$SCRIPT_DIR/../target/release/geotiff-to-pmtiles"

# Download pmtiles CLI if not present
if [ ! -x "$PMTILES_BIN" ]; then
  echo "Downloading pmtiles CLI..."
  PMTILES_VERSION=$(gh release view --repo protomaps/go-pmtiles --json tagName -q .tagName)
  PMTILES_URL="https://github.com/protomaps/go-pmtiles/releases/download/${PMTILES_VERSION}/go-pmtiles-${PMTILES_VERSION#v}_Darwin_arm64.zip"
  tmpzip=$(mktemp)
  curl -sL "$PMTILES_URL" -o "$tmpzip"
  unzip -o -j "$tmpzip" pmtiles -d "$(dirname "$PMTILES_BIN")"
  rm -f "$tmpzip"
  chmod +x "$PMTILES_BIN"
  echo "Downloaded pmtiles ${PMTILES_VERSION}"
fi

# Build release binary upfront
cargo build --release

# Write helper scripts that hyperfine can invoke directly
GDAL_SCRIPT=$(mktemp)
cat > "$GDAL_SCRIPT" << 'HEREDOC'
#!/usr/bin/env bash
set -euo pipefail
INPUT="$1"; MIN_ZOOM="$2"; MAX_ZOOM="$3"; PMTILES_BIN="$4"
tmpdir=$(mktemp -d)
zoom_diff=$((MAX_ZOOM - MIN_ZOOM))
factors=""
f=2
for _ in $(seq 1 "$zoom_diff"); do
  factors="$factors $f"
  f=$((f * 2))
done
gdalwarp -s_srs EPSG:4326 -t_srs EPSG:3857 -r bilinear \
  -co COMPRESS=DEFLATE -co TILED=YES \
  -multi --config GDAL_NUM_THREADS ALL_CPUS \
  $INPUT "$tmpdir/merged.tif"
gdal_translate -of MBTILES -co TILE_FORMAT=PNG -co ZOOM_LEVEL_STRATEGY=UPPER \
  -co MINZOOM="$MIN_ZOOM" -co MAXZOOM="$MAX_ZOOM" "$tmpdir/merged.tif" "$tmpdir/tiles.mbtiles"
gdaladdo -r bilinear --config GDAL_NUM_THREADS ALL_CPUS \
  "$tmpdir/tiles.mbtiles" $factors
"$PMTILES_BIN" convert "$tmpdir/tiles.mbtiles" out.pmtiles
rm -rf "$tmpdir"
HEREDOC
chmod +x "$GDAL_SCRIPT"

RIO_SCRIPT=$(mktemp)
cat > "$RIO_SCRIPT" << 'HEREDOC'
#!/usr/bin/env bash
set -euo pipefail
INPUT="$1"; MIN_ZOOM="$2"; MAX_ZOOM="$3"
tmpdir=$(mktemp -d)
gdalbuildvrt -a_srs EPSG:4326 "$tmpdir/merged.vrt" $INPUT
uv run rio pmtiles "$tmpdir/merged.vrt" out.pmtiles \
  --format WEBP \
  --tile-size 256 \
  --zoom-levels "${MIN_ZOOM}..${MAX_ZOOM}"
rm -rf "$tmpdir"
HEREDOC
chmod +x "$RIO_SCRIPT"

trap 'rm -f "$GDAL_SCRIPT" "$RIO_SCRIPT"' EXIT

scenarios=(
  "small:bench-data/small/*.tif:12:16"
  "large:bench-data/large/*.tif:12:16"
  "many:bench-data/many/*.tif:12:16"
)

for scenario in "${scenarios[@]}"; do
  IFS=: read -r name input min_zoom max_zoom <<< "$scenario"
  echo ""
  echo "===== Scenario: $name ====="
  echo ""

  hyperfine \
    --warmup 0 \
    --runs 1 \
    --cleanup "rm -f out.pmtiles" \
    --export-markdown "bench-${name}.md" \
    -n "geotiff-to-pmtiles" "$GEOTIFF_BIN $input --tile-format png --src-crs EPSG:4326 --min-zoom $min_zoom --max-zoom $max_zoom" \
    -n "gdal" "bash $GDAL_SCRIPT '$input' $min_zoom $max_zoom $PMTILES_BIN" \
    -n "rio-pmtiles" "bash $RIO_SCRIPT '$input' $min_zoom $max_zoom"
done

echo ""
echo "===== Results ====="
for scenario in "${scenarios[@]}"; do
  IFS=: read -r name _ _ _ <<< "$scenario"
  echo ""
  echo "--- $name ---"
  cat "bench-${name}.md"
done
