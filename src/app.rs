use crate::inter_molecular_interaction_render::{
    InterMolecularInteractionRender, InteractionPairsState,
};
use crate::ndx_selection_render::{NdxSelectionRender, NdxSelectionState};
use crate::parsing::{AtomRecord, GroFile, Mol2File, NdxFile, PdbFile, TopFile};
use crate::view_rs::To3dViewMolecule;
use eframe::egui::{self};
use moleucle_3dview_rs::additional_render::SelectedAtomRenderState;
use moleucle_3dview_rs::{Atom, InteractiveMoleculeViewport, Molecule, SelectedAtomRender, ViewPortEvent};
use rfd::FileDialog;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[path = "app_ui.rs"]
mod app_ui;

struct LoadedDataState {
    pdb_file: Option<PdbFile>,
    gro_file: Option<GroFile>,
    top_file: Option<TopFile>,
    ndx_file: Option<NdxFile>,
    top_file_path: Option<PathBuf>,
    gro_file_path: Option<PathBuf>,
    ndx_file_path: Option<PathBuf>,
    current_file_path: Option<PathBuf>,
    loaded_summary: String,
    is_modified: bool,
}

impl LoadedDataState {
    fn clear_structures(&mut self) {
        self.pdb_file = None;
        self.gro_file = None;
        self.top_file = None;
        self.top_file_path = None;
        self.gro_file_path = None;
    }
}

struct SelectionState {
    with_hbond_chk: bool,
    selected_atom_indices: Vec<usize>,
}

struct UiState {
    status_msg: String,
    show_edit_dialog: bool,
    new_res_name: String,
    hovered_atom_info: String,
    selector_input: String,
    ndx_selected_group_index: Option<usize>,
    ndx_visible: bool,
    ndx_selected_atom_count: usize,
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s <= 0.0 {
        return (l, l, l);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - (l * s)
    };
    let p = 2.0 * l - q;

    fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            return p + (q - p) * 6.0 * t;
        }
        if t < 1.0 / 2.0 {
            return q;
        }
        if t < 2.0 / 3.0 {
            return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
        }
        p
    }

    (
        hue_to_rgb(p, q, h + 1.0 / 3.0),
        hue_to_rgb(p, q, h),
        hue_to_rgb(p, q, h - 1.0 / 3.0),
    )
}

fn color_by_res_name(atom: &Atom, is_selected: bool) -> (f32, f32, f32) {
    if is_selected {
        return (1.0, 0.0, 0.0);
    }

    let key = atom
        .res_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(atom.element.as_str());

    // Deterministic hash so the same residue name gets the same color each run.
    let hash = key.bytes().fold(2166136261u32, |acc, b| {
        (acc ^ (b as u32)).wrapping_mul(16777619)
    });
    let hue = (hash % 360) as f32 / 360.0;
    hsl_to_rgb(hue, 0.65, 0.52)
}

pub struct KuromameApp {
    molecule: Option<Molecule>,
    viewport: InteractiveMoleculeViewport,
    pub render_state: Option<egui_wgpu::RenderState>,
    data: LoadedDataState,
    selection: SelectionState,
    ui: UiState,
    hovered_atom: Arc<Mutex<Option<usize>>>,
}

