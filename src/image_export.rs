//! Settings and geometry for exporting the 3D view as an image.
//!
//! The export is meant to reproduce what is on screen — same camera, same
//! style, same overlays — with only the background under the caller's control.
//! Nothing is hidden for the export, so anything drawn through the render
//! pipeline (axis triad, NDX colouring, dot surfaces, the simulation cell, the
//! other layers, the selection highlight) comes out too.
//!
//! The workflow the dialog drives is: render a small preview, let the user drag
//! a region on it, then render just that region at full size. The region is
//! kept in *normalised view coordinates* — `[x0, y0, x1, y1]` in `0.0..=1.0`
//! with y pointing down — which is both what egui rects reduce to and what
//! `InteractiveMoleculeViewport::render_image` expects.
//!
//! One invariant runs through all of this: **the output's pixel aspect always
//! equals the region's pixel aspect.** The engine's region crop only re-frames
//! the existing projection, it does not re-fit it, so any mismatch would show up
//! as a stretched image. Output size is therefore always *derived* from the
//! region ([`ExportSettings::output_size`]) rather than set independently.

use std::path::Path;

use moleucle_3dview_rs::DEFAULT_CLEAR_COLOR;

/// Aspect ratio the region drag is locked to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AspectPreset {
    /// Match the on-screen viewport, so a full-view export is exactly the view.
    Screen,
    /// No constraint — the drag decides.
    Free,
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
}

impl AspectPreset {
    pub const ALL: [AspectPreset; 6] = [
        AspectPreset::Screen,
        AspectPreset::Free,
        AspectPreset::Square,
        AspectPreset::FourThree,
        AspectPreset::ThreeTwo,
        AspectPreset::SixteenNine,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AspectPreset::Screen => "Screen",
            AspectPreset::Free => "Free",
            AspectPreset::Square => "1:1",
            AspectPreset::FourThree => "4:3",
            AspectPreset::ThreeTwo => "3:2",
            AspectPreset::SixteenNine => "16:9",
        }
    }

    /// Target width/height, or `None` when the drag is unconstrained.
    pub fn ratio(self, view_aspect: f32) -> Option<f32> {
        match self {
            AspectPreset::Screen => Some(view_aspect),
            AspectPreset::Free => None,
            AspectPreset::Square => Some(1.0),
            AspectPreset::FourThree => Some(4.0 / 3.0),
            AspectPreset::ThreeTwo => Some(3.0 / 2.0),
            AspectPreset::SixteenNine => Some(16.0 / 9.0),
        }
    }
}

/// Long edge of the preview render, in pixels. Small enough that re-rendering it
/// on every settings change is unnoticeable.
pub const PREVIEW_LONG_EDGE: u32 = 520;

/// Bounds on the output's long edge. The upper bound is a sanity limit; the
/// engine additionally clamps against the device's maximum texture size.
pub const MIN_LONG_EDGE: u32 = 128;
pub const MAX_LONG_EDGE: u32 = 8192;

#[derive(Clone, Debug)]
pub struct ExportSettings {
    pub aspect: AspectPreset,
    /// Region to frame, or `None` for the whole view.
    pub region: Option<[f32; 4]>,
    /// Pixels along the output's longer edge; the shorter one follows from the
    /// region's aspect.
    pub long_edge: u32,
    pub transparent: bool,
    /// Background used when `transparent` is false. Defaults to the viewer's own
    /// background so an opaque export matches the screen.
    pub background: [f32; 3],
    /// Render at this multiple and box-filter down, to make up for the
    /// pipeline having no MSAA.
    pub supersample: u32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            aspect: AspectPreset::Screen,
            region: None,
            long_edge: 1920,
            transparent: true,
            background: [
                DEFAULT_CLEAR_COLOR[0],
                DEFAULT_CLEAR_COLOR[1],
                DEFAULT_CLEAR_COLOR[2],
            ],
            supersample: 2,
        }
    }
}

impl ExportSettings {
    /// The region actually rendered — the whole view when none was dragged.
    pub fn effective_region(&self) -> [f32; 4] {
        self.region.unwrap_or([0.0, 0.0, 1.0, 1.0])
    }

