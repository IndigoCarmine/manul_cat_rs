use eframe::egui;
use material_icons::{Icon as MaterialIcon, icon_to_char};
use moleucle_3dview_rs::RenderStyle;

use super::KuromameApp;

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

pub fn render_menu_bar(app: &mut KuromameApp, ctx: &egui::Context) {
    egui::Panel::top("menu_bar")
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(14, 8)),
        )
        .show(ctx, |ui| {
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
        ui.separator();
        if ui
            .button(format!("{} Export", mi(MaterialIcon::Save)))
            .on_hover_text("Ctrl+S")
            .clicked()
        {
            app.export_structure();
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
        ] {
            ui.label(egui::RichText::new(line).color(theme::MUTED).size(12.0));
        }
    });
}

pub fn render_left_panel(app: &mut KuromameApp, ctx: &egui::Context) {
    egui::SidePanel::left("left_panel")
        .resizable(true)
        .default_width(264.0)
        .min_width(200.0)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(16, 16)),
        )
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                file_header(app, ui);
                ui.add_space(14.0);
                selection_section(app, ui);
                ui.add_space(14.0);
                components_section(app, ui);
            });
        });
}

fn file_header(app: &mut KuromameApp, ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(&app.data.loaded_summary)
            .color(theme::TEXT)
            .size(14.0)
            .strong(),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let atoms = app.atom_count();
        if atoms > 0 {
            ui.label(
                egui::RichText::new(format!("{atoms} atoms"))
                    .color(theme::MUTED2)
                    .size(12.0),
            );
        } else {
            ui.label(
                egui::RichText::new("no file loaded")
                    .color(theme::MUTED2)
                    .size(12.0),
            );
        }
    });
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

    if app.has_res_names() {
        ui.horizontal(|ui| {
            if secondary_button(ui, "Show all".to_string(), true).clicked() {
                app.set_all_res_visible(true);
            }
            if secondary_button(ui, "Hide all".to_string(), true).clicked() {
                app.set_all_res_visible(false);
            }
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .id_salt("res_visibility_scroll")
            .max_height(260.0)
            .show(ui, |ui| {
                let rows = app.res_visibility_list();
                let mut toggles: Vec<(String, bool)> = Vec::new();
                for (name, visible) in &rows {
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
                    if resp.clicked() {
                        toggles.push((name.clone(), !visible));
                    }
                }
                for (name, vis) in toggles {
                    app.set_res_visible(&name, vis);
                }
            });
    } else {
        ui.label(
            egui::RichText::new("Load a structure to list residues")
                .color(theme::MUTED2)
                .size(12.0),
        );
    }

    // NDX group block (only when an NDX file with groups is loaded).
    if app.ndx_group_count() > 0 {
        ui.add_space(12.0);
        section_label(ui, "NDX GROUP");
        ui.add_space(6.0);

        let mut ndx_visible = app.ndx_visible();
        if ui.checkbox(&mut ndx_visible, "Show NDX group").changed() {
            app.set_ndx_visible(ndx_visible);
        }

        let options = app.ndx_group_options();
        let mut selected_index = app.ndx_selected_group_index().unwrap_or(0);
        if selected_index >= options.len() {
            selected_index = 0;
        }
        egui::ComboBox::from_id_salt("ndx_group_selector")
            .width(ui.available_width())
            .selected_text(
                options
                    .get(selected_index)
                    .cloned()
                    .unwrap_or_else(|| "(none)".to_string()),
            )
            .show_ui(ui, |ui| {
                for (idx, label) in options.iter().enumerate() {
                    if ui.selectable_label(idx == selected_index, label).clicked() {
                        selected_index = idx;
                    }
                }
            });
        if app.ndx_selected_group_index() != Some(selected_index) {
            app.set_ndx_selected_group_index(selected_index);
        }

        let group_name = app.ndx_selected_group_name().unwrap_or("-");
        ui.label(
            egui::RichText::new(format!(
                "{} · {} atoms",
                group_name,
                app.ndx_selected_atom_count()
            ))
            .color(theme::MUTED2)
            .size(12.0),
        );
    }
}

/// Bottom dock: render-style segmented control + trajectory transport.
pub fn render_bottom_dock(app: &mut KuromameApp, ctx: &egui::Context) {
    egui::Panel::bottom("bottom_dock")
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(16, 10)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                section_label(ui, "STYLE");
                style_segment(app, ui);

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

    // Right-aligned FPS + frame/time, slider fills the middle.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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

pub fn render_bottom_status_bar(app: &mut KuromameApp, ctx: &egui::Context) {
    egui::Panel::bottom("status_bar")
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(16, 6)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.selection.with_hbond_chk, "Select with hbond");
                ui.separator();
                ui.label(
                    egui::RichText::new(&app.ui.hovered_atom_info)
                        .color(theme::MUTED)
                        .size(12.0),
                );
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