impl KuromameApp {
    fn apply_visual_theme(ctx: &egui::Context) {
        let primary = egui::Color32::from_rgb(19, 161, 152);
        let secondary = egui::Color32::from_rgb(241, 98, 69);

        let mut style = (*ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);

        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(24.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(18.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(18.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(16.0, egui::FontFamily::Monospace),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
        );

        style.visuals.widgets.active.bg_fill = primary;
        style.visuals.widgets.hovered.bg_fill = secondary;
        style.visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;
        style.visuals.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;
        style.visuals.selection.bg_fill = primary;
        style.visuals.hyperlink_color = secondary;

        ctx.set_global_style(style);
    }

    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "material_icons".to_string(),
            egui::FontData::from_static(material_icons::FONT).into(),
        );
        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            family.push("material_icons".to_string());
        }
        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            family.push("material_icons".to_string());
        }
        cc.egui_ctx.set_fonts(fonts);
        Self::apply_visual_theme(&cc.egui_ctx);
        let mut viewport = InteractiveMoleculeViewport::new(None);
        viewport.add_additional_render_box(Box::new(SelectedAtomRender::new()));
        viewport.add_additional_render_box(Box::new(InterMolecularInteractionRender::new()));
        viewport.add_additional_render_box(Box::new(NdxSelectionRender::new()));

        let hovered_atom: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
        let hovered_atom_for_handler = Arc::clone(&hovered_atom);
        viewport.register_event_handler(Box::new(move |vp, event| match event {
            ViewPortEvent::hovered { atom } => {
                if let Ok(mut g) = hovered_atom_for_handler.lock() {
                    *g = Some(atom);
                }
            }
            ViewPortEvent::clicked { atom } => {
                let mut atoms = vp.selected_atoms();
                if let Some(pos) = atoms.iter().position(|&a| a == atom) {
                    atoms.remove(pos);
                } else {
                    atoms.push(atom);
                }
                vp.set_state_by_type(SelectedAtomRenderState {
                    selected_atoms: atoms,
                    color: [1.0, 0.0, 0.0],
                });
            }
        }));

        Self {
            molecule: None,
            viewport,
            render_state: cc.wgpu_render_state.clone(),
            data: LoadedDataState {
                pdb_file: None,
                gro_file: None,
                top_file: None,
                ndx_file: None,
                top_file_path: None,
                gro_file_path: None,
                ndx_file_path: None,
                current_file_path: None,
                loaded_summary: "No file loaded".to_string(),
                is_modified: false,
            },
            selection: SelectionState {
                with_hbond_chk: false,
                selected_atom_indices: Vec::new(),
            },
            ui: UiState {
                status_msg: "Ready".to_string(),
                show_edit_dialog: false,
                new_res_name: String::new(),
                hovered_atom_info: "Hover an atom for details".to_string(),
                selector_input: String::new(),
                ndx_selected_group_index: None,
                ndx_visible: true,
                ndx_selected_atom_count: 0,
            },
            hovered_atom,
        }
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.ui.status_msg = msg.into();
    }

    fn set_loaded_summary(&mut self, summary: impl Into<String>) {
        self.data.loaded_summary = summary.into();
    }

    fn mark_modified(&mut self) {
        self.data.is_modified = true;
    }

    fn mark_clean(&mut self) {
        self.data.is_modified = false;
    }

    fn sync_viewer_molecule(&mut self) {
        if let Some(molecule) = self.molecule.clone() {
            self.viewport.set_molecule(molecule);
            self.viewport.focus_on_molecule_center();
        }
    }

    fn post_load_cleanup(&mut self) {
        self.sync_viewer_molecule();
        self.refresh_ndx_selection_state();
    }

    fn normalized_ndx_indices(entries: &[u32], atom_count: usize) -> Vec<usize> {
        let mut atom_indices = Vec::new();
        for &entry in entries {
            let Some(index_0_based) = (entry as usize).checked_sub(1) else {
                continue;
            };

            if atom_count > 0 && index_0_based >= atom_count {
                continue;
            }

            atom_indices.push(index_0_based);
        }

        atom_indices.sort_unstable();
        atom_indices.dedup();
        atom_indices
    }

    fn current_ndx_indices(&self) -> Vec<usize> {
        let Some(ndx) = self.data.ndx_file.as_ref() else {
            return Vec::new();
        };
        let Some(group_index) = self.ui.ndx_selected_group_index else {
            return Vec::new();
        };
        let Some(group) = ndx.groups.get(group_index) else {
            return Vec::new();
        };

        let atom_count = self.molecule.as_ref().map(|m| m.atoms.len()).unwrap_or(0);
        Self::normalized_ndx_indices(&group.entries, atom_count)
    }

    fn refresh_ndx_selection_state(&mut self) {
        let atom_indices = if self.ui.ndx_visible {
            self.current_ndx_indices()
        } else {
            Vec::new()
        };

        self.ui.ndx_selected_atom_count = atom_indices.len();
        self.viewport.set_state_by_type(NdxSelectionState {
            atom_indices,
            visible: self.ui.ndx_visible,
        });
    }

    pub fn open_ndx_file(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("NDX Files", &["ndx"])
            .set_title("Import NDX file")
            .pick_file()
        {
            self.load_ndx_file(path);
        }
    }

    pub fn reload_loaded_files(&mut self) {
        let top_path = self.data.top_file_path.clone();
        let gro_path = self.data.gro_file_path.clone();
        let current_file_path = self.data.current_file_path.clone();
        let ndx_path = self.data.ndx_file_path.clone();

        let mut reloaded_any = false;

        if let (Some(top_path), Some(gro_path)) = (top_path, gro_path) {
            self.load_top_and_gro_for_resname_sync(top_path, gro_path);
            reloaded_any = true;
        } else if let Some(path) = current_file_path {
            self.load_file(path);
            reloaded_any = true;
        }

        if let Some(path) = ndx_path {
            self.load_ndx_file(path);
            reloaded_any = true;
        }

        if !reloaded_any {
            self.set_status("No loaded files to reload");
        }
    }

    fn load_ndx_file(&mut self, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();

        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => {
                self.set_status("Failed to read NDX file");
                return;
            }
        };

        let ndx = match NdxFile::parse(&content) {
            Ok(ndx) => ndx,
            Err(err) => {
                self.set_status(format!("NDX parse failed: {err}"));
                return;
            }
        };

        let group_count = ndx.groups.len();
        self.data.ndx_file = Some(ndx);
        self.data.ndx_file_path = Some(path);
        self.ui.ndx_selected_group_index = if group_count > 0 { Some(0) } else { None };
        self.ui.ndx_visible = true;
        self.refresh_ndx_selection_state();

        self.set_status(format!(
            "Imported NDX {} ({} groups)",
            file_name, group_count
        ));
    }

    pub fn ndx_group_options(&self) -> Vec<String> {
        let Some(ndx) = self.data.ndx_file.as_ref() else {
            return Vec::new();
        };

        ndx.groups
            .iter()
            .map(|group| format!("{} ({})", group.name, group.entries.len()))
            .collect()
    }

    pub fn ndx_selected_group_index(&self) -> Option<usize> {
        self.ui.ndx_selected_group_index
    }

    pub fn set_ndx_selected_group_index(&mut self, group_index: usize) {
        let group_name = {
            let Some(ndx) = self.data.ndx_file.as_ref() else {
                return;
            };
            let Some(group) = ndx.groups.get(group_index) else {
                return;
            };
            group.name.clone()
        };

        self.ui.ndx_selected_group_index = Some(group_index);
        self.refresh_ndx_selection_state();
        self.set_status(format!(
            "NDX group selected: {} ({} atoms rendered)",
            group_name, self.ui.ndx_selected_atom_count
        ));
    }

    pub fn ndx_selected_group_name(&self) -> Option<&str> {
        let ndx = self.data.ndx_file.as_ref()?;
        let idx = self.ui.ndx_selected_group_index?;
        Some(ndx.groups.get(idx)?.name.as_str())
    }

    pub fn ndx_group_count(&self) -> usize {
        self.data
            .ndx_file
            .as_ref()
            .map(|ndx| ndx.groups.len())
            .unwrap_or(0)
    }

    pub fn ndx_file_name(&self) -> Option<String> {
        self.data
            .ndx_file_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
    }

    pub fn ndx_visible(&self) -> bool {
        self.ui.ndx_visible
    }

    pub fn set_ndx_visible(&mut self, visible: bool) {
        self.ui.ndx_visible = visible;
        self.refresh_ndx_selection_state();
    }

    pub fn ndx_selected_atom_count(&self) -> usize {
        self.ui.ndx_selected_atom_count
    }

    fn open_resname_dialog(&mut self) {
        self.ui.show_edit_dialog = true;
        self.ui.new_res_name = "ALA".to_string();
    }

    fn clear_selection(&mut self) {
        self.selection.selected_atom_indices.clear();
    }

    fn toggle_hbond_selection(&mut self) {
        self.selection.with_hbond_chk = !self.selection.with_hbond_chk;
    }

    fn atom_name_at(&self, atom_index: usize) -> Option<String> {
        if let Some(mol) = &self.molecule {
            if let Some(atom) = mol.atoms.get(atom_index) {
                if let Some(name) = atom.name.as_deref() {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_ascii_uppercase());
                    }
                }

                let element = atom.element.trim();
                if !element.is_empty() {
                    return Some(element.to_ascii_uppercase());
                }
            }
        }

        if let Some(gro) = &self.data.gro_file {
            if let Some(atom) = gro.atoms().nth(atom_index) {
                let name = atom.atom_name.trimmed();
                if !name.is_empty() {
                    return Some(name.to_ascii_uppercase());
                }
            }
        }

        if let Some(pdb) = &self.data.pdb_file {
            if let Some(atom) = pdb.atoms().nth(atom_index) {
                let name = atom.name.trim();
                if !name.is_empty() {
                    return Some(name.to_ascii_uppercase());
                }
            }
        }

        if let Some(top) = &self.data.top_file {
            if let Some(atom) = top.atoms().nth(atom_index) {
                let name = atom.atom.trim();
                if !name.is_empty() {
                    return Some(name.to_ascii_uppercase());
                }
            }
        }

        None
    }

    fn parse_selector_tokens(selector_expr: &str) -> Vec<String> {
        selector_expr
            .split('|')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .filter_map(|token| {
                let normalized = if token.len() >= 2
                    && matches!(token.chars().next(), Some('a' | 'A'))
                    && token[1..]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    token[1..].trim()
                } else {
                    token
                };

                if normalized.is_empty() {
                    None
                } else {
                    Some(normalized.to_ascii_uppercase())
                }
            })
            .collect()
    }

    fn apply_selector_expression(&mut self) {
        let selector_expr = self.ui.selector_input.trim();
        if selector_expr.is_empty() {
            self.set_status("Selector is empty (example: aC1|aC2)");
            return;
        }

        let Some(mol) = self.molecule.as_ref() else {
            self.set_status("No molecule loaded");
            return;
        };

        let selector_tokens = Self::parse_selector_tokens(selector_expr);
        if selector_tokens.is_empty() {
            self.set_status("Selector format is invalid (example: aC1|aC2)");
            return;
        }

        let token_set: std::collections::HashSet<String> = selector_tokens.into_iter().collect();
        let mut selected_indices = Vec::new();

        for atom_index in 0..mol.atoms.len() {
            if let Some(atom_name) = self.atom_name_at(atom_index) {
                if token_set.contains(&atom_name) {
                    selected_indices.push(atom_index);
                }
            }
        }

        self.selection.selected_atom_indices = selected_indices;

        if self.selection.selected_atom_indices.is_empty() {
            self.set_status("Selector matched 0 atoms");
        } else {
            self.set_status(format!(
                "Selector matched {} atoms",
                self.selection.selected_atom_indices.len()
            ));
        }
    }

    fn selector_expression_from_selection(&self) -> Option<String> {
        if self.selection.selected_atom_indices.is_empty() {
            return None;
        }

        let mut seen = std::collections::HashSet::new();
        let mut selector_tokens = Vec::new();

        for &atom_index in &self.selection.selected_atom_indices {
            if let Some(atom_name) = self.atom_name_at(atom_index) {
                if seen.insert(atom_name.clone()) {
                    selector_tokens.push(format!("a{}", atom_name));
                }
            }
        }

        if selector_tokens.is_empty() {
            None
        } else {
            Some(selector_tokens.join("|"))
        }
    }

    fn update_selector_input_from_selection(&mut self) -> Option<String> {
        let text = self.selector_expression_from_selection()?;
        self.ui.selector_input = text.clone();
        Some(text)
    }

    fn handle_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        let shortcuts = ctx.input(|i| {
            let ctrl = i.modifiers.ctrl;
            let shift = i.modifiers.shift;
            (
                ctrl && !shift && i.key_pressed(egui::Key::O),
                ctrl && shift && i.key_pressed(egui::Key::O),
                ctrl && !shift && i.key_pressed(egui::Key::R),
                ctrl && !shift && i.key_pressed(egui::Key::S),
                ctrl && !shift && i.key_pressed(egui::Key::H),
                ctrl && !shift && i.key_pressed(egui::Key::B),
                ctrl && shift && i.key_pressed(egui::Key::A),
            )
        });

        if shortcuts.0 {
            self.open_file();
        }
        if shortcuts.1 {
            self.open_top_and_gro_for_resname_sync();
        }
        if shortcuts.2 {
            self.open_resname_dialog();
        }
        if shortcuts.3 {
            self.export_structure();
        }
        if shortcuts.4 {
            self.toggle_hbond_selection();
        }
        if shortcuts.5 && self.selection.selected_atom_indices.len() == 2 {
            self.select_shortest_path(
                self.selection.selected_atom_indices[0],
                self.selection.selected_atom_indices[1],
            );
        }
        if shortcuts.6 {
            self.clear_selection();
        }
    }

    fn hovered_atom_info(&self, atom_index: usize) -> Option<String> {
        let mut atom_name: Option<String> = None;
        let mut res_name: Option<String> = None;

        if let Some(mol) = &self.molecule {
            if let Some(atom) = mol.atoms.get(atom_index) {
                if let Some(name) = atom.name.as_deref() {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() {
                        atom_name = Some(trimmed.to_string());
                    }
                }

                if atom_name.is_none() && !atom.element.trim().is_empty() {
                    atom_name = Some(atom.element.trim().to_string());
                }

                if let Some(name) = atom.res_name.as_deref() {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() {
                        res_name = Some(trimmed.to_string());
                    }
                }
            }
        }

        if let Some(pdb) = &self.data.pdb_file {
            if let Some(atom) = pdb.atoms().nth(atom_index) {
                if atom_name.is_none() && !atom.name.trim().is_empty() {
                    atom_name = Some(atom.name.trim().to_string());
                }
                if res_name.is_none() && !atom.res_name.trim().is_empty() {
                    res_name = Some(atom.res_name.trim().to_string());
                }
            }
        }

        if let Some(gro) = &self.data.gro_file {
            if let Some(atom) = gro.atoms().nth(atom_index) {
                if atom_name.is_none() {
                    let name = atom.atom_name.trimmed();
                    if !name.is_empty() {
                        atom_name = Some(name.to_string());
                    }
                }
                if res_name.is_none() {
                    let name = atom.res_name.trimmed();
                    if !name.is_empty() {
                        res_name = Some(name.to_string());
                    }
                }
            }
        }

        if let Some(top) = &self.data.top_file {
            if let Some(atom) = top.atoms().nth(atom_index) {
                if atom_name.is_none() && !atom.atom.trim().is_empty() {
                    atom_name = Some(atom.atom.trim().to_string());
                }
                if res_name.is_none() && !atom.res.trim().is_empty() {
                    res_name = Some(atom.res.trim().to_string());
                }
            }
        }

        if atom_name.is_none() && res_name.is_none() {
            return None;
        }

        Some(format!(
            "Index={} AtomName={} Resname={}",
            atom_index + 1,
            atom_name.unwrap_or_else(|| "-".to_string()),
            res_name.unwrap_or_else(|| "-".to_string())
        ))
    }

    fn sync_viewer_resnames_from_loaded_files(&mut self) {
        let viewer_atom_count = self
            .molecule
            .as_ref()
            .map(|mol| mol.atoms.len())
            .unwrap_or(0);

        if viewer_atom_count == 0 {
            return;
        }

        let top_resnames = self.data.top_file.as_ref().map(|top| {
            top.atoms()
                .map(|atom| atom.res.trim().to_string())
                .collect::<Vec<_>>()
        });

        let gro_resnames = self.data.gro_file.as_ref().map(|gro| {
            gro.atoms()
                .map(|atom| atom.res_name.trimmed().to_string())
                .collect::<Vec<_>>()
        });

        let pdb_resnames = self.data.pdb_file.as_ref().map(|pdb| {
            pdb.atoms()
                .map(|atom| atom.res_name.trim().to_string())
                .collect::<Vec<_>>()
        });

        let resnames: Vec<String> = if let Some(names) = top_resnames {
            if names.len() == viewer_atom_count {
                names
            } else {
                gro_resnames
                    .filter(|names| names.len() == viewer_atom_count)
                    .or_else(|| pdb_resnames.filter(|names| names.len() == viewer_atom_count))
                    .unwrap_or_default()
            }
        } else {
            gro_resnames
                .filter(|names| names.len() == viewer_atom_count)
                .or_else(|| pdb_resnames.filter(|names| names.len() == viewer_atom_count))
                .unwrap_or_default()
        };

        if resnames.is_empty() {
            return;
        }

        if let Some(mol) = &mut self.molecule {
            for (atom, name) in mol.atoms.iter_mut().zip(resnames.into_iter()) {
                atom.res_name = Some(name);
            }
            self.sync_viewer_molecule();
        }
    }

    fn toggle_selected_atom(&mut self, atom_index: usize) -> bool {
        let was_selected = self.selection.selected_atom_indices.contains(&atom_index);
        if was_selected {
            self.selection
                .selected_atom_indices
                .retain(|&i| i != atom_index);
        } else {
            self.selection.selected_atom_indices.push(atom_index);
        }

        !was_selected
    }

    fn add_connected_hydrogens(&mut self, atom_index: usize) {
        let Some(mol) = &self.molecule else {
            return;
        };

        let mut targets = Self::collect_connected_hydrogens(atom_index, mol);
        targets.sort_unstable();
        targets.dedup();

        for idx in targets {
            if !self.selection.selected_atom_indices.contains(&idx) {
                self.selection.selected_atom_indices.push(idx);
            }
        }
    }

    fn remove_connected_hydrogens(&mut self, atom_index: usize) {
        let Some(mol) = &self.molecule else {
            return;
        };

        let mut targets = Self::collect_connected_hydrogens(atom_index, mol);
        targets.sort_unstable();
        targets.dedup();

        self.selection
            .selected_atom_indices
            .retain(|idx| !targets.contains(idx));
    }

    pub fn open_file(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("PDB Files", &["pdb", "ent", "cif"])
            .add_filter("MOL2 Files", &["mol2"])
            .add_filter("GRO Files", &["gro"])
            .add_filter("TOP/ITP Files", &["top", "itp"])
            .pick_file()
        {
            self.load_file(path);
        }
    }

    pub fn open_top_and_gro_for_resname_sync(&mut self) {
        let top_path = FileDialog::new()
            .add_filter("TOP Files", &["top"])
            .set_title("Select TOP file")
            .pick_file();
        let gro_path = FileDialog::new()
            .add_filter("GRO Files", &["gro"])
            .set_title("Select GRO file")
            .pick_file();

        match (top_path, gro_path) {
            (Some(top), Some(gro)) => self.load_top_and_gro_for_resname_sync(top, gro),
            _ => {
                self.set_status("TOP/GRO pair selection cancelled");
            }
        }
    }

    fn load_top_and_gro_for_resname_sync(&mut self, top_path: PathBuf, gro_path: PathBuf) {
        let top = match TopFile::load_from_path(&top_path) {
            Ok(top) => top,
            Err(err) => {
                self.set_status(err);
                return;
            }
        };
        let gro = match GroFile::load_from_path(&gro_path) {
            Ok(gro) => gro,
            Err(_) => {
                self.set_status("Failed to read GRO file");
                return;
            }
        };

        let (molecule, interaction_pairs) = match top.generate_molecule_with_gro(&gro) {
            Ok(result) => result,
            Err(error_message) => {
                self.set_status(error_message);
                return;
            }
        };
        println!(
            "Generated molecule with {} atoms and {} interaction pairs",
            molecule.atoms.len(),
            interaction_pairs.len()
        );
        self.viewport.set_state_by_type(InteractionPairsState {
            pairs: interaction_pairs,
        });
        self.data.top_file_path = Some(top_path);
        self.data.gro_file_path = Some(gro_path);
        self.data.current_file_path = None;
        self.set_molecule_and_frame(molecule);

        self.post_load_cleanup();
    }

    pub fn load_file(&mut self, path: PathBuf) {
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            self.set_status("Unsupported file type");
            return;
        };

        match ext.to_lowercase().as_str() {
            "pdb" | "ent" => self.load_pdb_file(path),
            "mol2" => self.load_mol2_file(path),
            "gro" => self.load_gro_file(path),
            _ => {
                self.set_status("Unsupported file type");
            }
        }
        self.post_load_cleanup();
    }

    fn load_pdb_file(&mut self, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let pdb = PdbFile::load(&content);
                let mol = pdb.to_molecule();
                self.set_molecule_and_frame(mol);
                self.data.clear_structures();
                self.data.pdb_file = Some(pdb);
                self.data.current_file_path = Some(path);
                self.set_loaded_summary(format!("PDB: {}", file_name));
                self.mark_clean();
                self.set_status("Loaded PDB");
            }
            Err(_) => self.set_status("Failed to load PDB file"),
        }
    }

    fn load_mol2_file(&mut self, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let mol2 = Mol2File::load(&content);
                let mol = mol2.to_molecule();
                let pdb_from_mol2 = PdbFile::from_molecule(&mol);
                self.set_molecule_and_frame(mol);
                self.data.clear_structures();
                self.data.pdb_file = Some(pdb_from_mol2);
                self.data.current_file_path = Some(path);
                self.set_loaded_summary(format!("MOL2: {}", file_name));
                self.mark_clean();
                self.set_status("Loaded MOL2");
            }
            Err(_) => self.set_status("Failed to load MOL2 file"),
        }
    }

    fn load_gro_file(&mut self, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
        const LARGE_GRO_THRESHOLD_BYTES: u64 = 20 * 1024 * 1024;
        let large_gro = std::fs::metadata(&path)
            .map(|m| m.len() >= LARGE_GRO_THRESHOLD_BYTES)
            .unwrap_or(false);

        match GroFile::load_from_path(&path) {
            Ok(gro) => {
                let mol = gro.to_molecule_with_metadata(!large_gro, None);
                self.set_molecule_and_frame(mol);
                self.data.clear_structures();
                self.data.gro_file = if large_gro { None } else { Some(gro) };
                self.data.current_file_path = Some(path);
                self.set_loaded_summary(format!("GRO: {}", file_name));
                self.mark_clean();
                if large_gro {
                    self.set_status(
                        "Loaded GRO (compact mode: reduced memory, GRO edit/export disabled)",
                    );
                } else {
                    self.set_status("Loaded GRO");
                }
            }
            Err(_) => self.set_status("Failed to load GRO file"),
        }
    }

    fn set_molecule_and_frame(&mut self, molecule: Molecule) {
        self.molecule = Some(molecule);
        self.sync_viewer_molecule();

        let Some(mol) = self.molecule.as_ref() else {
            return;
        };

        if mol.atoms.is_empty() {
            return;
        }

        // Note: Camera setup would be handled by viewport if needed
        // For now, just sync the molecule to the viewport
        self.sync_viewer_molecule();
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped_paths: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });

        if dropped_paths.is_empty() {
            return;
        }

        let mut top_path: Option<PathBuf> = None;
        let mut gro_path: Option<PathBuf> = None;
        let mut ndx_path: Option<PathBuf> = None;

        for path in &dropped_paths {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                match ext.to_ascii_lowercase().as_str() {
                    "top" => top_path = Some(path.clone()),
                    "gro" => gro_path = Some(path.clone()),
                    "ndx" => ndx_path = Some(path.clone()),
                    _ => {}
                }
            }
        }

        if let (Some(top), Some(gro)) = (top_path, gro_path) {
            self.load_top_and_gro_for_resname_sync(top, gro);
            return;
        }

        if let Some(path) = ndx_path {
            self.load_ndx_file(path);
            return;
        }

        if let Some(path) = dropped_paths.into_iter().next() {
            self.load_file(path);
        }
    }

    pub fn render_ui(&mut self, ctx: &egui::Context) {
        app_ui::render_edit_dialog(self, ctx);
    }

    fn select_shortest_path(&mut self, start: usize, end: usize) {
        let Some(mol) = &self.molecule else {
            return;
        };

        // Find all atoms on all simple paths between start and end
        let atoms_on_path = Self::find_atoms_between_dfs(mol, start, end);

        // Toggle only the atoms on the path first.
        for idx in atoms_on_path.iter().copied() {
            self.toggle_selected_atom(idx);
        }

        if self.selection.with_hbond_chk {
            for idx in atoms_on_path {
                if self.selection.selected_atom_indices.contains(&idx) {
                    self.add_connected_hydrogens(idx);
                }
            }
        }
    }

    fn find_atoms_between_dfs(mol: &Molecule, start: usize, end: usize) -> Vec<usize> {
        if start == end {
            return vec![start];
        }

        // 1. Build adjacency map from bonds
        let mut adj: std::collections::HashMap<usize, std::collections::HashSet<usize>> =
            std::collections::HashMap::new();

        for bond in &mol.bonds {
            adj.entry(bond.atom_a).or_default().insert(bond.atom_b);
            adj.entry(bond.atom_b).or_default().insert(bond.atom_a);
        }

        // 2. DFS to find all simple paths and collect all atoms on any path
        let mut all_path_atoms = std::collections::HashSet::new();

        fn dfs(
            current: usize,
            target: usize,
            adj: &std::collections::HashMap<usize, std::collections::HashSet<usize>>,
            visited: &mut std::collections::HashSet<usize>,
            path: &mut Vec<usize>,
            all_path_atoms: &mut std::collections::HashSet<usize>,
        ) {
            if current == target {
                // Found a path - add all atoms in this path
                all_path_atoms.extend(path.iter());
                return;
            }

            if let Some(neighbors) = adj.get(&current) {
                for &neighbor in neighbors {
                    if !visited.contains(&neighbor) {
                        visited.insert(neighbor);
                        path.push(neighbor);
                        dfs(neighbor, target, adj, visited, path, all_path_atoms);
                        path.pop();
                        visited.remove(&neighbor);
                    }
                }
            }
        }

        let mut visited = std::collections::HashSet::new();
        visited.insert(start);
        let mut path = vec![start];
        dfs(
            start,
            end,
            &adj,
            &mut visited,
            &mut path,
            &mut all_path_atoms,
        );

        // 3. Return as sorted vector
        let mut result: Vec<usize> = all_path_atoms.into_iter().collect();
        result.sort();
        result
    }

    fn collect_connected_hydrogens(atom_idx: usize, mol: &Molecule) -> Vec<usize> {
        let mut hydrogens = Vec::new();
        for bond in &mol.bonds {
            let neighbor = if bond.atom_a == atom_idx {
                Some(bond.atom_b)
            } else if bond.atom_b == atom_idx {
                Some(bond.atom_a)
            } else {
                None
            };

            if let Some(n_idx) = neighbor {
                if let Some(atom) = mol.atoms.get(n_idx) {
                    // Check if element starts with "H" (matching Python's "H" in name check)
                    if atom.element.starts_with("H") && !hydrogens.contains(&n_idx) {
                        hydrogens.push(n_idx);
                    }
                }
            }
        }
        hydrogens
    }

    fn apply_res_name_change(&mut self) {
        let new_name = self.ui.new_res_name.trim().to_uppercase();
        if new_name.len() > 3 {
            self.set_status("Residue name too long");
            return;
        }
        let new_name = format!("{:>3}", new_name); // Pad to 3 chars
        let indices_to_update = self.selection.selected_atom_indices.clone();

        if indices_to_update.is_empty() {
            self.set_status("No atoms selected");
            return;
        }

        if let Some(pdb) = &mut self.data.pdb_file {
            // Update PDB atoms
            let mut atoms_vec: Vec<&mut AtomRecord> = pdb.atoms_mut().collect();
            for &idx in &indices_to_update {
                if let Some(atom) = atoms_vec.get_mut(idx) {
                    atom.res_name = new_name.clone();
                }
            }
        }

        if let Some(gro) = &mut self.data.gro_file {
            // In GRO, the 2nd field is residue name (resname).
            let mut atoms_vec: Vec<_> = gro.atoms_mut().collect();
            for &idx in &indices_to_update {
                if let Some(atom) = atoms_vec.get_mut(idx) {
                    atom.set_res_name(&new_name);
                }
            }
        }

        if let Some(top) = &mut self.data.top_file {
            // Keep TOP in sync with current selection indices when TOP+GRO are loaded together.
            let mut atoms_vec: Vec<_> = top.atoms_mut().collect();
            for &idx in &indices_to_update {
                if let Some(atom) = atoms_vec.get_mut(idx) {
                    atom.set_res_name(&new_name);
                }
            }
        }

        // Keep renderer metadata aligned with currently loaded structural data.
        self.sync_viewer_resnames_from_loaded_files();
        self.mark_modified();

        // Clear selection
        self.selection.selected_atom_indices.clear();
        self.set_status("Residue names updated");
    }

    fn export_structure(&mut self) {
        if self.data.top_file.is_none()
            && self.data.gro_file.is_none()
            && self.data.pdb_file.is_none()
        {
            if let Some(mol) = &self.molecule {
                self.data.pdb_file = Some(PdbFile::from_molecule(mol));
            }
        }

        if let Some(path) = FileDialog::new().save_file() {
            let saved = if let Some(top) = &self.data.top_file {
                let content = top.dump();
                std::fs::write(&path, content).is_ok()
            } else if let Some(gro) = &self.data.gro_file {
                let content = gro.dump();
                std::fs::write(&path, content).is_ok()
            } else if let Some(pdb) = &mut self.data.pdb_file {
                let content = pdb.dump();
                std::fs::write(&path, content).is_ok()
            } else {
                false
            };

            if saved {
                self.mark_clean();
                self.set_status("Exported structure");
            } else {
                self.set_status("Failed to export structure");
            }
        }
    }
}

