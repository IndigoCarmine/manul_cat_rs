use eframe::egui;
use material_icons::{Icon as MaterialIcon, icon_to_char};
use moleucle_3dview_rs::RenderStyle;

use crate::{ffmpeg, video_export};

use super::{KuromameApp, LeftPanelTab};

/// Color palette for the "Viewer UI" dark design. Shared with `app.rs` so the
/// global theme and the per-panel frames stay in sync.
pub mod theme {
    use eframe::egui::Color32;

    pub const BG: Color32 = Color32::from_rgb(0x06, 0x09, 0x0f);
    pub const PANEL: Color32 = Color32::from_rgb(0x0d, 0x11, 0x17);
    pub const BORDER: Color32 = Color32::from_rgb(0x21, 0x26, 0x2d);
    pub const BORDER2: Color32 = Color32::from_rgb(0x30, 0x36, 0x3d);
    pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xed, 0xf3);
    pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
    pub const MUTED2: Color32 = Color32::from_rgb(0x58, 0x60, 0x69);
    pub const ACCENT: Color32 = Color32::from_rgb(0x4c, 0xa3, 0xff);
    pub const ACCENT_FG: Color32 = Color32::from_rgb(0x04, 0x11, 0x1f);
    pub const GREEN: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
    pub const AMBER: Color32 = Color32::from_rgb(0xe0, 0xb3, 0x41);
    pub const INPUT_BG: Color32 = Color32::from_rgb(0x01, 0x04, 0x09);
    pub const HOVER_BG: Color32 = Color32::from_rgb(0x16, 0x1b, 0x22);
    /// Layer-card fill and its resting (inactive) border, from the "1A" design.
    pub const CARD_BG: Color32 = Color32::from_rgb(0x14, 0x1b, 0x24);
    pub const CARD_BORDER: Color32 = Color32::from_rgb(0x23, 0x2c, 0x38);
    /// Count-badge pill background / text.
    pub const BADGE_BG: Color32 = Color32::from_rgb(0x1c, 0x25, 0x31);
    pub const BADGE_FG: Color32 = Color32::from_rgb(0xad, 0xba, 0xc7);
}

/// A small rounded count pill (e.g. the `2` next to a `LAYERS` header).
fn count_badge(ui: &mut egui::Ui, n: usize) {
    egui::Frame::new()
        .fill(theme::BADGE_BG)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(7, 1))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(n.to_string())
                    .size(10.5)
                    .color(theme::BADGE_FG)
                    .strong(),
            );
        });
}

/// A 12×12 rounded colour swatch used as a layer's identity marker.
fn color_swatch(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(4), color);
}

fn rgb_to_color32(c: [f32; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c[0] * 255.0).round().clamp(0.0, 255.0) as u8,
        (c[1] * 255.0).round().clamp(0.0, 255.0) as u8,
        (c[2] * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn mi(icon: MaterialIcon) -> String {
    icon_to_char(icon).to_string()
}

/// Small uppercase section header, e.g. "SELECTION".
fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(11.0)
            .color(theme::MUTED2)
            .strong(),
    );
}

/// A small outlined type badge pill (e.g. `GRO`, `TOP`, `NDX`), matching the
/// "Viewer UI" design's file-format chips.
fn type_badge(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, theme::BORDER2))
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(10.5)
                    .color(theme::MUTED)
                    .strong(),
            );
        });
}

/// A secondary (outlined) action button sized to fill the available width slot.
fn secondary_button(ui: &mut egui::Ui, label: String, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).color(theme::TEXT).size(12.5))
            .fill(theme::HOVER_BG)
            .stroke(egui::Stroke::new(1.0, theme::BORDER2))
            .corner_radius(egui::CornerRadius::same(7))
            .min_size(egui::vec2(0.0, 30.0)),
    )
}

pub fn render_edit_dialog(app: &mut KuromameApp, ctx: &egui::Context) {
    let mut open_edit_dialog = app.ui.show_edit_dialog;

    if open_edit_dialog {
        let mut close_requested = false;
        egui::Window::new("Edit Residue Name")
            .open(&mut open_edit_dialog)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Enter new residue name (3 letters):");
                let edit_response = ui.text_edit_singleline(&mut app.ui.new_res_name);
                if !edit_response.has_focus() {
                    ui.memory_mut(|mem| mem.request_focus(edit_response.id));
                }

                let apply_by_enter =
                    edit_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let apply_by_button = ui.button("Apply").clicked();

                if apply_by_enter || apply_by_button {
                    app.apply_res_name_change();
                    app.ui.show_edit_dialog = false;
                    close_requested = true;
                }
            });
        if close_requested {
            open_edit_dialog = false;
        }
        app.ui.show_edit_dialog = open_edit_dialog;
    }
}

/// Checker size (px) behind a transparent preview, so alpha is visible.
const CHECKER: f32 = 8.0;

/// Paint the standard light/dark checkerboard, marking out where the image is
/// transparent rather than dark-coloured.
fn paint_checkerboard(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, egui::CornerRadius::ZERO, theme::MUTED2);
    let cols = (rect.width() / CHECKER).ceil() as i32;
    let rows = (rect.height() / CHECKER).ceil() as i32;
    for row in 0..rows {
        for col in 0..cols {
            if (row + col) % 2 != 0 {
                continue;
            }
            let cell = egui::Rect::from_min_size(
                rect.min + egui::vec2(col as f32 * CHECKER, row as f32 * CHECKER),
                egui::vec2(CHECKER, CHECKER),
            )
            .intersect(rect);
            painter.rect_filled(cell, egui::CornerRadius::ZERO, theme::BADGE_FG);
        }
    }
}