    pub fn clear_color(&self) -> [f32; 4] {
        if self.transparent {
            // RGB still matters: it is what partially covered edge pixels blend
            // toward before the premultiplied downsample undoes it. Black keeps
            // that neutral.
            [0.0, 0.0, 0.0, 0.0]
        } else {
            [self.background[0], self.background[1], self.background[2], 1.0]
        }
    }

    /// Output size in pixels, derived from the region so the image is never
    /// stretched. `view` is the on-screen viewport size in pixels.
    pub fn output_size(&self, view: (u32, u32)) -> (u32, u32) {
        let aspect = region_pixel_aspect(self.effective_region(), view);
        let long = self.long_edge.clamp(MIN_LONG_EDGE, MAX_LONG_EDGE);
        if aspect >= 1.0 {
            (long, ((long as f32 / aspect).round() as u32).max(1))
        } else {
            (((long as f32 * aspect).round() as u32).max(1), long)
        }
    }

    /// Preview size in pixels — the whole view at [`PREVIEW_LONG_EDGE`], since
    /// the region is chosen *on* the preview and so cannot crop it.
    pub fn preview_size(&self, view: (u32, u32)) -> (u32, u32) {
        let aspect = region_pixel_aspect([0.0, 0.0, 1.0, 1.0], view);
        if aspect >= 1.0 {
            (
                PREVIEW_LONG_EDGE,
                ((PREVIEW_LONG_EDGE as f32 / aspect).round() as u32).max(1),
            )
        } else {
            (
                ((PREVIEW_LONG_EDGE as f32 * aspect).round() as u32).max(1),
                PREVIEW_LONG_EDGE,
            )
        }
    }
}

/// Pixel aspect (width / height) of a normalised region within `view`.
pub fn region_pixel_aspect(region: [f32; 4], view: (u32, u32)) -> f32 {
    let (vw, vh) = (view.0.max(1) as f32, view.1.max(1) as f32);
    let w = (region[2] - region[0]).abs().max(1e-6) * vw;
    let h = (region[3] - region[1]).abs().max(1e-6) * vh;
    w / h
}

/// Turn a drag from `anchor` to `corner` (both normalised, y down) into a region.
///
/// With a locked `ratio` the rectangle keeps that pixel aspect and is shrunk —
/// never clipped — to stay inside the view, because clipping a corner would
/// silently change the aspect and stretch the export.
pub fn region_from_drag(
    anchor: [f32; 2],
    corner: [f32; 2],
    ratio: Option<f32>,
    view: (u32, u32),
) -> [f32; 4] {
    let (vw, vh) = (view.0.max(1) as f32, view.1.max(1) as f32);
    let dx = corner[0] - anchor[0];
    let dy = corner[1] - anchor[1];

    let Some(ratio) = ratio else {
        let rect = [
            anchor[0].min(corner[0]),
            anchor[1].min(corner[1]),
            anchor[0].max(corner[0]),
            anchor[1].max(corner[1]),
        ];
        return clamp_region(rect);
    };

    // Drive the size from whichever axis the user pulled further, so dragging
    // mostly-vertically feels as responsive as mostly-horizontally.
    let drag_w = dx.abs() * vw;
    let drag_h = dy.abs() * vh;
    let (mut w_px, mut h_px) = if drag_w * drag_w >= drag_h * drag_h * ratio * ratio {
        (drag_w, drag_w / ratio)
    } else {
        (drag_h * ratio, drag_h)
    };

    // Room left between the anchor and the edge we are heading for.
    let room_x = if dx >= 0.0 { 1.0 - anchor[0] } else { anchor[0] } * vw;
    let room_y = if dy >= 0.0 { 1.0 - anchor[1] } else { anchor[1] } * vh;
    let mut scale = 1.0f32;
    if w_px > room_x && w_px > 0.0 {
        scale = scale.min(room_x / w_px);
    }
    if h_px > room_y && h_px > 0.0 {
        scale = scale.min(room_y / h_px);
    }
    w_px *= scale;
    h_px *= scale;

    let end_x = anchor[0] + (w_px / vw) * if dx >= 0.0 { 1.0 } else { -1.0 };
    let end_y = anchor[1] + (h_px / vh) * if dy >= 0.0 { 1.0 } else { -1.0 };
    clamp_region([
        anchor[0].min(end_x),
        anchor[1].min(end_y),
        anchor[0].max(end_x),
        anchor[1].max(end_y),
    ])
}

