use eframe::egui;
use material_icons::{Icon as MaterialIcon, icon_to_char};
use moleucle_3dview_rs::RenderStyle;

use super::KuromameApp;

fn mi(icon: MaterialIcon) -> String {
    icon_to_char(icon).to_string()
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
    egui::Panel::top("menu_bar").show(ctx, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui
                    .button(format!("{} Open Molecule", mi(MaterialIcon::FolderOpen)))
                    .on_hover_text("Ctrl+O")
                    .clicked()
                {
                    app.open_file();
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
        });
    });
}

pub fn render_left_panel(app: &mut KuromameApp, ctx: &egui::Context) {
    let font_size = 17.0;
    let font_family = egui::FontFamily::Proportional;
    let primary = egui::Color32::from_rgb(19, 161, 152);
    let secondary = egui::Color32::from_rgb(241, 98, 69);

    egui::SidePanel::left("left_panel")
        .resizable(true)
        .default_width(260.0)
        .min_width(180.0)
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::WHITE)
                .inner_margin(egui::Margin::symmetric(12, 10)),
        )
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // Loaded file info
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(245, 245, 245))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Loaded").strong());
                        ui.monospace(&app.data.loaded_summary);
                        if app.data.is_modified {
                            ui.colored_label(egui::Color32::from_rgb(200, 160, 0), "modified");
                        } else {
                            ui.colored_label(egui::Color32::from_rgb(80, 180, 80), "saved");
                        }
                    });

                ui.add_space(4.0);

                // Keyboard shortcut hints
                ui.horizontal_wrapped(|ui| {
                    ui.small("Ctrl+O open");
                    ui.small("Ctrl+Shift+O TOP+GRO");
                    ui.small("Ctrl+R edit");
                    ui.small("Ctrl+S export");
                    ui.small("Ctrl+B path");
                    ui.small("Ctrl+H hbond");
                    ui.small("Ctrl+Shift+A clear");
                });

                ui.separator();

                // Selection buttons — full panel width
                let w = ui.available_width();
                let btn_size = egui::vec2(w, 34.0);

                let can_select_path = app.selection.selected_atom_indices.len() == 2;
                if ui
                    .add_enabled_ui(can_select_path, |ui| {
                        ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "{} Select Between",
                                    mi(MaterialIcon::AltRoute)
                                ))
                                .color(egui::Color32::WHITE)
                                .size(font_size - 2.0)
                                .family(font_family.clone()),
                            )
                            .fill(primary),
                        )
                    })
                    .inner
                    .on_hover_text("Ctrl+B (need 2 atoms selected)")
                    .clicked()
                {
                    app.select_shortest_path(
                        app.selection.selected_atom_indices[0],
                        app.selection.selected_atom_indices[1],
                    );
                }

                ui.add_space(2.0);

                if ui
                    .add_enabled_ui(!app.selection.selected_atom_indices.is_empty(), |ui| {
                        ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "{} Change Resname",
                                    mi(MaterialIcon::Edit)
                                ))
                                .color(egui::Color32::WHITE)
                                .size(font_size - 2.0)
                                .family(font_family.clone()),
                            )
                            .fill(secondary),
                        )
                    })
                    .inner
                    .on_hover_text("Ctrl+R")
                    .clicked()
                {
                    app.open_resname_dialog();
                }

                ui.add_space(2.0);

                if ui
                    .add_sized(
                        btn_size,
                        egui::Button::new(
                            egui::RichText::new(format!(
                                "{} Clear Selection",
                                mi(MaterialIcon::ClearAll)
                            ))
                            .color(egui::Color32::WHITE)
                            .size(font_size - 2.0)
                            .family(font_family.clone()),
                        )
                        .fill(egui::Color32::from_rgb(84, 98, 125)),
                    )
                    .on_hover_text("Ctrl+Shift+A")
                    .clicked()
                {
                    app.clear_selection();
                }

                ui.separator();

                // Selector
                ui.label("Selector:");
                let input = ui.add_sized(
                    egui::vec2(ui.available_width(), 30.0),
                    egui::TextEdit::singleline(&mut app.ui.selector_input)
                        .hint_text("aC1|aC2"),
                );
                let apply_by_enter =
                    input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    let apply_by_button = ui.button("Apply").clicked();
                    let output_by_button = ui.button("-> Text").clicked();
                    if apply_by_enter || apply_by_button {
                        app.apply_selector_expression();
                    }
                    if output_by_button {
                        if let Some(selector_text) = app.update_selector_input_from_selection() {
                            ui.ctx().copy_text(selector_text);
                            app.ui.status_msg =
                                "Selection exported to selector text and copied".to_string();
                        } else {
                            app.ui.status_msg =
                                "No selected atoms with usable atom names".to_string();
                        }
                    }
                });

                ui.label(
                    egui::RichText::new(format!(
                        "Selected: {}",
                        app.selection.selected_atom_indices.len()
                    ))
                    .strong(),
                );

                if app.ndx_group_count() > 0 {
                    ui.separator();

                    let mut ndx_visible = app.ndx_visible();
                    if ui.checkbox(&mut ndx_visible, "Show NDX Group").changed() {
                        app.set_ndx_visible(ndx_visible);
                    }

                    ui.label("NDX Group:");

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

                    let file_name = app
                        .ndx_file_name()
                        .unwrap_or_else(|| "(unknown)".to_string());
                    let group_name = app.ndx_selected_group_name().unwrap_or("-");
                    ui.label(format!("File: {}", file_name));
                    ui.label(format!("Group: {}", group_name));
                    ui.label(format!("Rendered atoms: {}", app.ndx_selected_atom_count()));
                }

                ui.separator();

                ui.label("Render Style:");
                let mut style = app.viewport.render_style();
                ui.selectable_value(&mut style, RenderStyle::BallStick, "BallStick");
                ui.selectable_value(&mut style, RenderStyle::BallOnly, "BallOnly");
                ui.selectable_value(&mut style, RenderStyle::Wireframe, "Wireframe");
                ui.selectable_value(&mut style, RenderStyle::Circles, "Circles");
                app.viewport.set_render_style(style);
            });
        });
}

pub fn render_bottom_status_bar(app: &mut KuromameApp, ctx: &egui::Context) {
    egui::Panel::bottom("status_bar")
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_hex(&"#ffffff".to_string()).unwrap())
                .inner_margin(egui::Margin::symmetric(12, 8)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.selection.with_hbond_chk, "Select with hbond");
                ui.separator();
                ui.label(egui::RichText::new("Hover").strong());
                ui.monospace(&app.ui.hovered_atom_info);
                ui.separator();
                ui.label(egui::RichText::new("Status").strong());
                ui.label(&app.ui.status_msg);
            });
        });
}