/// The image-export dialog: a low-resolution render of the current view, a
/// region you drag on it, and the resulting full-size export.
///
/// The preview deliberately shows the *export's* background rather than the
/// viewer's, so a transparent export reads as a checkerboard before it is saved.
pub fn render_export_dialog(app: &mut KuromameApp, ctx: &egui::Context) {
    if !app.export_dialog_open() {
        return;
    }
    let mut open = true;
    let mut close_requested = false;
    let mut save_requested = false;
    let mut reset_region = false;
    let mut clear_region = false;

    egui::Window::new("Export Image")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(560.0)
        .show(ctx, |ui| {
            let view = app.viewport_pixel_size();

            // ---- preview + region drag -------------------------------------
            if let Some(texture) = app.export_preview().cloned() {
                let size = texture.size_vec2();
                let (rect, response) =
                    ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                let painter = ui.painter_at(rect);
                if app.export_settings().transparent {
                    paint_checkerboard(&painter, rect);
                }
                painter.image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );

                // Pointer position as a normalised view coordinate (y down),
                // which is the space both the region and the engine work in.
                let to_norm = |pos: egui::Pos2| {
                    [
                        ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
                        ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
                    ]
                };

                let view_aspect = crate::image_export::region_pixel_aspect([0.0, 0.0, 1.0, 1.0], view);
                let ratio = app.export_settings().aspect.ratio(view_aspect);

                if response.drag_started()
                    && let Some(pos) = response.interact_pointer_pos() {
                        app.set_export_drag_anchor(Some(to_norm(pos)));
                    }
                if let (Some(anchor), Some(pos)) =
                    (app.export_drag_anchor(), response.interact_pointer_pos())
                    && (response.dragged() || response.drag_stopped()) {
                        let region = crate::image_export::region_from_drag(
                            anchor,
                            to_norm(pos),
                            ratio,
                            view,
                        );
                        // A click without a real drag means "clear the region",
                        // handled on release below.
                        if !crate::image_export::is_degenerate(region, view) {
                            app.set_export_region(Some(region));
                        }
                    }
                if response.drag_stopped() {
                    app.set_export_drag_anchor(None);
                }
                if response.clicked() {
                    clear_region = true;
                }

                // ---- region outline + dimmed surroundings -------------------
                let region = app.export_settings().region;
                if let Some([x0, y0, x1, y1]) = region {
                    let sel = egui::Rect::from_min_max(
                        rect.min + egui::vec2(x0 * rect.width(), y0 * rect.height()),
                        rect.min + egui::vec2(x1 * rect.width(), y1 * rect.height()),
                    );
                    let shade = egui::Color32::from_black_alpha(120);
                    // Four bands around the selection, so the kept area stays clean.
                    for band in [
                        egui::Rect::from_min_max(rect.left_top(), egui::pos2(rect.right(), sel.top())),
                        egui::Rect::from_min_max(
                            egui::pos2(rect.left(), sel.bottom()),
                            rect.right_bottom(),
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(rect.left(), sel.top()),
                            egui::pos2(sel.left(), sel.bottom()),
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(sel.right(), sel.top()),
                            egui::pos2(rect.right(), sel.bottom()),
                        ),
                    ] {
                        if band.is_positive() {
                            painter.rect_filled(band, egui::CornerRadius::ZERO, shade);
                        }
                    }
                    painter.rect_stroke(
                        sel,
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.5, theme::ACCENT),
                        egui::StrokeKind::Middle,
                    );
                }

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(if region.is_some() {
                        "Drag to re-frame · click to use the whole view"
                    } else {
                        "Drag on the preview to pick a region"
                    })
                    .color(theme::MUTED2)
                    .size(11.0),
                );
            } else if let Some(err) = app.export_error() {
                ui.colored_label(egui::Color32::from_rgb(0xe0, 0xb3, 0x41), err);
            } else {
                ui.label(
                    egui::RichText::new("Rendering preview…")
                        .color(theme::MUTED)
                        .size(12.0),
                );
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            // ---- settings ---------------------------------------------------
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Aspect").color(theme::MUTED).size(12.0));
                let current = app.export_settings().aspect;
                egui::ComboBox::from_id_salt("export_aspect")
                    .selected_text(current.label())
                    .show_ui(ui, |ui| {
                        for preset in crate::image_export::AspectPreset::ALL {
                            if ui
                                .selectable_label(preset == current, preset.label())
                                .clicked()
                                && preset != current
                            {
                                app.edit_export_settings().aspect = preset;
                                reset_region = true;
                            }
                        }
                    });

                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("Long edge")
                        .color(theme::MUTED)
                        .size(12.0),
                );
                let mut long_edge = app.export_settings().long_edge;
                if ui
                    .add(
                        egui::DragValue::new(&mut long_edge)
                            .speed(16.0)
                            .range(
                                crate::image_export::MIN_LONG_EDGE
                                    ..=crate::image_export::MAX_LONG_EDGE,
                            )
                            .suffix(" px"),
                    )
                    .changed()
                {
                    app.edit_export_settings().long_edge = long_edge;
                }

                ui.add_space(12.0);
                let mut supersample = app.export_settings().supersample;
                ui.label(egui::RichText::new("AA").color(theme::MUTED).size(12.0))
                    .on_hover_text(
                        "Supersampling: render this many times larger, then shrink.\n\
                         The renderer has no MSAA, so this is what smooths the edges.",
                    );
                egui::ComboBox::from_id_salt("export_supersample")
                    .selected_text(format!("{supersample}x"))
                    .show_ui(ui, |ui| {
                        for factor in [1u32, 2, 3] {
                            if ui
                                .selectable_label(factor == supersample, format!("{factor}x"))
                                .clicked()
                            {
                                supersample = factor;
                            }
                        }
                    });
                if supersample != app.export_settings().supersample {
                    app.edit_export_settings().supersample = supersample;
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let mut transparent = app.export_settings().transparent;
                if ui
                    .checkbox(&mut transparent, "Transparent background")
                    .changed()
                {
                    app.edit_export_settings().transparent = transparent;
                }
                if !transparent {
                    ui.add_space(10.0);
                    let mut background = app.export_settings().background;
                    if ui.color_edit_button_rgb(&mut background).changed() {
                        app.edit_export_settings().background = background;
                    }
                }
            });

            ui.add_space(6.0);
            let (out_w, out_h) = app.export_output_size();
            let region_note = match app.export_settings().region {
                Some(_) => "region",
                None => "whole view",
            };
            ui.label(
                egui::RichText::new(format!("Output: {out_w} x {out_h} px  ({region_note})"))
                    .color(theme::MUTED)
                    .size(12.0),
            );
            if app.export_preview().is_some()
                && let Some(err) = app.export_error() {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(0xe0, 0xb3, 0x41), err);
                }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Save PNG…")
                                .color(theme::ACCENT_FG)
                                .size(12.5)
                                .strong(),
                        )
                        .fill(theme::ACCENT)
                        .corner_radius(egui::CornerRadius::same(7))
                        .min_size(egui::vec2(110.0, 30.0)),
                    )
                    .clicked()
                {
                    save_requested = true;
                }
                if secondary_button(ui, "Whole view".to_string(), true).clicked() {
                    clear_region = true;
                }
                if secondary_button(ui, "Close".to_string(), true).clicked() {
                    close_requested = true;
                }
            });
        });

    if reset_region {
        app.reset_export_region_for_aspect();
    }
    if clear_region {
        app.set_export_region(None);
        app.set_export_drag_anchor(None);
    }
    if save_requested {
        app.pick_export_image_path();
    }
    if !open || close_requested {
        app.close_export_image_dialog();
    }
}

/// The video-export dialog: settings while idle, progress while running.
///
/// Deferred actions are collected into locals and applied after the window
/// closure, the same way [`render_export_dialog`] does, because the closure
/// already borrows `app`.
pub fn render_video_dialog(app: &mut KuromameApp, ctx: &egui::Context) {
    render_ffmpeg_install_prompt(app, ctx);

    if !app.video_dialog_open() {
        return;
    }
    let running = app.video_is_running();
    let mut open = true;
    let mut close_requested = false;
    let mut export_requested = false;
    let mut cancel_requested = false;
    let mut install_requested = false;

    egui::Window::new("Export Video")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            let frames = app.trajectory_frame_count();

            if running {
                render_video_progress(app, ui, &mut cancel_requested);
                return;
            }

            let mut settings = app.video_settings();
            let mut changed = false;

            egui::Grid::new("video_export_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Resolution");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut settings.long_edge)
                                .range(video_export::MIN_LONG_EDGE..=video_export::MAX_LONG_EDGE)
                                .speed(16.0)
                                .suffix(" px"),
                        )
                        .on_hover_text("Long edge; the other follows the view's aspect")
                        .changed();
                    ui.end_row();

                    ui.label("Frame rate");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut settings.fps)
                                .range(video_export::MIN_FPS..=video_export::MAX_FPS)
                                .suffix(" fps"),
                        )
                        .changed();
                    ui.end_row();

                    ui.label("Use every");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut settings.frame_step)
                                .range(1..=100usize)
                                .suffix(" frame(s)"),
                        )
                        .on_hover_text("Skip frames to shorten a long trajectory")
                        .changed();
                    ui.end_row();

                    ui.label("Anti-aliasing");
                    egui::ComboBox::from_id_salt("video_supersample")
                        .selected_text(format!("{}x", settings.supersample))
                        .show_ui(ui, |ui| {
                            for factor in [1u32, 2, 3] {
                                changed |= ui
                                    .selectable_value(
                                        &mut settings.supersample,
                                        factor,
                                        format!("{factor}x"),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();

                    ui.label("Quality");
                    egui::ComboBox::from_id_salt("video_quality")
                        .selected_text(settings.quality.label())
                        .show_ui(ui, |ui| {
                            for quality in ffmpeg::Quality::ALL {
                                changed |= ui
                                    .selectable_value(
                                        &mut settings.quality,
                                        quality,
                                        quality.label(),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();

                    ui.label("Background");
                    changed |= ui
                        .color_edit_button_rgb(&mut settings.background)
                        .on_hover_text("A video has no alpha channel, so this is always painted")
                        .changed();
                    ui.end_row();
                });

            ui.add_space(4.0);
            changed |= ui
                .checkbox(&mut settings.keep_frames, "Keep the PNG sequence")
                .on_hover_text(
                    "Leave the rendered frames next to the video instead of deleting them",
                )
                .changed();

            if changed {
                app.edit_video_settings(|current| *current = settings);
            }

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);

            // ---- what this will produce ------------------------------------
            let view = app.viewport_pixel_size();
            let (out_w, out_h) = settings.output_size(view);
            let planned = settings.frame_indices(frames).len();
            let duration = settings.duration_secs(frames);
            ui.label(
                egui::RichText::new(format!(
                    "{out_w} x {out_h} px  ·  {planned} of {frames} frames  ·  {duration:.1} s"
                ))
                .weak(),
            );

            // ---- encoder status --------------------------------------------
            ui.add_space(6.0);
            match app.ffmpeg_path() {
                Some(path) => {
                    ui.label(
                        egui::RichText::new(format!("Encoder: {}", path.display()))
                            .weak()
                            .small(),
                    );
                }
                None => {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "ffmpeg was not found — the export will write the PNG sequence only.",
                    );
                    if app.ffmpeg_install_running() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Installing ffmpeg…");
                        });
                    } else if app.ffmpeg_install_is_managed() {
                        if ui.button("Install ffmpeg…").clicked() {
                            install_requested = true;
                        }
                    } else {
                        ui.label(
                            egui::RichText::new(format!(
                                "Install it with:\n{}",
                                app.ffmpeg_install_route().display_command()
                            ))
                            .weak()
                            .small(),
                        );
                    }
                }
            }

            if let Some(error) = app.video_error() {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = if app.ffmpeg_path().is_some() {
                    "Export MP4…"
                } else {
                    "Export frames…"
                };
                if ui.button(label).clicked() {
                    export_requested = true;
                }
                if ui.button("Close").clicked() {
                    close_requested = true;
                }
            });
        });

    if export_requested && let Some(path) = app.pick_export_video_path() {
        app.request_export_video(path);
    }
    if install_requested {
        app.open_ffmpeg_install_prompt();
    }
    if cancel_requested {
        app.request_cancel_video();
    }
    // The window's own close button is ignored while an export runs: the dialog
    // is the only place the progress and the cancel button live.
    if close_requested || (!open && !running) {
        app.close_export_video_dialog();
    }
}