/// The largest centred region of `ratio` that fits the view, for when an aspect
/// is chosen without dragging anything.
pub fn centred_region(ratio: Option<f32>, view: (u32, u32)) -> Option<[f32; 4]> {
    let ratio = ratio?;
    let view_aspect = region_pixel_aspect([0.0, 0.0, 1.0, 1.0], view);
    if (ratio - view_aspect).abs() < 1e-4 {
        return None; // already the whole view
    }
    let (w, h) = if ratio > view_aspect {
        (1.0, view_aspect / ratio)
    } else {
        (ratio / view_aspect, 1.0)
    };
    Some([
        0.5 - w / 2.0,
        0.5 - h / 2.0,
        0.5 + w / 2.0,
        0.5 + h / 2.0,
    ])
}

/// A region is unusable below roughly a pixel; treat those as "no region".
pub fn is_degenerate(region: [f32; 4], view: (u32, u32)) -> bool {
    let (vw, vh) = (view.0.max(1) as f32, view.1.max(1) as f32);
    (region[2] - region[0]) * vw < 2.0 || (region[3] - region[1]) * vh < 2.0
}

fn clamp_region(r: [f32; 4]) -> [f32; 4] {
    [
        r[0].clamp(0.0, 1.0),
        r[1].clamp(0.0, 1.0),
        r[2].clamp(0.0, 1.0),
        r[3].clamp(0.0, 1.0),
    ]
}

