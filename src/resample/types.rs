use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Pt {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

pub(crate) fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    pub(crate) gdal_nodata: Option<String>,
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

/// Piecewise-bilinear approximation of the mapping from output tile UV space
/// into source raster pixel space.
///
/// UV space is the unit square over one output tile: `(0, 0)` is the tile's
/// upper-left corner, `(1, 1)` its lower-right corner. The tile is split into
/// `cells × cells` equal cells; every cell corner carries the exact source
/// pixel position, and positions inside a cell are bilinearly interpolated.
///
/// The grid is built by [`TileMapping::refine`], which doubles `cells` until
/// the interpolation error measured at cell midpoints drops below a threshold
/// expressed in output pixels, so the accuracy is bounded regardless of zoom
/// level, latitude, or source CRS.
#[derive(Debug, Clone)]
pub(crate) struct TileMapping {
    cells: usize,
    /// `(cells + 1)²` control points, row-major: index `j * (cells + 1) + i`
    /// holds the source position of UV `(i / cells, j / cells)`.
    grid: Vec<Pt>,
}

impl TileMapping {
    /// Upper bound for `cells`; refinement stops here even if the error target
    /// is not met (this only happens for very coarse tiles near the poles).
    pub(crate) const MAX_CELLS: usize = 64;

    /// Build a mapping by adaptive refinement.
    ///
    /// * `exact(u, v)` evaluates the exact UV → source pixel mapping.
    /// * `max_error_out_px` is the tolerated interpolation error in output
    ///   pixels; it is converted to source pixels with the tile's smallest
    ///   edge scale, which is conservative for anisotropic mappings.
    /// * `tile_size_px` is the output tile edge length in pixels.
    ///
    /// Returns the coarsest power-of-two grid whose midpoint error is within
    /// the threshold, or a `MAX_CELLS` grid if the threshold is never met.
    /// Errors from `exact` are propagated; a degenerate (zero-area) corner
    /// mapping is an error rather than being silently accepted.
    pub(crate) fn refine<E>(
        max_error_out_px: f64,
        tile_size_px: usize,
        mut exact: E,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        E: FnMut(f64, f64) -> Result<Pt, Box<dyn std::error::Error>>,
    {
        let ul = exact(0.0, 0.0)?;
        let ur = exact(1.0, 0.0)?;
        let ll = exact(0.0, 1.0)?;
        let lr = exact(1.0, 1.0)?;

        // Source pixels per output pixel, taken from the shortest tile edge so the
        // threshold is never looser than the visible error in any direction.
        let edge = |a: Pt, b: Pt| (b.x - a.x).hypot(b.y - a.y);
        let scale = edge(ul, ur)
            .min(edge(ll, lr))
            .min(edge(ul, ll))
            .min(edge(ur, lr))
            / tile_size_px as f64;
        if !scale.is_finite() || scale <= 0.0 {
            return Err("degenerate tile mapping: tile collapses in source raster space".into());
        }
        let threshold_src_px = max_error_out_px * scale;

        let mut mapping = TileMapping {
            cells: 1,
            grid: vec![ul, ur, ll, lr],
        };

        while mapping.cells < Self::MAX_CELLS {
            let fine = mapping.cells * 2;
            let stride = fine + 1;
            let mut fine_grid = vec![Pt { x: 0.0, y: 0.0 }; stride * stride];
            let mut max_err = 0.0_f64;
            for j in 0..=fine {
                for i in 0..=fine {
                    let idx = j * stride + i;
                    if i % 2 == 0 && j % 2 == 0 {
                        fine_grid[idx] = mapping.point(i / 2, j / 2);
                        continue;
                    }
                    let u = i as f64 / fine as f64;
                    let v = j as f64 / fine as f64;
                    let p = exact(u, v)?;
                    let q = mapping.eval(u, v);
                    max_err = max_err.max((p.x - q.x).hypot(p.y - q.y));
                    fine_grid[idx] = p;
                }
            }
            if max_err <= threshold_src_px {
                break;
            }
            mapping = TileMapping {
                cells: fine,
                grid: fine_grid,
            };
            if mapping.cells == Self::MAX_CELLS {
                eprintln!(
                    "warning: reprojection grid reached {} cells with error {max_err:.3} source px (target {threshold_src_px:.3}); using it anyway",
                    Self::MAX_CELLS
                );
            }
        }

        Ok(mapping)
    }

    pub(crate) fn cells(&self) -> usize {
        self.cells
    }

    /// Control point at grid indices `(i, j)`, i.e. UV `(i / cells, j / cells)`.
    pub(crate) fn point(&self, i: usize, j: usize) -> Pt {
        self.grid[j * (self.cells + 1) + i]
    }

    /// Index of the cell column/row containing `t ∈ [0, 1]`, clamped so `t = 1`
    /// belongs to the last cell.
    pub(crate) fn cell_index(&self, t: f64) -> usize {
        ((t * self.cells as f64).floor().max(0.0) as usize).min(self.cells - 1)
    }

    /// Left and right cell-boundary positions of cell column `cu` at UV row `v`,
    /// i.e. the vertical interpolation along the two cell edges bounding the row.
    pub(crate) fn row_segment(&self, v: f64, cu: usize) -> (Pt, Pt) {
        let cv = self.cell_index(v);
        let lv = v * self.cells as f64 - cv as f64;
        let left = lerp(self.point(cu, cv), self.point(cu, cv + 1), lv);
        let right = lerp(self.point(cu + 1, cv), self.point(cu + 1, cv + 1), lv);
        (left, right)
    }

    /// Interpolated source position of UV `(u, v)`.
    pub(crate) fn eval(&self, u: f64, v: f64) -> Pt {
        let cu = self.cell_index(u);
        let lu = u * self.cells as f64 - cu as f64;
        let (left, right) = self.row_segment(v, cu);
        lerp(left, right, lu)
    }

    /// Axis-aligned bounding box `(min_x, min_y, max_x, max_y)` of all control
    /// points in source pixel space.
    pub(crate) fn bbox(&self) -> (f64, f64, f64, f64) {
        self.grid.iter().fold(
            (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
            |(min_x, min_y, max_x, max_y), p| {
                (
                    min_x.min(p.x),
                    min_y.min(p.y),
                    max_x.max(p.x),
                    max_y.max(p.y),
                )
            },
        )
    }
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

    fn dense_max_error(mapping: &TileMapping, exact: impl Fn(f64, f64) -> Pt) -> f64 {
        let mut worst = 0.0_f64;
        for j in 0..=256 {
            for i in 0..=256 {
                let (u, v) = (i as f64 / 256.0, j as f64 / 256.0);
                let p = exact(u, v);
                let q = mapping.eval(u, v);
                worst = worst.max((p.x - q.x).hypot(p.y - q.y));
            }
        }
        worst
    }

    #[test]
    fn tile_mapping_affine_needs_single_cell() {
        // Rotated + scaled affine mapping is reproduced exactly by one bilinear cell.
        let exact = |u: f64, v: f64| Pt {
            x: 100.0 + 300.0 * u - 50.0 * v,
            y: 20.0 + 40.0 * u + 280.0 * v,
        };
        let m = TileMapping::refine(0.125, 512, |u, v| Ok(exact(u, v))).unwrap();
        assert_eq!(m.cells(), 1);
        assert!(dense_max_error(&m, exact) < 1e-9);
        let (min_x, min_y, max_x, max_y) = m.bbox();
        assert_close(min_x, 50.0);
        assert_close(max_x, 400.0);
        assert_close(min_y, 20.0);
        assert_close(max_y, 340.0);
    }

    #[test]
    fn tile_mapping_refines_until_error_is_below_threshold() {
        // y is quadratic in v: a single cell is off by 10 source px at v = 0.5.
        // Edge scale is 1 source px per output px, so the threshold is 0.125 px;
        // error shrinks 4x per subdivision -> 10/64 > 0.125 >= 10/256 -> 16 cells.
        let exact = |u: f64, v: f64| Pt {
            x: 512.0 * u,
            y: 512.0 * v + 40.0 * v * v,
        };
        let m = TileMapping::refine(0.125, 512, |u, v| Ok(exact(u, v))).unwrap();
        assert_eq!(m.cells(), 16);
        assert!(dense_max_error(&m, exact) <= 0.125);

        // A coarser tolerance stops earlier.
        let coarse = TileMapping::refine(1.0, 512, |u, v| Ok(exact(u, v))).unwrap();
        assert_eq!(coarse.cells(), 4);
        assert!(dense_max_error(&coarse, exact) <= 1.0);
    }

    #[test]
    fn tile_mapping_threshold_scales_with_source_resolution() {
        // Same shape as above but the source has 8 px per output px, so the same
        // 0.125 output px tolerance allows 1 source px of error -> 4 cells.
        let exact = |u: f64, v: f64| Pt {
            x: 4096.0 * u,
            y: 4096.0 * v + 40.0 * v * v,
        };
        let m = TileMapping::refine(0.125, 512, |u, v| Ok(exact(u, v))).unwrap();
        assert_eq!(m.cells(), 4);
    }

    #[test]
    fn tile_mapping_stops_at_max_cells() {
        let exact = |u: f64, v: f64| Pt {
            x: 512.0 * u,
            y: 512.0 * v + 1.0e6 * v * v,
        };
        let m = TileMapping::refine(0.125, 512, |u, v| Ok(exact(u, v))).unwrap();
        assert_eq!(m.cells(), TileMapping::MAX_CELLS);
    }

    #[test]
    fn tile_mapping_rejects_degenerate_and_propagates_errors() {
        let collapsed = TileMapping::refine(0.125, 512, |_, _| Ok(Pt { x: 1.0, y: 1.0 }));
        assert!(collapsed.is_err());

        let failing = TileMapping::refine(0.125, 512, |u, v| {
            if u == 0.5 && v == 0.5 {
                Err("out of projection domain".into())
            } else {
                Ok(Pt {
                    x: 512.0 * u,
                    y: 512.0 * v,
                })
            }
        });
        assert!(failing.is_err());
    }

    #[test]
    fn tile_mapping_eval_is_continuous_across_cells_and_hits_control_points() {
        let exact = |u: f64, v: f64| Pt {
            x: 512.0 * u + 30.0 * u * u,
            y: 512.0 * v + 40.0 * v * v,
        };
        let m = TileMapping::refine(0.125, 512, |u, v| Ok(exact(u, v))).unwrap();
        let n = m.cells();
        for j in 0..=n {
            for i in 0..=n {
                let (u, v) = (i as f64 / n as f64, j as f64 / n as f64);
                let p = m.eval(u, v);
                let e = exact(u, v);
                assert_close(p.x, e.x);
                assert_close(p.y, e.y);
            }
        }
        // Approaching a cell boundary from both sides gives the same position.
        let b = 1.0 / n as f64;
        let left = m.eval(b - 1e-12, 0.3);
        let right = m.eval(b + 1e-12, 0.3);
        assert!((left.x - right.x).abs() < 1e-6 && (left.y - right.y).abs() < 1e-6);
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