fn render_video_progress(app: &KuromameApp, ui: &mut egui::Ui, cancel: &mut bool) {
    let (done, total) = app.video_progress().unwrap_or((0, 0));
    let encoding = app.video_encoding();

    ui.label(if encoding {
        "Encoding the video…".to_string()
    } else {
        format!("Rendering frame {done} of {total}")
    });
    ui.add_space(6.0);

    let bar = if encoding {
        egui::ProgressBar::new(1.0).animate(true).text("ffmpeg")
    } else {
        let fraction = if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        };
        egui::ProgressBar::new(fraction).text(format!("{:.0}%", fraction * 100.0))
    };
    ui.add(bar);

    ui.add_space(8.0);
    // Cancelling during the encode would leave a half-written file, so the
    // button is only live while frames are still being rendered.
    ui.add_enabled_ui(!encoding, |ui| {
        if ui.button("Cancel").clicked() {
            *cancel = true;
        }
    });
}

/// Consent dialog shown before the app runs the platform's package manager.
///
/// The exact command is on screen: nothing runs that the user has not read.
fn render_ffmpeg_install_prompt(app: &mut KuromameApp, ctx: &egui::Context) {
    if !app.ffmpeg_install_prompt_open() {
        return;
    }
    let mut confirm = false;
    let mut cancel = false;

    egui::Window::new("Install ffmpeg")
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label(
                "Manul does not bundle a video encoder. It can install one for you through \
                 your system's package manager.",
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new("This will run:").strong());
            ui.add_space(2.0);
            ui.code(app.ffmpeg_install_route().display_command());
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "The download is roughly 100 MB and is handled by the package manager, \
                     not by Manul.",
                )
                .weak()
                .small(),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Install").clicked() {
                    confirm = true;
                }
                if ui.button("Not now").clicked() {
                    cancel = true;
                }
            });
        });

    if confirm {
        app.start_ffmpeg_install();
    } else if cancel {
        app.close_ffmpeg_install_prompt();
    }
}

pub fn render_menu_bar(app: &mut KuromameApp, ui: &mut egui::Ui) {
    egui::Panel::top("menu_bar")
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(14, 8)),
        )
        .show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // Wordmark.
                ui.label(egui::RichText::new("■").color(theme::ACCENT).size(13.0));
                ui.label(
                    egui::RichText::new("MD Viewer")
                        .color(theme::TEXT)
                        .strong()
                        .size(13.0),
                );
                ui.add_space(8.0);

                file_menu(app, ui);
                render_menu(app, ui);
                selection_menu(app, ui);
                help_menu(ui);

                // Right-aligned saved/modified indicator.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (color, text) = if app.data.is_modified {
                        (theme::AMBER, "modified")
                    } else {
                        (theme::GREEN, "saved")
                    };
                    ui.label(egui::RichText::new(text).color(color).size(12.0));
                    ui.label(egui::RichText::new("●").color(color).size(10.0));
                });
            });
        });
}

fn file_menu(app: &mut KuromameApp, ui: &mut egui::Ui) {
    ui.menu_button("File", |ui| {
        if ui
            .button(format!("{} Open Molecule", mi(MaterialIcon::FolderOpen)))
            .on_hover_text("Ctrl+O (PDB/MOL2)")
            .clicked()
        {
            app.open_file();
            ui.close();
        }
        if ui
            .button(format!("{} Open TOP", mi(MaterialIcon::Description)))
            .on_hover_text("Ctrl+T")
            .clicked()
        {
            app.open_top_file();
            ui.close();
        }
        if ui
            .button(format!("{} Open GRO", mi(MaterialIcon::GridOn)))
            .on_hover_text("Ctrl+G")
            .clicked()
        {
            app.open_gro_file();
            ui.close();
        }
        if ui
            .button(format!("{} Open TOP+GRO", mi(MaterialIcon::FileOpen)))
            .on_hover_text("Ctrl+Shift+O")
            .clicked()
        {
            app.open_top_and_gro_for_resname_sync();
            ui.close();
        }
        if ui
            .button(format!("{} Open XTC", mi(MaterialIcon::Movie)))
            .on_hover_text("Load XTC trajectory")
            .clicked()
        {
            app.open_xtc_file();
            ui.close();
        }
        if ui
            .button(format!("{} Import NDX", mi(MaterialIcon::UploadFile)))
            .clicked()
        {
            app.open_ndx_file();
            ui.close();
        }
        if ui
            .button(format!("{} Import PLUMED", mi(MaterialIcon::Rule)))
            .on_hover_text("Inspect a PLUMED input file against the loaded structure")
            .clicked()
        {
            app.open_plumed_file();
            ui.close();
        }
        if ui
            .button(format!("{} Add Overlay Surface", mi(MaterialIcon::Layers)))
            .on_hover_text("Overlay a PDB dot surface on top of the current structure")
            .clicked()
        {
            app.open_overlay_surface_file();
            ui.close();
        }
        if ui
            .button(format!("{} Add Layer", mi(MaterialIcon::Layers)))
            .on_hover_text("Add a new layer and load a structure into it")
            .clicked()
        {
            app.add_layer();
            ui.close();
        }
        ui.separator();
        if ui
            .button(format!("{} Export", mi(MaterialIcon::Save)))
            .on_hover_text("Ctrl+S")
            .clicked()
        {
            app.export_structure();
            ui.close();
        }
        if ui
            .button(format!("{} Export Image…", mi(MaterialIcon::Image)))
            .on_hover_text("Ctrl+E — render the view to a PNG, optionally transparent")
            .clicked()
        {
            app.open_export_image_dialog();
            ui.close();
        }
        if ui
            .button(format!("{} Export Video…", mi(MaterialIcon::Movie)))
            .on_hover_text("Render the loaded trajectory to an MP4")
            .clicked()
        {
            app.open_export_video_dialog();
            ui.close();
        }
        ui.separator();
        if ui
            .button(format!("{} Update All", mi(MaterialIcon::Refresh)))
            .on_hover_text("Reload all currently loaded files")
            .clicked()
        {
            app.reload_loaded_files();
            ui.close();
        }
    });
}

fn render_menu(app: &mut KuromameApp, ui: &mut egui::Ui) {
    ui.menu_button("Render", |ui| {
        let mut style = app.viewport.render_style();
        ui.selectable_value(&mut style, RenderStyle::BallStick, "Ball + Stick");
        ui.selectable_value(&mut style, RenderStyle::BallOnly, "Ball only");
        ui.selectable_value(&mut style, RenderStyle::Wireframe, "Wireframe");
        ui.selectable_value(&mut style, RenderStyle::Circles, "Circles");
        app.viewport.set_render_style(style);
    });
}

fn selection_menu(app: &mut KuromameApp, ui: &mut egui::Ui) {
    ui.menu_button("Selection", |ui| {
        let two_selected = app.selection.selected_atom_indices.len() == 2;
        if ui
            .add_enabled(two_selected, egui::Button::new("Select Between"))
            .on_hover_text("Ctrl+B (need 2 atoms selected)")
            .clicked()
        {
            app.select_shortest_path(
                app.selection.selected_atom_indices[0],
                app.selection.selected_atom_indices[1],
            );
            ui.close();
        }
        let any_selected = !app.selection.selected_atom_indices.is_empty();
        if ui
            .add_enabled(any_selected, egui::Button::new("Change Resname"))
            .on_hover_text("Ctrl+R")
            .clicked()
        {
            app.open_resname_dialog();
            ui.close();
        }
        if ui.button("Clear Selection").clicked() {
            app.clear_selection();
            ui.close();
        }
    });
}

fn help_menu(ui: &mut egui::Ui) {
    ui.menu_button("Help", |ui| {
        ui.label(egui::RichText::new("Shortcuts").strong());
        ui.separator();
        for line in [
            "Ctrl+O   Open molecule",
            "Ctrl+T   Open TOP",
            "Ctrl+G   Open GRO",
            "Ctrl+Shift+O   TOP+GRO",
            "Ctrl+R   Edit resname",
            "Ctrl+S   Export",
            "Ctrl+B   Select path",
            "Ctrl+H   Toggle hbond",
            "Ctrl+Shift+A   Clear",
            "Ctrl+P   Command bar",
            "Ctrl+E   Export image",
        ] {
            ui.label(egui::RichText::new(line).color(theme::MUTED).size(12.0));
        }
        ui.separator();
        ui.label(
            egui::RichText::new("Type 'help' in the command bar for the\ncomponent syntax.")
                .color(theme::MUTED2)
                .size(11.5),
        );
    });
}