/// Write straight-alpha RGBA8 rows out as a PNG.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: Vec<u8>) -> Result<(), String> {
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        return Err(format!(
            "Pixel buffer is {} bytes, expected {expected} for {width}x{height}",
            rgba.len()
        ));
    }
    let image = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "Could not wrap the pixel buffer as an image".to_string())?;
    image
        .save(path)
        .map_err(|e| format!("Could not write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: (u32, u32) = (1600, 900); // 16:9

    #[test]
    fn a_full_view_export_keeps_the_view_aspect() {
        let settings = ExportSettings {
            long_edge: 1920,
            ..ExportSettings::default()
        };
        assert_eq!(settings.output_size(VIEW), (1920, 1080));
    }

    #[test]
    fn output_size_follows_the_region_not_the_preset() {
        // A square region in a 16:9 view must produce a square image, whatever
        // the preset says, or the crop would come out stretched.
        let settings = ExportSettings {
            region: Some([0.25, 0.0, 0.25 + 900.0 / 1600.0, 1.0]),
            long_edge: 1000,
            ..ExportSettings::default()
        };
        let (w, h) = settings.output_size(VIEW);
        assert_eq!((w, h), (1000, 1000));
    }

    #[test]
    fn a_tall_region_puts_the_long_edge_on_the_height() {
        let settings = ExportSettings {
            region: Some([0.4, 0.0, 0.5, 1.0]), // 160 x 900 px
            long_edge: 900,
            ..ExportSettings::default()
        };
        assert_eq!(settings.output_size(VIEW), (160, 900));
    }

    #[test]
    fn long_edge_is_clamped() {
        let mut settings = ExportSettings {
            long_edge: 99_999,
            ..ExportSettings::default()
        };
        assert_eq!(settings.output_size(VIEW).0, MAX_LONG_EDGE);
        settings.long_edge = 1;
        assert_eq!(settings.output_size(VIEW).0, MIN_LONG_EDGE);
    }

    #[test]
    fn transparent_settings_zero_the_alpha() {
        let settings = ExportSettings::default();
        assert!(settings.transparent);
        assert_eq!(settings.clear_color()[3], 0.0);
    }

    #[test]
    fn an_opaque_export_defaults_to_the_viewer_background() {
        let settings = ExportSettings {
            transparent: false,
            ..ExportSettings::default()
        };
        assert_eq!(settings.clear_color(), DEFAULT_CLEAR_COLOR);
    }

    #[test]
    fn a_free_drag_is_just_the_normalised_rectangle() {
        let r = region_from_drag([0.2, 0.3], [0.6, 0.8], None, VIEW);
        assert_eq!(r, [0.2, 0.3, 0.6, 0.8]);
    }

    #[test]
    fn a_backwards_drag_still_yields_an_ordered_rectangle() {
        let r = region_from_drag([0.6, 0.8], [0.2, 0.3], None, VIEW);
        assert_eq!(r, [0.2, 0.3, 0.6, 0.8]);
    }

    #[test]
    fn a_locked_drag_holds_its_pixel_aspect() {
        for preset in [
            AspectPreset::Square,
            AspectPreset::FourThree,
            AspectPreset::SixteenNine,
        ] {
            let ratio = preset.ratio(16.0 / 9.0).unwrap();
            let r = region_from_drag([0.1, 0.1], [0.6, 0.9], Some(ratio), VIEW);
            let got = region_pixel_aspect(r, VIEW);
            assert!(
                (got - ratio).abs() < 1e-3,
                "{}: aspect {got} != {ratio}",
                preset.label()
            );
        }
    }

    #[test]
    fn a_locked_drag_shrinks_rather_than_clipping_at_the_edge() {
        // Dragging far past the bottom edge with a 1:1 lock: the rectangle has to
        // stay square, so it must be scaled down, not cut off.
        let r = region_from_drag([0.1, 0.6], [0.99, 0.99], Some(1.0), VIEW);
        assert!(r[3] <= 1.0 + 1e-6, "stays inside the view: {r:?}");
        let got = region_pixel_aspect(r, VIEW);
        assert!((got - 1.0).abs() < 1e-3, "still square: {got}");
    }

    #[test]
    fn a_locked_drag_upward_and_leftward_works_too() {
        let r = region_from_drag([0.8, 0.8], [0.3, 0.2], Some(1.0), VIEW);
        assert!(r[0] < r[2] && r[1] < r[3], "ordered: {r:?}");
        let got = region_pixel_aspect(r, VIEW);
        assert!((got - 1.0).abs() < 1e-3, "{got}");
    }

    #[test]
    fn a_centred_region_matches_the_preset_and_is_centred() {
        let r = centred_region(Some(1.0), VIEW).expect("1:1 differs from 16:9");
        assert!((region_pixel_aspect(r, VIEW) - 1.0).abs() < 1e-3);
        assert!(((r[0] + r[2]) / 2.0 - 0.5).abs() < 1e-6);
        assert!(((r[1] + r[3]) / 2.0 - 0.5).abs() < 1e-6);
        // It should touch the short edge, using all the room there is.
        assert!((r[1] - 0.0).abs() < 1e-6 && (r[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_screen_preset_needs_no_region() {
        let view_aspect = region_pixel_aspect([0.0, 0.0, 1.0, 1.0], VIEW);
        assert!(centred_region(AspectPreset::Screen.ratio(view_aspect), VIEW).is_none());
        assert!(centred_region(AspectPreset::Free.ratio(view_aspect), VIEW).is_none());
    }

    #[test]
    fn degenerate_regions_are_detected() {
        assert!(is_degenerate([0.5, 0.5, 0.5, 0.5], VIEW));
        assert!(is_degenerate([0.5, 0.2, 0.5004, 0.8], VIEW), "sub-pixel width");
        assert!(!is_degenerate([0.2, 0.2, 0.8, 0.8], VIEW));
    }

    #[test]
    fn write_png_rejects_a_mismatched_buffer() {
        let err = write_png(Path::new("unused.png"), 2, 2, vec![0; 8]).unwrap_err();
        assert!(err.contains("expected 16"), "{err}");
    }

    #[test]
    fn write_png_round_trips_alpha() {
        let dir = std::env::temp_dir().join("manul_cat_rs_export_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("alpha.png");
        // One opaque red pixel and one fully transparent one.
        let rgba = vec![255, 0, 0, 255, 0, 0, 0, 0];
        write_png(&path, 2, 1, rgba).unwrap();

        let decoded = image::open(&path).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(1, 0).0[3], 0, "alpha survived the PNG");
        let _ = std::fs::remove_file(&path);
    }
}
