use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Pt {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum GeoTransform {
    TiePointAndPixelScale {
        raster_x: f64,
        raster_y: f64,
        model_x: f64,
        model_y: f64,
        scale_x: f64,
        scale_y: f64,
    },
    Affine {
        t0: f64,
        t1: f64,
        t2: f64,
        t3: f64,
        t4: f64,
        t5: f64,
    },
}

impl GeoTransform {
    pub(crate) fn apply(self, p: Pt) -> Pt {
        match self {
            GeoTransform::TiePointAndPixelScale {
                raster_x,
                raster_y,
                model_x,
                model_y,
                scale_x,
                scale_y,
            } => Pt {
                x: (p.x - raster_x) * scale_x + model_x,
                y: (p.y - raster_y) * -scale_y + model_y,
            },
            GeoTransform::Affine {
                t0,
                t1,
                t2,
                t3,
                t4,
                t5,
            } => Pt {
                x: p.x * t0 + p.y * t1 + t2,
                y: p.x * t3 + p.y * t4 + t5,
            },
        }
    }

    pub(crate) fn invert(self) -> Result<Self, Box<dyn std::error::Error>> {
        match self {
            GeoTransform::TiePointAndPixelScale {
                raster_x,
                raster_y,
                model_x,
                model_y,
                scale_x,
                scale_y,
            } => {
                if scale_x == 0.0 || scale_y == 0.0 {
                    return Err("invalid tie-point transform: zero scale".into());
                }
                Ok(GeoTransform::TiePointAndPixelScale {
                    raster_x: model_x,
                    raster_y: model_y,
                    model_x: raster_x,
                    model_y: raster_y,
                    scale_x: 1.0 / scale_x,
                    scale_y: 1.0 / scale_y,
                })
            }
            GeoTransform::Affine {
                t0,
                t1,
                t2,
                t3,
                t4,
                t5,
            } => {
                let det = t0 * t4 - t1 * t3;
                if det.abs() < 1e-15 {
                    return Err("affine transform is not invertible".into());
                }
                Ok(GeoTransform::Affine {
                    t0: t4 / det,
                    t1: -t1 / det,
                    t2: (t1 * t5 - t2 * t4) / det,
                    t3: -t3 / det,
                    t4: t0 / det,
                    t5: (-t0 * t5 + t2 * t3) / det,
                })
            }
        }
    }
}

pub(crate) struct Raster {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride: usize,
    pub(crate) data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum NoDataSpec {
    Gray(u8),
    Rgb(u8, u8, u8),
}

impl NoDataSpec {
    pub(crate) fn is_nodata(self, rgba: [u8; 4]) -> bool {
        match self {
            NoDataSpec::Gray(v) => rgba[0] == v && rgba[1] == v && rgba[2] == v,
            NoDataSpec::Rgb(r, g, b) => rgba[0] == r && rgba[1] == g && rgba[2] == b,
        }
    }
}

pub(crate) struct Georef {
    pub(crate) source_crs: String,
    pub(crate) forward: GeoTransform,
    pub(crate) raster_offset: f64,
}

pub(crate) struct SourceMetadata {
    pub(crate) path: PathBuf,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) georef: Georef,
}

pub(crate) fn zoom_for_tile_size(required_size: f64) -> u8 {
    const MAX_ZOOM: u8 = 24;
    const ORIGIN_SHIFT: f64 = 20_037_508.342_789_244;
    let world_size = 2.0 * ORIGIN_SHIFT;

    // Search from fine -> coarse and return first zoom whose tile edge can cover required size.
    for z in (0..=MAX_ZOOM).rev() {
        let tile_size = world_size / (1_u32 << z) as f64;
        if tile_size >= required_size {
            return z;
        }
    }

    0
}

pub(crate) fn webmerc_to_tile(x: f64, y: f64, z: u8) -> (u32, u32) {
    const ORIGIN_SHIFT: f64 = 20_037_508.342_789_244;
    let n = (1_u32 << z) as f64;

    // Clamp to avoid mapping exact world boundary to out-of-range tile index (n).
    let x_norm = ((x + ORIGIN_SHIFT) / (2.0 * ORIGIN_SHIFT)).clamp(0.0, 1.0 - f64::EPSILON);
    let y_norm = ((ORIGIN_SHIFT - y) / (2.0 * ORIGIN_SHIFT)).clamp(0.0, 1.0 - f64::EPSILON);

    let xtile = (x_norm * n).floor() as u32;
    let ytile = (y_norm * n).floor() as u32;

    (xtile, ytile)
}

pub(crate) struct TileBounds {
    pub(crate) ul: Pt,
    pub(crate) ur: Pt,
    pub(crate) lr: Pt,
    pub(crate) ll: Pt,
}

pub(crate) fn tile_bounds_webmerc(z: u8, x: u32, y: u32) -> TileBounds {
    const ORIGIN_SHIFT: f64 = 20_037_508.342_789_244;
    let n = (1_u32 << z) as f64;
    let tile_size = (2.0 * ORIGIN_SHIFT) / n;

    let min_x = -ORIGIN_SHIFT + x as f64 * tile_size;
    let max_x = min_x + tile_size;
    let max_y = ORIGIN_SHIFT - y as f64 * tile_size;
    let min_y = max_y - tile_size;

    TileBounds {
        ul: Pt { x: min_x, y: max_y },
        ur: Pt { x: max_x, y: max_y },
        lr: Pt { x: max_x, y: min_y },
        ll: Pt { x: min_x, y: min_y },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn affine_transform_apply_and_invert_roundtrip() {
        let t = GeoTransform::Affine {
            t0: 2.0,
            t1: 1.0,
            t2: 10.0,
            t3: -1.0,
            t4: 3.0,
            t5: 20.0,
        };
        let p = Pt { x: 5.0, y: -2.0 };
        let mapped = t.apply(p);
        let inv = t.invert().expect("invertible affine");
        let roundtrip = inv.apply(mapped);
        assert_close(roundtrip.x, p.x);
        assert_close(roundtrip.y, p.y);
    }

    #[test]
    fn affine_transform_non_invertible_errors() {
        let singular = GeoTransform::Affine {
            t0: 1.0,
            t1: 2.0,
            t2: 0.0,
            t3: 2.0,
            t4: 4.0,
            t5: 0.0,
        };
        assert!(singular.invert().is_err());
    }

    #[test]
    fn tiepoint_transform_apply_and_invert_roundtrip() {
        let t = GeoTransform::TiePointAndPixelScale {
            raster_x: 100.0,
            raster_y: 200.0,
            model_x: 1000.0,
            model_y: 2000.0,
            scale_x: 2.0,
            scale_y: 4.0,
        };
        let p = Pt { x: 110.0, y: 210.0 };
        let mapped = t.apply(p);
        let inv = t.invert().expect("invertible tiepoint");
        let roundtrip = inv.apply(mapped);
        assert_close(roundtrip.x, p.x);
        assert_close(roundtrip.y, p.y);
    }

    #[test]
    fn tiepoint_transform_zero_scale_errors() {
        let t = GeoTransform::TiePointAndPixelScale {
            raster_x: 0.0,
            raster_y: 0.0,
            model_x: 0.0,
            model_y: 0.0,
            scale_x: 0.0,
            scale_y: 1.0,
        };
        assert!(t.invert().is_err());
    }

    #[test]
    fn nodata_detection_matches_gray_and_rgb() {
        assert!(NoDataSpec::Gray(7).is_nodata([7, 7, 7, 255]));
        assert!(!NoDataSpec::Gray(7).is_nodata([7, 8, 7, 255]));
        assert!(NoDataSpec::Rgb(1, 2, 3).is_nodata([1, 2, 3, 0]));
        assert!(!NoDataSpec::Rgb(1, 2, 3).is_nodata([1, 2, 4, 0]));
    }

    #[test]
    fn zoom_for_tile_size_respects_bounds() {
        const ORIGIN_SHIFT: f64 = 20_037_508.342_789_244;
        let world_size = 2.0 * ORIGIN_SHIFT;
        assert_eq!(zoom_for_tile_size(world_size), 0);

        let tile_size_z10 = world_size / (1_u32 << 10) as f64;
        let tile_size_z11 = world_size / (1_u32 << 11) as f64;
        assert_eq!(zoom_for_tile_size(tile_size_z10), 10);
        assert_eq!(zoom_for_tile_size(tile_size_z11 + 1.0), 10);
    }

    #[test]
    fn webmerc_to_tile_clamps_world_edges() {
        const ORIGIN_SHIFT: f64 = 20_037_508.342_789_244;
        let (x0, y0) = webmerc_to_tile(-ORIGIN_SHIFT, ORIGIN_SHIFT, 2);
        assert_eq!((x0, y0), (0, 0));

        let (x1, y1) = webmerc_to_tile(ORIGIN_SHIFT, -ORIGIN_SHIFT, 2);
        assert_eq!((x1, y1), (3, 3));
    }

    #[test]
    fn tile_bounds_for_world_tile_are_correct() {
        const ORIGIN_SHIFT: f64 = 20_037_508.342_789_244;
        let b = tile_bounds_webmerc(0, 0, 0);
        assert_close(b.ul.x, -ORIGIN_SHIFT);
        assert_close(b.ul.y, ORIGIN_SHIFT);
        assert_close(b.lr.x, ORIGIN_SHIFT);
        assert_close(b.lr.y, -ORIGIN_SHIFT);
    }
}