pub fn render_left_panel(app: &mut KuromameApp, ui: &mut egui::Ui) {
    let on_plumed = app.ui.left_tab == LeftPanelTab::Plumed && app.plumed_is_loaded();

    // The two tabs get two panel *ids*, because egui remembers a panel's
    // dragged width per id and a stored width always beats `default_size`.
    // That is what lets the PLUMED reading view open at half the window while
    // the structure tab keeps its narrow 264 -- and lets the user drag either
    // one without the other forgetting.
    let (id, default_size, min_size) = if on_plumed {
        let half = ui.ctx().content_rect().width() * 0.5;
        ("left_panel_plumed", half.max(360.0), 320.0)
    } else {
        ("left_panel", 264.0, 200.0)
    };

    egui::Panel::left(id)
        .resizable(true)
        .default_size(default_size)
        .min_size(min_size)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(16, 16)),
        )
        .show(ui, |ui| {
            if left_panel_tabs(app, ui) {
                ui.add_space(12.0);
            }

            if on_plumed {
                plumed_section(app, ui);
                return;
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                file_header(app, ui);
                ui.add_space(14.0);
                if loaded_files_section(app, ui) {
                    ui.add_space(14.0);
                }
                selection_section(app, ui);
                ui.add_space(14.0);
                components_section(app, ui);
            });
        });
}

/// The STRUCTURE / PLUMED tab strip. Only drawn once a script is open, so the
/// panel looks exactly as it always did until there is a second thing to show.
/// Returns whether anything was drawn.
fn left_panel_tabs(app: &mut KuromameApp, ui: &mut egui::Ui) -> bool {
    if !app.plumed_is_loaded() {
        return false;
    }

    let mut picked = None;
    ui.horizontal(|ui| {
        for (tab, label) in [
            (LeftPanelTab::Structure, "STRUCTURE"),
            (LeftPanelTab::Plumed, "PLUMED"),
        ] {
            let active = app.ui.left_tab == tab;
            let button = egui::Button::new(
                egui::RichText::new(label)
                    .size(11.0)
                    .strong()
                    .color(if active { theme::ACCENT_FG } else { theme::MUTED }),
            )
            .fill(if active { theme::ACCENT } else { theme::HOVER_BG })
            .stroke(egui::Stroke::new(
                1.0,
                if active { theme::ACCENT } else { theme::BORDER2 },
            ))
            .corner_radius(egui::CornerRadius::same(7))
            .min_size(egui::vec2(0.0, 26.0));

            if ui.add(button).clicked() {
                picked = Some(tab);
            }
        }

        let errors = app.plumed_error_count();
        if errors > 0 {
            ui.label(
                egui::RichText::new(format!("{errors} error{}", if errors == 1 { "" } else { "s" }))
                    .size(11.0)
                    .color(theme::AMBER)
                    .strong(),
            );
        }
    });

    if let Some(tab) = picked {
        app.ui.left_tab = tab;
    }
    true
}

/// Height kept below the line list for the detail box. Fixed rather than
/// measured so the list does not jump every time the selected line's
/// description changes length.
const PLUMED_DETAIL_HEIGHT: f32 = 150.0;

/// The PLUMED reading view: the script on the left of the window, the line the
/// user is standing on highlighted in the 3D view on the right.
fn plumed_section(app: &mut KuromameApp, ui: &mut egui::Ui) {
    plumed_header(app, ui);
    ui.add_space(10.0);
    plumed_controls(app, ui);
    ui.add_space(8.0);

    let list_height = (ui.available_height() - PLUMED_DETAIL_HEIGHT).max(120.0);
    plumed_line_list(app, ui, list_height);
    ui.add_space(8.0);
    plumed_detail(app, ui);
}

fn plumed_header(app: &KuromameApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        type_badge(ui, "PLUMED");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(app.plumed_file_name().unwrap_or_else(|| "plumed.dat".into()))
                .color(theme::TEXT)
                .size(13.0)
                .strong(),
        );
        count_badge(ui, app.plumed_action_count());
    });

    let position = match (app.plumed_selected(), app.plumed_action_count()) {
        (Some(idx), total) if total > 0 => format!("action {} of {total}", idx + 1),
        (_, total) => format!("{total} actions"),
    };
    ui.label(
        egui::RichText::new(position)
            .color(theme::MUTED2)
            .size(11.0),
    );
}

fn plumed_controls(app: &mut KuromameApp, ui: &mut egui::Ui) {
    let mut visible = app.plumed_visible();
    let mut arrows = app.plumed_show_arrows();
    let mut markers = app.plumed_show_markers();

    ui.horizontal(|ui| {
        if ui.checkbox(&mut visible, "Highlight").changed() {
            app.set_plumed_visible(visible);
        }
        if ui.checkbox(&mut arrows, "Vectors").changed() {
            app.set_plumed_show_arrows(arrows);
        }
        if ui.checkbox(&mut markers, "Centres").changed() {
            app.set_plumed_show_markers(markers);
        }
    });

    ui.add_space(4.0);
    // Buttons as well as the arrow keys: the 3D view takes keyboard focus as
    // soon as the pointer is over it, and stepping the script is the whole
    // point of this panel.
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 8.0) * 0.5;
        if ui
            .add_sized(
                egui::vec2(width, 28.0),
                egui::Button::new(egui::RichText::new("◀ Prev").color(theme::TEXT).size(12.5))
                    .fill(theme::HOVER_BG)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER2))
                    .corner_radius(egui::CornerRadius::same(7)),
            )
            .on_hover_text("Previous action (Up arrow)")
            .clicked()
        {
            app.step_plumed_selection(-1);
        }
        if ui
            .add_sized(
                egui::vec2(width, 28.0),
                egui::Button::new(egui::RichText::new("Next ▶").color(theme::TEXT).size(12.5))
                    .fill(theme::HOVER_BG)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER2))
                    .corner_radius(egui::CornerRadius::same(7)),
            )
            .on_hover_text("Next action (Down arrow)")
            .clicked()
        {
            app.step_plumed_selection(1);
        }
    });
}

/// The script, one row per physical line. A `...` block is several rows that
/// all select the same action.
fn plumed_line_list(app: &mut KuromameApp, ui: &mut egui::Ui, height: f32) {
    let owners = app.plumed_line_owners();
    let selected = app.plumed_selected();
    let scroll_to = app.take_plumed_scroll_request();
    let gutter = gutter_width(app.plumed_lines().len());

    let mut picked: Option<usize> = None;

    egui::Frame::new()
        .fill(theme::INPUT_BG)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(2, 6))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("plumed_lines")
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;

                    let width = ui.available_width();
                    for (index, line) in app.plumed_lines().iter().enumerate() {
                        let owner = owners.get(index).copied().flatten();
                        let is_selected = owner.is_some() && owner == selected;
                        let is_error = owner.is_some_and(|idx| app.plumed_action_has_error(idx));

                        // Sized to its content, not to a fixed row height: a
                        // `CENTER` over a whole rosette is a 700-character
                        // ATOMS list, and wrapping it is what makes the panel
                        // readable at half the window instead of needing a
                        // horizontal scrollbar nobody would find.
                        let response = ui.add(
                            egui::Button::new(plumed_line_text(line, index + 1, gutter, width))
                                .min_size(egui::vec2(width, 16.0))
                                .fill(if is_selected {
                                    theme::HOVER_BG
                                } else {
                                    egui::Color32::TRANSPARENT
                                })
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(egui::CornerRadius::ZERO),
                        );

                        // A bar in the left margin, rather than a tinted row:
                        // it marks selection and error without fighting the
                        // syntax colours for the text itself.
                        let bar = if is_error {
                            Some(egui::Color32::from_rgb(0xe5, 0x53, 0x4b))
                        } else if is_selected {
                            Some(theme::ACCENT)
                        } else {
                            None
                        };
                        if let Some(color) = bar {
                            let rect = response.rect;
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    rect.left_top(),
                                    egui::vec2(2.5, rect.height()),
                                ),
                                egui::CornerRadius::ZERO,
                                color,
                            );
                        }

                        if response.clicked()
                            && let Some(idx) = owner
                        {
                            picked = Some(idx);
                        }
                        if is_selected && scroll_to {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                });
        });

    if let Some(idx) = picked {
        app.set_plumed_selected(Some(idx));
    }
}