impl eframe::App for KuromameApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Update hover info from the previous frame's viewport pick result.
        let last_hovered = self.hovered_atom.lock().ok().and_then(|mut g| g.take());
        self.ui.hovered_atom_info = last_hovered
            .and_then(|atom| self.hovered_atom_info(atom))
            .unwrap_or_else(|| "Hover an atom for details".to_string());

        self.handle_dropped_files(&ctx);
        self.handle_keyboard_shortcuts(&ctx);

        app_ui::render_top_info_panel(self, &ctx);
        app_ui::render_edit_dialog(self, &ctx);

        app_ui::render_bottom_status_bar(self, &ctx);

        egui::CentralPanel::default().show(&ctx, |ui| {
            let Some(render_state) = &self.render_state else {
                ui.heading("WGPU backend is unavailable");
                ui.label("Start with the wgpu backend enabled in eframe.");
                return;
            };

            if let Err(err) = self.viewport.show(ui, render_state) {
                ui.colored_label(egui::Color32::RED, format!("Render failed: {err}"));
            }
        });

        // Show a drop-target overlay while files are being dragged over the window.
        // This provides visual feedback on Linux (X11/Wayland) and other platforms.
        let is_dragging = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if is_dragging {
            egui::Area::new(egui::Id::new("dnd_overlay"))
                .fixed_pos(egui::pos2(0.0, 0.0))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    let rect = ctx.content_rect();
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::ZERO,
                        egui::Color32::from_rgba_unmultiplied(30, 140, 240, 100),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Drop molecular file here",
                        egui::FontId::proportional(28.0),
                        egui::Color32::WHITE,
                    );
                });
        }
    }

    fn on_exit(&mut self) {
        if let Some(render_state) = &self.render_state {
            self.viewport.free_egui_texture(render_state);
        }
    }
}
