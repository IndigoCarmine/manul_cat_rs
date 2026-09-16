//! Settings and framing for exporting a trajectory as a video.
//!
//! Like [`crate::image_export`] this module is pure: no GPU, no egui, no file
//! system. It decides *what* to render — which trajectory frames, at what size,
//! at what rate — and the app drives the rendering one frame per UI frame.
//!
//! Two constraints from the encoder shape everything here:
//!
//! * **Dimensions must be even.** The video is written as H.264 `yuv420p`,
//!   which subsamples chroma 2x2 and so cannot describe an odd-sized image.
//!   [`VideoSettings::output_size`] therefore always returns an even pair.
//! * **There is no alpha.** The still export can write a transparent PNG; a
//!   video cannot, so the background colour is always composited in. That is
//!   why this has a `background` and no `transparent`.
//!
//! The framing is deliberately the whole view rather than a dragged region: a
//! trajectory movie wants to show what is on screen, and the region machinery
//! would mean duplicating the still dialog's drag-preview for little gain.

use crate::ffmpeg::Quality;
use moleucle_3dview_rs::DEFAULT_CLEAR_COLOR;

/// Bounds on the output's long edge, in pixels.
///
/// The ceiling is lower than the still export's 8192: every frame is rendered,
/// read back and PNG-encoded, so the cost is paid hundreds of times over, and
/// 4K is already past what a trajectory movie is shown at.
pub const MIN_LONG_EDGE: u32 = 128;
pub const MAX_LONG_EDGE: u32 = 3840;

/// Bounds on the output frame rate.
pub const MIN_FPS: u32 = 1;
pub const MAX_FPS: u32 = 60;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoSettings {
    /// Long edge of the output, in pixels. The short edge follows the view's
    /// aspect ratio.
    pub long_edge: u32,
    /// Supersampling factor, as in the still export. Quadratic in cost, and
    /// here that cost is per frame.
    pub supersample: u32,
    /// Background composited under the molecule. Opaque by necessity.
    pub background: [f32; 3],
    /// Frames per second of the finished video.
    pub fps: u32,
    /// Take every Nth trajectory frame. 1 exports all of them.
    pub frame_step: usize,
    pub quality: Quality,
    /// Keep the intermediate PNG sequence next to the video instead of deleting
    /// it once the encode succeeds.
    pub keep_frames: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            long_edge: 1280,
            // The still export defaults to 2x, but that is one render. Here it
            // would quadruple the cost of every frame in the trajectory, so the
            // default trades the edge quality for an export that finishes.
            supersample: 1,
            background: [
                DEFAULT_CLEAR_COLOR[0],
                DEFAULT_CLEAR_COLOR[1],
                DEFAULT_CLEAR_COLOR[2],
            ],
            fps: 30,
            frame_step: 1,
            quality: Quality::Balanced,
            keep_frames: false,
        }
    }
}

impl VideoSettings {
    /// Opaque clear colour for the render.
    pub fn clear_color(&self) -> [f32; 4] {
        [
            self.background[0],
            self.background[1],
            self.background[2],
            1.0,
        ]
    }

    /// Output size in pixels for a viewport of `view`, always even in both axes.
    ///
    /// Rounding is downward so the result never exceeds the requested long edge,
    /// and both axes are floored to at least 2 — a zero-sized render is an error
    /// from the engine and a 1-pixel one cannot be `yuv420p`.
    pub fn output_size(&self, view: (u32, u32)) -> (u32, u32) {
        let long_edge = self.long_edge.clamp(MIN_LONG_EDGE, MAX_LONG_EDGE);
        let width = view.0.max(1) as f32;
        let height = view.1.max(1) as f32;

        let (w, h) = if width >= height {
            (long_edge as f32, long_edge as f32 * height / width)
        } else {
            (long_edge as f32 * width / height, long_edge as f32)
        };

        (to_even(w), to_even(h))
    }

    /// The trajectory frame indices this export will render, in order.
    ///
    /// Always includes frame 0 when the trajectory is non-empty, so a stepped
    /// export starts where playback would.
    pub fn frame_indices(&self, trajectory_len: usize) -> Vec<usize> {
        if trajectory_len == 0 {
            return Vec::new();
        }
        let step = self.frame_step.max(1);
        (0..trajectory_len).step_by(step).collect()
    }