/// What the selected line means, in words.
fn plumed_detail(app: &KuromameApp, ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(theme::CARD_BG)
        .stroke(egui::Stroke::new(1.0, theme::CARD_BORDER))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(detail) = app.plumed_selected_detail() else {
                ui.label(
                    egui::RichText::new("Pick a line to see what it sets up")
                        .color(theme::MUTED2)
                        .size(12.0),
                );
                return;
            };

            egui::ScrollArea::vertical()
                .id_salt("plumed_detail")
                .max_height(PLUMED_DETAIL_HEIGHT - 20.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        type_badge(ui, &detail.name);
                        ui.add_space(4.0);
                        if let Some(label) = &detail.label {
                            ui.label(
                                egui::RichText::new(label)
                                    .monospace()
                                    .size(12.0)
                                    .color(theme::ACCENT),
                            );
                        }
                        ui.label(
                            egui::RichText::new(format!("line {}", detail.line))
                                .size(11.0)
                                .color(theme::MUTED2),
                        );
                    });
                    ui.add_space(4.0);

                    ui.label(
                        egui::RichText::new(&detail.summary)
                            .color(theme::TEXT)
                            .size(12.0),
                    );

                    if let Some(error) = &detail.error {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(error)
                                .color(egui::Color32::from_rgb(0xff, 0x7b, 0x72))
                                .size(12.0),
                        );
                    } else if detail.atom_count + detail.marker_count + detail.arrow_count == 0 {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(if app.atom_count() == 0 {
                                "Load a structure to resolve this line's atoms"
                            } else {
                                "Nothing to draw for this line"
                            })
                            .color(theme::MUTED2)
                            .size(11.5),
                        );
                    } else {
                        ui.add_space(4.0);
                        let drawn = format!(
                            "drawing {} atom{}, {} centre{}, {} vector{}{}",
                            detail.atom_count,
                            plural(detail.atom_count),
                            detail.marker_count,
                            plural(detail.marker_count),
                            detail.arrow_count,
                            plural(detail.arrow_count),
                            if detail.indirect {
                                " (inherited through ARG=)"
                            } else {
                                ""
                            },
                        );
                        ui.label(
                            egui::RichText::new(drawn)
                                .color(theme::MUTED2)
                                .size(11.5),
                        );
                    }
                });
        });
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Digits reserved for the line-number gutter, so the code all starts in the
/// same column.
fn gutter_width(lines: usize) -> usize {
    lines.max(1).to_string().len()
}

/// One source line, syntax-coloured: comments grey, the label blue, the action
/// name bright, keys amber and their values plain.
fn plumed_line_text(
    line: &str,
    number: usize,
    gutter: usize,
    width: f32,
) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};

    let font = egui::FontId::monospace(11.5);
    let mut job = LayoutJob::default();
    // Wrapped at the row width rather than clipped: the point of the panel is
    // to read the atom lists, and a clipped `ATOMS=` hides exactly the numbers
    // the user came to check. One source line is still one clickable row.
    job.wrap.max_width = width;
    // Those lists are one long comma-separated token, so breaking only at
    // whitespace would not wrap them at all.
    job.wrap.break_anywhere = true;

    let mut push = |text: &str, color: egui::Color32| {
        job.append(text, 0.0, TextFormat::simple(font.clone(), color));
    };

    push(&format!("{number:>gutter$} │ "), theme::MUTED2);

    // Everything from an unbraced `#` is a comment, whole-line or trailing.
    let (code, comment) = split_comment(line);

    let mut seen_action = false;
    for (token, is_space) in tokens_with_gaps(code) {
        if is_space {
            push(token, theme::MUTED2);
            continue;
        }

        if token == "..." {
            push(token, theme::MUTED2);
        } else if !seen_action && token.ends_with(':') {
            push(token, theme::ACCENT);
        } else if !seen_action && !token.contains('=') {
            seen_action = true;
            push(token, theme::TEXT);
        } else if let Some((key, value)) = token.split_once('=') {
            seen_action = true;
            push(key, theme::AMBER);
            push("=", theme::MUTED2);
            push(value, theme::MUTED);
        } else {
            push(token, theme::MUTED);
        }
    }

    if !comment.is_empty() {
        push(comment, theme::MUTED2);
    }

    job
}

/// Split a source line into its code and its trailing comment. `#` inside
/// braces is part of a `FUNC={...}`, not a comment -- the same rule the parser
/// applies.
fn split_comment(line: &str) -> (&str, &str) {
    let mut depth = 0i32;
    for (idx, ch) in line.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            '#' if depth <= 0 => return line.split_at(idx),
            _ => {}
        }
    }
    (line, "")
}

/// Walk a line as alternating whitespace and non-whitespace runs, so the
/// original spacing survives into the coloured output.
fn tokens_with_gaps(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_space = text.starts_with(char::is_whitespace);

    for (idx, ch) in text.char_indices() {
        let is_space = ch.is_whitespace();
        if is_space != in_space {
            out.push((&text[start..idx], in_space));
            start = idx;
            in_space = is_space;
        }
    }
    if start < text.len() {
        out.push((&text[start..], in_space));
    }
    out
}

fn file_header(app: &mut KuromameApp, ui: &mut egui::Ui) {
    // Hero: the active layer's structure file name (falls back to the layer
    // name when nothing is loaded), then a format badge + atom count row.
    let title = app
        .structure_file_name()
        .unwrap_or_else(|| app.active_layer_name());
    ui.label(
        egui::RichText::new(title)
            .color(theme::TEXT)
            .size(14.0)
            .strong(),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if let Some(badge) = app.structure_badge() {
            type_badge(ui, badge);
            ui.add_space(2.0);
        }
        let atoms = app.atom_count();
        let detail = if atoms > 0 {
            format!("{atoms} atoms")
        } else {
            "no file loaded".to_string()
        };
        ui.label(
            egui::RichText::new(detail)
                .color(theme::MUTED2)
                .size(12.0),
        );
    });
}

/// Lists every file loaded into the active layer besides the primary structure
/// (which is the header hero) — topology, index, trajectory, dot surface and
/// Martini force field — each as a badge row, so the whole loaded state is
/// visible at a glance. Returns whether anything was drawn.
fn loaded_files_section(app: &mut KuromameApp, ui: &mut egui::Ui) -> bool {
    let aux: Vec<_> = app
        .loaded_files()
        .into_iter()
        .filter(|r| !matches!(r.badge, "GRO" | "PDB"))
        .collect();
    if aux.is_empty() {
        return false;
    }
    section_label(ui, "LOADED FILES");
    ui.add_space(6.0);
    for row in aux {
        ui.horizontal(|ui| {
            type_badge(ui, row.badge);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(
                    egui::RichText::new(&row.name)
                        .color(theme::TEXT)
                        .size(12.5),
                );
                ui.label(
                    egui::RichText::new(&row.detail)
                        .color(theme::MUTED2)
                        .size(11.0),
                );
            });
        });
        ui.add_space(6.0);
    }
    true
}

fn selection_section(app: &mut KuromameApp, ui: &mut egui::Ui) {
    section_label(ui, "SELECTION");
    ui.add_space(6.0);

    let input = ui.add_sized(
        egui::vec2(ui.available_width(), 32.0),
        egui::TextEdit::singleline(&mut app.ui.selector_input).hint_text("aC1 | aC2"),
    );
    let apply_by_enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.add_space(6.0);

    // Apply (accent) + -> Text.
    ui.columns(2, |cols| {
        let apply = cols[0].add_sized(
            egui::vec2(cols[0].available_width(), 30.0),
            egui::Button::new(
                egui::RichText::new("Apply")
                    .color(theme::ACCENT_FG)
                    .size(12.5)
                    .strong(),
            )
            .fill(theme::ACCENT)
            .corner_radius(egui::CornerRadius::same(7)),
        );
        if apply.clicked() || apply_by_enter {
            app.apply_selector_expression();
        }
        if secondary_button(&mut cols[1], "→ Text".to_string(), true).clicked() {
            if let Some(selector_text) = app.update_selector_input_from_selection() {
                cols[1].ctx().copy_text(selector_text);
                app.ui.status_msg = "Selection exported to selector text and copied".to_string();
            } else {
                app.ui.status_msg = "No selected atoms with usable atom names".to_string();
            }
        }
    });

    ui.add_space(6.0);

    // Between / Resname / Clear.
    let two_selected = app.selection.selected_atom_indices.len() == 2;
    let any_selected = !app.selection.selected_atom_indices.is_empty();
    let mut do_between = false;
    let mut do_resname = false;
    let mut do_clear = false;
    ui.columns(3, |cols| {
        do_between = secondary_button(&mut cols[0], "Between".to_string(), two_selected)
            .on_hover_text("Need 2 atoms selected")
            .clicked();
        do_resname = secondary_button(&mut cols[1], "Resname".to_string(), any_selected).clicked();
        do_clear = secondary_button(&mut cols[2], "Clear".to_string(), true).clicked();
    });
    if do_between {
        app.select_shortest_path(
            app.selection.selected_atom_indices[0],
            app.selection.selected_atom_indices[1],
        );
    }
    if do_resname {
        app.open_resname_dialog();
    }
    if do_clear {
        app.clear_selection();
    }

    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(format!(
            "Selected: {}",
            app.selection.selected_atom_indices.len()
        ))
        .color(theme::MUTED)
        .size(12.0),
    );
}

fn components_section(app: &mut KuromameApp, ui: &mut egui::Ui) {
    section_label(ui, "COMPONENTS");
    ui.add_space(6.0);

    if app.has_components() {
        ui.horizontal(|ui| {
            if secondary_button(ui, "Show all".to_string(), true).clicked() {
                app.set_all_components_visible(true);
            }
            if secondary_button(ui, "Hide all".to_string(), true).clicked() {
                app.set_all_components_visible(false);
            }
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .id_salt("res_visibility_scroll")
            .max_height(260.0)
            .show(ui, |ui| {
                let rows = app.component_list();
                let mut toggles: Vec<(String, bool)> = Vec::new();
                for (name, visible, count) in &rows {
                    let label = if name.is_empty() {
                        "(no residue)".to_string()
                    } else {
                        name.clone()
                    };
                    let icon = if *visible {
                        MaterialIcon::Visibility
                    } else {
                        MaterialIcon::VisibilityOff
                    };
                    let text_col = if *visible { theme::TEXT } else { theme::MUTED2 };
                    let text = egui::RichText::new(format!("{}  {}", mi(icon), label))
                        .color(text_col)
                        .size(13.0);
                    let resp = ui.add_sized(
                        egui::vec2(ui.available_width(), 28.0),
                        egui::Button::new(text)
                            .fill(egui::Color32::TRANSPARENT)
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(egui::CornerRadius::same(7)),
                    );
                    // The atom count is painted right-aligned inside the row's
                    // own rect rather than added as a second widget, so it does
                    // not steal clicks from the toggle button underneath.
                    ui.painter().text(
                        resp.rect.right_center() - egui::vec2(10.0, 0.0),
                        egui::Align2::RIGHT_CENTER,
                        count.to_string(),
                        egui::FontId::proportional(11.0),
                        theme::MUTED2,
                    );
                    if resp.clicked() {
                        toggles.push((name.clone(), !visible));
                    }
                }
                for (name, vis) in toggles {
                    app.set_component_visible(&name, vis);
                }
            });
    } else {
        ui.label(
            egui::RichText::new("Load a structure to list components")
                .color(theme::MUTED2)
                .size(12.0),
        );
    }

    // Dot-surface block (only when the loaded PDB carries a "DOT" surface).
    if app.has_surface() {
        ui.add_space(12.0);
        section_label(ui, "SURFACE");
        ui.add_space(6.0);

        let mut surface_visible = app.surface_visible();
        if ui
            .checkbox(&mut surface_visible, "Show dot surface")
            .changed()
        {
            app.set_surface_visible(surface_visible);
        }
        ui.label(
            egui::RichText::new(format!("{} dots", app.surface_dot_count()))
                .color(theme::MUTED2)
                .size(12.0),
        );
    }

    // NDX group block (only when an NDX file with groups is loaded). Every group
    // can be shown at once, each in its own editable colour; the swatch sets the
    // colour and the row toggles that group on its own, while "Show NDX groups"
    // hides the lot without disturbing the per-group state.
    if app.ndx_group_count() > 0 {
        ui.add_space(12.0);
        section_label(ui, "NDX GROUPS");
        ui.add_space(6.0);

        let mut ndx_visible = app.ndx_visible();
        if ui.checkbox(&mut ndx_visible, "Show NDX groups").changed() {
            app.set_ndx_visible(ndx_visible);
        }
        ui.add_space(6.0);

        // Alpha of the highlight spheres only. The structure behind them keeps
        // the layer's own OPACITY slider, so the two fade independently.
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("OPACITY")
                    .size(9.5)
                    .color(theme::MUTED2)
                    .strong(),
            );
            let mut op = app.ndx_opacity();
            ui.spacing_mut().slider_width = (ui.available_width() - 44.0).max(60.0);
            let resp = ui.add(egui::Slider::new(&mut op, 0.0..=1.0).show_value(false));
            if resp.changed() {
                app.set_ndx_opacity(op);
            }
            ui.label(
                egui::RichText::new(format!("{}%", (op * 100.0).round() as i32))
                    .size(11.0)
                    .color(theme::MUTED),
            );
        })
        .response
        .on_hover_text("Transparency of the NDX colouring, independent of the structure's opacity");
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            if secondary_button(ui, "Show all".to_string(), true).clicked() {
                app.set_all_ndx_groups_enabled(true);
            }
            if secondary_button(ui, "Hide all".to_string(), true).clicked() {
                app.set_all_ndx_groups_enabled(false);
            }
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .id_salt("ndx_group_scroll")
            .max_height(200.0)
            .show(ui, |ui| {
                let options = app.ndx_group_options();
                let mut toggles: Vec<(usize, bool)> = Vec::new();
                let mut recolors: Vec<(usize, [f32; 3])> = Vec::new();
                for (idx, label) in options.iter().enumerate() {
                    let enabled = app.ndx_group_enabled(idx);
                    ui.horizontal(|ui| {
                        let mut color = app.ndx_group_color(idx);
                        if ui.color_edit_button_rgb(&mut color).changed() {
                            recolors.push((idx, color));
                        }
                        let icon = if enabled {
                            MaterialIcon::Visibility
                        } else {
                            MaterialIcon::VisibilityOff
                        };
                        let text_col = if enabled { theme::TEXT } else { theme::MUTED2 };
                        let text = egui::RichText::new(format!("{}  {}", mi(icon), label))
                            .color(text_col)
                            .size(13.0);
                        let resp = ui.add_sized(
                            egui::vec2(ui.available_width(), 28.0),
                            egui::Button::new(text)
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(egui::CornerRadius::same(7)),
                        );
                        if resp.clicked() {
                            toggles.push((idx, !enabled));
                        }
                    });
                }
                for (idx, color) in recolors {
                    app.set_ndx_group_color(idx, color);
                }
                for (idx, enabled) in toggles {
                    app.set_ndx_group_enabled(idx, enabled);
                }
            });

        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!("{} atoms rendered", app.ndx_selected_atom_count()))
                .color(theme::MUTED2)
                .size(12.0),
        );
    }
}

/// Right-side panel: the LAYERS list (switch the active structure, toggle each
/// layer's sphere overlay, add/remove) plus the dot-surface overlays below it.
pub fn render_overlay_panel(app: &mut KuromameApp, ui: &mut egui::Ui) {
    egui::Panel::right("overlay_panel")
        .resizable(true)
        .default_size(240.0)
        .min_size(190.0)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(16, 16)),
        )
        .show(ui, |ui| {
            render_layers_section(app, ui);

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(12.0);

            ui.horizontal(|ui| {
                section_label(ui, "OVERLAY SURFACES");
                count_badge(ui, app.overlay_count());
            });
            ui.add_space(6.0);

            if secondary_button(
                ui,
                format!("{}  Add surface…", mi(MaterialIcon::Layers)),
                true,
            )
            .on_hover_text("Load a PDB dot surface as a new overlay layer")
            .clicked()
            {
                app.open_overlay_surface_file();
            }
            ui.add_space(8.0);

            let count = app.overlay_count();
            if count == 0 {
                ui.label(
                    egui::RichText::new("No overlay surfaces.\nAdd PDBs with a DOT surface to compare them over the base structure.")
                        .color(theme::MUTED2)
                        .size(12.0),
                );
            } else {
                // Tab bar: one selectable chip per overlay surface.
                let names = app.overlay_names();
                let mut active = app.active_overlay_index();
                egui::ScrollArea::horizontal()
                    .id_salt("overlay_tabs_scroll")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (i, name) in names.iter().enumerate() {
                                let label = format!("{}. {}", i + 1, tab_short_name(name));
                                if ui.selectable_label(i == active, label).clicked() {
                                    active = i;
                                }
                            }
                        });
                    });
                if active != app.active_overlay_index() {
                    app.set_active_overlay(active);
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                let idx = app.active_overlay_index();
                if let Some(name) = app.overlay_name(idx) {
                    ui.label(egui::RichText::new(name).color(theme::TEXT).size(13.0).strong());
                    ui.label(
                        egui::RichText::new(format!("{} dots", app.overlay_dot_count(idx)))
                            .color(theme::MUTED2)
                            .size(12.0),
                    );
                    ui.add_space(8.0);

                    let mut visible = app.overlay_visible(idx);
                    if ui.checkbox(&mut visible, "Show").changed() {
                        app.set_overlay_visible(idx, visible);
                    }
                    ui.add_space(6.0);

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Color").color(theme::TEXT).size(12.5));
                        let mut color = app.overlay_color(idx);
                        if ui.color_edit_button_rgb(&mut color).changed() {
                            app.set_overlay_color(idx, color);
                        }
                    });
                    ui.add_space(12.0);

                    if secondary_button(ui, format!("{}  Remove", mi(MaterialIcon::Delete)), true)
                        .clicked()
                    {
                        app.remove_overlay(idx);
                    }
                }
            }
        });
}