    /// How long the finished video will run, in seconds.
    pub fn duration_secs(&self, trajectory_len: usize) -> f32 {
        let frames = self.frame_indices(trajectory_len).len() as f32;
        frames / self.fps.clamp(MIN_FPS, MAX_FPS) as f32
    }
}

/// Round `value` down to an even integer, never below 2.
fn to_even(value: f32) -> u32 {
    let rounded = value.max(2.0).round() as u32;
    (rounded & !1).max(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_size_is_always_even() {
        // yuv420p cannot encode an odd dimension, so every view aspect and every
        // long edge has to come out even -- including the awkward ones.
        let settings = VideoSettings::default();
        for view in [
            (1920, 1080),
            (1001, 999),
            (1365, 767),
            (3, 7),
            (1, 1),
            (800, 1279),
        ] {
            for long_edge in [128, 129, 720, 721, 1280, 1281, 3840] {
                let settings = VideoSettings {
                    long_edge,
                    ..settings
                };
                let (w, h) = settings.output_size(view);
                assert_eq!(w % 2, 0, "width {w} odd for view {view:?} @ {long_edge}");
                assert_eq!(h % 2, 0, "height {h} odd for view {view:?} @ {long_edge}");
                assert!(w >= 2 && h >= 2, "{w}x{h} is too small to encode");
            }
        }
    }

    #[test]
    fn output_size_never_exceeds_the_requested_long_edge() {
        let settings = VideoSettings {
            long_edge: 1280,
            ..VideoSettings::default()
        };
        for view in [(1920, 1080), (1080, 1920), (1000, 1000)] {
            let (w, h) = settings.output_size(view);
            assert!(w.max(h) <= 1280, "{w}x{h} overshoots for view {view:?}");
        }
    }

    #[test]
    fn output_size_keeps_the_view_aspect() {
        let settings = VideoSettings {
            long_edge: 1920,
            ..VideoSettings::default()
        };
        let (w, h) = settings.output_size((1600, 900));
        // Even-rounding can move each axis by at most one pixel.
        let want = 16.0 / 9.0;
        let got = w as f32 / h as f32;
        assert!(
            (got - want).abs() < 0.01,
            "{w}x{h} is {got}, expected about {want}"
        );
    }

    #[test]
    fn output_size_clamps_the_long_edge_into_range() {
        let view = (1920, 1080);
        let tiny = VideoSettings {
            long_edge: 1,
            ..VideoSettings::default()
        };
        let huge = VideoSettings {
            long_edge: 100_000,
            ..VideoSettings::default()
        };
        assert_eq!(tiny.output_size(view).0, MIN_LONG_EDGE);
        assert_eq!(huge.output_size(view).0, MAX_LONG_EDGE);
    }

    #[test]
    fn frame_indices_start_at_zero_and_respect_the_step() {
        let settings = VideoSettings {
            frame_step: 3,
            ..VideoSettings::default()
        };
        assert_eq!(settings.frame_indices(10), vec![0, 3, 6, 9]);
    }

    #[test]
    fn a_zero_step_still_advances() {
        // The UI clamps this, but a zero here would otherwise hang the export
        // on frame 0 forever.
        let settings = VideoSettings {
            frame_step: 0,
            ..VideoSettings::default()
        };
        assert_eq!(settings.frame_indices(4), vec![0, 1, 2, 3]);
    }

    #[test]
    fn an_empty_trajectory_has_nothing_to_render() {
        assert!(VideoSettings::default().frame_indices(0).is_empty());
    }

    #[test]
    fn duration_follows_the_frame_count_and_rate() {
        let settings = VideoSettings {
            fps: 30,
            frame_step: 1,
            ..VideoSettings::default()
        };
        assert!((settings.duration_secs(60) - 2.0).abs() < 1e-6);

        let stepped = VideoSettings {
            frame_step: 2,
            ..settings
        };
        assert!((stepped.duration_secs(60) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_clear_color_is_opaque() {
        // A video has no alpha channel; letting a transparent clear through
        // would render the background black instead of the chosen colour.
        let settings = VideoSettings {
            background: [0.2, 0.4, 0.6],
            ..VideoSettings::default()
        };
        assert_eq!(settings.clear_color(), [0.2, 0.4, 0.6, 1.0]);
    }
}