/// LAYERS list: each row selects the active layer (drawn as the full main
/// molecule) and toggles that layer's sphere overlay when it is not active.
/// Only one layer is the main molecule at a time; the rest render as spheres.
fn render_layers_section(app: &mut KuromameApp, ui: &mut egui::Ui) {
    let count = app.layer_count();
    let active = app.active_layer_index();
    let names = app.layer_names();
    // Precompute so the scroll closure needs no borrow of `app`.
    let atom_counts: Vec<usize> = (0..count).map(|i| app.layer_atom_count(i)).collect();
    let visibles: Vec<bool> = (0..count).map(|i| app.layer_visible(i)).collect();
    let colors: Vec<egui::Color32> = (0..count).map(|i| rgb_to_color32(app.layer_color(i))).collect();
    let opacities: Vec<f32> = (0..count).map(|i| app.layer_opacity(i)).collect();

    // Header: LAYERS · count · Add layer.
    let mut do_add = false;
    ui.horizontal(|ui| {
        section_label(ui, "LAYERS");
        count_badge(ui, count);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            do_add = ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(format!("{} Add layer", mi(MaterialIcon::Add)))
                            .color(theme::ACCENT)
                            .size(12.0),
                    )
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE),
                )
                .on_hover_text("Add a new layer and load a structure into it")
                .clicked();
        });
    });
    ui.add_space(8.0);

    let mut make_active: Option<usize> = None;
    let mut toggle_vis: Option<(usize, bool)> = None;
    let mut remove: Option<usize> = None;
    let mut set_opacity: Option<(usize, f32)> = None;

    egui::ScrollArea::vertical()
        .id_salt("layers_scroll")
        .max_height(320.0)
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let is_active = i == active;
                let border = if is_active {
                    theme::ACCENT
                } else {
                    theme::CARD_BORDER
                };
                let card = egui::Frame::new()
                    .fill(theme::CARD_BG)
                    .stroke(egui::Stroke::new(1.0, border))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(12, 11))
                    .outer_margin(egui::Margin {
                        bottom: 8,
                        ..egui::Margin::ZERO
                    });

                // Returns (visibility toggle, remove requested, opacity change)
                // so the whole-card activation click below can ignore inner hits.
                let out = card.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut toggle: Option<bool> = None;
                    let mut remove_req = false;
                    let mut set_op: Option<f32> = None;

                    ui.horizontal(|ui| {
                        // Eye = visibility. The active layer is always drawn as
                        // the main molecule, so its eye is a disabled indicator.
                        let icon = if visibles[i] {
                            MaterialIcon::Visibility
                        } else {
                            MaterialIcon::VisibilityOff
                        };
                        let eye_col = if visibles[i] { colors[i] } else { theme::MUTED2 };
                        let eye = ui.add_enabled(
                            !is_active,
                            egui::Button::new(egui::RichText::new(mi(icon)).color(eye_col).size(15.0))
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::NONE),
                        );
                        if eye.on_hover_text("Show as spheres while not active").clicked() {
                            toggle = Some(!visibles[i]);
                        }

                        color_swatch(ui, colors[i]);
                        ui.add_space(3.0);

                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            let name_col = if visibles[i] { theme::TEXT } else { theme::MUTED2 };
                            ui.label(
                                egui::RichText::new(tab_short_name(name))
                                    .color(name_col)
                                    .size(13.0)
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new(format!("{} atoms", atom_counts[i]))
                                    .color(theme::MUTED2)
                                    .size(11.0),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if is_active {
                                ui.label(
                                    egui::RichText::new("active")
                                        .color(theme::ACCENT)
                                        .size(10.5)
                                        .strong(),
                                );
                            } else if ui
                                .add_enabled(
                                    count > 1,
                                    egui::Button::new(
                                        egui::RichText::new(mi(MaterialIcon::Delete))
                                            .color(theme::MUTED2)
                                            .size(15.0),
                                    )
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::NONE),
                                )
                                .on_hover_text("Remove layer")
                                .clicked()
                            {
                                remove_req = true;
                            }
                        });
                    });

                    // Opacity: fades the main molecule for the active layer, or
                    // the sphere overlay for the others. The NDX highlight has
                    // its own slider and is not affected by this one.
                    ui.add_space(9.0);
                    let op_row = ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("OPACITY")
                                .size(9.5)
                                .color(theme::MUTED2)
                                .strong(),
                        );
                        let mut op = opacities[i];
                        ui.spacing_mut().slider_width = (ui.available_width() - 44.0).max(60.0);
                        let resp =
                            ui.add(egui::Slider::new(&mut op, 0.0..=1.0).show_value(false));
                        if resp.changed() {
                            set_op = Some(op);
                        }
                        ui.label(
                            egui::RichText::new(format!("{}%", (op * 100.0).round() as i32))
                                .size(11.0)
                                .color(theme::MUTED),
                        );
                    });
                    op_row
                        .response
                        .on_hover_text("Structure opacity — NDX colouring has its own slider");

                    (toggle, remove_req, set_op)
                });

                let (toggle, remove_req, set_op) = out.inner;
                if let Some(v) = toggle {
                    toggle_vis = Some((i, v));
                }
                if remove_req {
                    remove = Some(i);
                }
                if let Some(op) = set_op {
                    set_opacity = Some((i, op));
                }
                // Click anywhere else on a non-active card to make it active.
                let card_clicked = out.response.interact(egui::Sense::click()).clicked();
                if card_clicked
                    && toggle.is_none()
                    && !remove_req
                    && set_op.is_none()
                    && !is_active
                {
                    make_active = Some(i);
                }
            }
        });

    if do_add {
        app.add_layer();
    }
    if let Some(i) = make_active {
        app.set_active_layer(i);
    }
    if let Some((i, v)) = toggle_vis {
        app.set_layer_visible(i, v);
    }
    if let Some(i) = remove {
        app.remove_layer(i);
    }
    if let Some((i, op)) = set_opacity {
        app.set_layer_opacity(i, op);
    }
}

/// Trim an overlay's file name so it fits on a tab chip.
fn tab_short_name(name: &str) -> String {
    let stem = name.strip_suffix(".pdb").unwrap_or(name);
    let stem = stem.strip_suffix(".ent").unwrap_or(stem);
    let stem = stem.strip_suffix(".gro").unwrap_or(stem);
    if stem.chars().count() > 12 {
        let short: String = stem.chars().take(11).collect();
        format!("{short}…")
    } else {
        stem.to_string()
    }
}

/// Bottom dock: render-style segmented control + trajectory transport.
/// Per-axis periodic replication spinners.
///
/// Only shown once a simulation box is loaded, since there is nothing to
/// replicate along without one. Each number is how many cells to draw along
/// that axis *in total*, so every block size is reachable — 2×2×2 as much as
/// 3×3×3.
fn periodic_controls(app: &mut KuromameApp, ui: &mut egui::Ui) {
    if !app.has_simulation_cell() {
        return;
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(10.0);

    let mut cells = app.periodic_cells();

    ui.label("PBC")
        .on_hover_text("Repeat the simulation cell along each cell vector");

    let mut changed = false;
    for (axis, label) in ["a", "b", "c"].iter().enumerate() {
        changed |= ui
            .add(
                egui::DragValue::new(&mut cells[axis])
                    .speed(0.05)
                    .range(1..=9)
                    .prefix(format!("{label} ×")),
            )
            .on_hover_text(
                "Cells drawn along this vector, this one included. \
                 The geometry is not duplicated — each extra cell is one more draw.",
            )
            .changed();
    }

    let total: usize = cells.iter().map(|c| *c as usize).product();
    if total > 1 {
        ui.weak(format!(
            "{}×{}×{} = {total} cells",
            cells[0], cells[1], cells[2]
        ));
    }

    if changed {
        app.set_periodic_cells(cells);
    }
}

pub fn render_bottom_dock(app: &mut KuromameApp, ui: &mut egui::Ui) {
    egui::Panel::bottom("bottom_dock")
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(16, 10)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                section_label(ui, "STYLE");
                style_segment(app, ui);

                if app.has_martini_ff() {
                    ui.add_space(10.0);
                    let mut beads = app.martini_visible();
                    if ui
                        .checkbox(&mut beads, "Martini beads")
                        .on_hover_text("Draw coarse-grained beads sized/coloured by bead type")
                        .changed()
                    {
                        app.set_martini_visible(beads);
                    }
                }

                ui.add_space(10.0);
                let mut axes = app.axis_visible();
                if ui
                    .checkbox(&mut axes, "Axes")
                    .on_hover_text("Show the XYZ orientation triad at the box origin (X red, Y green, Z blue)")
                    .changed()
                {
                    app.set_axis_visible(axes);
                }

                periodic_controls(app, ui);

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                if app.trajectory_frame_count() > 0 {
                    trajectory_controls(app, ui);
                } else {
                    ui.label(
                        egui::RichText::new("No trajectory — open an XTC to enable playback")
                            .color(theme::MUTED2)
                            .size(12.0),
                    );
                }
            });
        });
}

fn style_segment(app: &mut KuromameApp, ui: &mut egui::Ui) {
    let current = app.viewport.render_style();
    let options = [
        (RenderStyle::BallStick, "B+S"),
        (RenderStyle::BallOnly, "Ball"),
        (RenderStyle::Wireframe, "Wire"),
        (RenderStyle::Circles, "Circ"),
    ];
    egui::Frame::new()
        .fill(theme::HOVER_BG)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (style, short) in options {
                    let active = current == style;
                    let (fill, fg) = if active {
                        (theme::ACCENT, theme::ACCENT_FG)
                    } else {
                        (egui::Color32::TRANSPARENT, theme::MUTED)
                    };
                    let btn = egui::Button::new(
                        egui::RichText::new(short).color(fg).size(11.0).strong(),
                    )
                    .fill(fill)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(egui::CornerRadius::same(6))
                    .min_size(egui::vec2(48.0, 24.0));
                    if ui.add(btn).clicked() {
                        app.viewport.set_render_style(style);
                    }
                }
            });
        });
}

fn trajectory_controls(app: &mut KuromameApp, ui: &mut egui::Ui) {
    if ui.button("|<").on_hover_text("First frame").clicked() {
        app.go_to_first_frame();
    }
    if ui.button("<").on_hover_text("Previous frame").clicked() {
        app.step_frame(-1);
    }
    let glyph = if app.trajectory_is_playing() { "⏸" } else { "▶" };
    let play = egui::Button::new(
        egui::RichText::new(glyph)
            .color(theme::ACCENT_FG)
            .size(13.0),
    )
    .fill(theme::ACCENT)
    .corner_radius(egui::CornerRadius::same(17));
    if ui
        .add_sized(egui::vec2(34.0, 30.0), play)
        .on_hover_text("Play / Pause")
        .clicked()
    {
        app.toggle_playback();
    }
    if ui.button(">").on_hover_text("Next frame").clicked() {
        app.step_frame(1);
    }
    if ui.button(">|").on_hover_text("Last frame").clicked() {
        app.go_to_last_frame();
    }

    let count = app.trajectory_frame_count();
    let max_frame = count.saturating_sub(1);
    let labels = format!(
        "Frame {} / {}    {:.2} ps",
        app.trajectory_current_frame() + 1,
        count,
        app.trajectory_current_time()
    );

    // Right-aligned smoothing + FPS + frame/time, slider fills the middle.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add(
            egui::DragValue::new(app.trajectory_interp_steps())
                .range(1..=20)
                .speed(0.1),
        )
        .on_hover_text(
            "Smoothing: number of linearly-interpolated frames per step (1 = off)",
        );
        ui.label(
            egui::RichText::new("Smooth")
                .color(theme::MUTED2)
                .size(11.0),
        );
        ui.add(
            egui::DragValue::new(app.trajectory_playback_fps())
                .range(0.1..=200.0)
                .speed(0.5),
        );
        ui.label(egui::RichText::new("FPS").color(theme::MUTED2).size(11.0));
        ui.label(
            egui::RichText::new(labels)
                .color(theme::MUTED2)
                .size(11.0),
        );

        let mut frame_idx = app.trajectory_current_frame();
        ui.spacing_mut().slider_width = (ui.available_width() - 20.0).max(80.0);
        let slider = egui::Slider::new(&mut frame_idx, 0..=max_frame).show_value(false);
        if ui.add(slider).changed() {
            app.set_trajectory_frame(frame_idx);
        }
    });
}

/// The shared command bar, stacked directly above the status bar.
///
/// It carries the COMPONENTS language today, but nothing about the bar is
/// component-specific — later verbs (layers, NDX, export) belong here too.
///
/// The log above the input exists because `status_msg` is one overwriting
/// `String`: it cannot show a parse error's caret line, and one expression can
/// emit several name-resolution notes that would each clobber the last.
pub fn render_command_bar(app: &mut KuromameApp, ui: &mut egui::Ui) {
    egui::Panel::bottom("command_bar")
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(16, 6)),
        )
        .show(ui, |ui| {
            let mut toggle_expand = false;
            let mut clear_log = false;

            if !app.command_log().is_empty() {
                let expanded = app.command_log_expanded();
                let max_height = if expanded { 240.0 } else { 68.0 };
                egui::ScrollArea::vertical()
                    .id_salt("command_log_scroll")
                    .max_height(max_height)
                    .auto_shrink([false, true])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        for entry in app.command_log() {
                            let color = match entry.level {
                                crate::app::LogLevel::Info => theme::MUTED,
                                crate::app::LogLevel::Warn => theme::AMBER,
                                crate::app::LogLevel::Error => theme::AMBER,
                            };
                            // Monospace: the parse-error caret line only lines
                            // up under its token in a fixed-width font.
                            ui.label(
                                egui::RichText::new(&entry.text)
                                    .monospace()
                                    .size(11.5)
                                    .color(color),
                            );
                        }
                    });
                ui.add_space(4.0);
            }

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(">")
                        .monospace()
                        .size(13.0)
                        .color(theme::ACCENT)
                        .strong(),
                );

                // Leave room for the two trailing buttons.
                let field_width = (ui.available_width() - 96.0).max(120.0);
                let input = ui.add_sized(
                    egui::vec2(field_width, 26.0),
                    egui::TextEdit::singleline(app.command_input_mut())
                        .font(egui::TextStyle::Monospace)
                        .hint_text("DOM1 = PROT and resid 1-100        (help)"),
                );

                // Ctrl+P, handled in `handle_keyboard_shortcuts`, parks a
                // one-frame request here — the same trick the resname dialog
                // uses, since nothing else in the app grabs focus on its own.
                if app.take_command_focus_request() {
                    input.request_focus();
                }

                if input.has_focus() {
                    // Consume the arrow keys so the text field does not also act
                    // on them, then move through the history.
                    let up = ui.input_mut(|i| {
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                    });
                    let down = ui.input_mut(|i| {
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                    });
                    if up || down {
                        app.recall_history(if up { -1 } else { 1 });
                        // Recall replaces the buffer behind the widget's back, so
                        // park the caret at the end instead of leaving it at 0.
                        let end = app.command_input().chars().count();
                        if let Some(mut state) =
                            egui::TextEdit::load_state(ui.ctx(), input.id)
                        {
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                                egui::text::CCursor::new(end),
                            )));
                            state.store(ui.ctx(), input.id);
                        }
                    }
                }

                let submitted =
                    input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if submitted {
                    let line = std::mem::take(app.command_input_mut());
                    app.run_command(&line);
                    // Keep focus so a run of commands can be typed without
                    // reaching for the mouse between them. This has to go
                    // through the same one-frame request Ctrl+P uses:
                    // `Response::request_focus` here loses to egui's own
                    // end-of-frame focus surrender on Enter, leaving the bar
                    // dead until clicked.
                    app.request_command_focus();
                }

                let icon = if app.command_log_expanded() {
                    MaterialIcon::ExpandMore
                } else {
                    MaterialIcon::ExpandLess
                };
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(mi(icon)).size(14.0).color(theme::MUTED),
                        )
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE),
                    )
                    .on_hover_text("Expand / collapse the command log")
                    .clicked()
                {
                    toggle_expand = true;
                }
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(mi(MaterialIcon::Delete))
                                .size(14.0)
                                .color(theme::MUTED),
                        )
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE),
                    )
                    .on_hover_text("Clear the command log")
                    .clicked()
                {
                    clear_log = true;
                }
            });

            if toggle_expand {
                app.toggle_command_log_expanded();
            }
            if clear_log {
                app.clear_command_log();
            }
        });
}

pub fn render_bottom_status_bar(app: &mut KuromameApp, ui: &mut egui::Ui) {
    egui::Panel::bottom("status_bar")
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(16, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.selection.with_hbond_chk, "Select with hbond");
                ui.separator();
                ui.label(
                    egui::RichText::new(&app.ui.hovered_atom_info)
                        .color(theme::MUTED)
                        .size(12.0),
                );
                // While a file is loading off-thread, show a progress bar (byte
                // fraction when known, an animated indeterminate bar otherwise —
                // e.g. a topology whose size we cannot total across #includes), a
                // short stage label, and a Cancel button.
                if app.load_in_progress() {
                    ui.separator();
                    let bar = match app.load_fraction() {
                        Some(frac) => egui::ProgressBar::new(frac).desired_width(140.0),
                        None => egui::ProgressBar::new(0.0)
                            .animate(true)
                            .desired_width(140.0),
                    };
                    ui.add(bar);
                    let stage = app.load_stage();
                    if !stage.is_empty() {
                        ui.label(
                            egui::RichText::new(stage).color(theme::MUTED2).size(11.0),
                        );
                    }
                    if ui.button("Cancel").clicked() {
                        app.request_load_cancel();
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(&app.ui.status_msg)
                            .color(theme::MUTED2)
                            .size(12.0),
                    );
                });
            });
        });
}
