use crate::layer_overlay_render::{
    LayerOverlayGeom, LayerOverlayRender, LayerOverlayState, OverlayAtom,
};
use crate::inter_molecular_interaction_render::{
    InterMolecularInteractionRender, InteractionPairsState,
};
use crate::ndx_selection_render::{NdxSelectionRender, NdxSelectionState};
use crate::parsing::{
    AtomRecord, GroFile, MartiniForceField, Mol2File, NdxFile, PdbFile, SURFACE_RES_NAME, TopFile,
    XtcFile, XtcFrame,
};
use crate::simulation_cell_render::{SimulationCellRender, SimulationCellRenderState};
use crate::surface_mesh_render::{SurfaceLayer, SurfaceMeshRender, SurfaceMeshState};
use crate::view_rs::{To3dViewMolecule, molecule_from_parts, view_atom};
use eframe::egui::{self};
use lin_alg::f32::Vec3;
use moleucle_3dview_rs::additional_render::SelectedAtomRenderState;
use moleucle_3dview_rs::molecule::{AtomMeta, Bond};
use moleucle_3dview_rs::{
    Atom, InteractiveMoleculeViewport, Molecule, SelectedAtomRender, ViewPortEvent,
    ball_stick_radius, default_color_fn,
};
use rfd::FileDialog;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[path = "app_ui.rs"]
mod app_ui;

enum StructureFile {
    Pdb(PdbFile),
    Gro(GroFile),
}

impl StructureFile {
    fn gro(&self) -> Option<&GroFile> {
        match self {
            StructureFile::Gro(g) => Some(g),
            _ => None,
        }
    }

    fn gro_mut(&mut self) -> Option<&mut GroFile> {
        match self {
            StructureFile::Gro(g) => Some(g),
            _ => None,
        }
    }

    fn pdb(&self) -> Option<&PdbFile> {
        match self {
            StructureFile::Pdb(p) => Some(p),
            _ => None,
        }
    }

    fn pdb_mut(&mut self) -> Option<&mut PdbFile> {
        match self {
            StructureFile::Pdb(p) => Some(p),
            _ => None,
        }
    }
}

#[derive(Default)]
struct LoadedDataState {
    structure_file: Option<StructureFile>,
    structure_file_path: Option<PathBuf>,
    top_file: Option<TopFile>,
    top_file_path: Option<PathBuf>,
    ndx_file: Option<NdxFile>,
    ndx_file_path: Option<PathBuf>,
    loaded_summary: String,
    is_modified: bool,
}

impl LoadedDataState {
    fn clear_structures(&mut self) {
        self.structure_file = None;
        self.structure_file_path = None;
        self.top_file = None;
        self.top_file_path = None;
    }
}

#[derive(Default)]
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

#[derive(Default)]
struct TrajectoryUiState {
    current_frame: usize,
    is_playing: bool,
    playback_fps: f32,
    last_advance_time: f64,
    /// Number of sub-steps each real-frame transition is divided into during
    /// playback. `1` disables smoothing (frames shown as-is); `N` inserts
    /// `N - 1` linearly-interpolated frames between consecutive real frames.
    interp_steps: u32,
    /// Current sub-step within the transition out of `current_frame` (`0` means
    /// the exact frame is displayed). Always `< interp_steps`.
    interp_sub: u32,
}

/// Per-residue-name show/hide state plus the index mapping between the full
/// molecule (`self.molecule`, original indices) and the possibly-filtered
/// molecule actually handed to the viewport (view indices).
///
/// The maps are empty when nothing is hidden, which means "identity" — the
/// viewport gets the full molecule and `to_view`/`to_orig` are no-ops. This
/// keeps the common (all-visible) case allocation-free and behaviour-identical
/// to before the feature existed.
#[derive(Default)]
struct VisibilityState {
    res_visible: BTreeMap<String, bool>,
    view_to_orig: Vec<usize>,
    orig_to_view: Vec<Option<u32>>,
}

impl VisibilityState {
    fn any_hidden(&self) -> bool {
        self.res_visible.values().any(|&v| !v)
    }

    fn is_filtered(&self) -> bool {
        !self.view_to_orig.is_empty()
    }

    /// Original atom index -> viewport atom index (None if currently hidden).
    fn to_view(&self, orig: usize) -> Option<usize> {
        if self.orig_to_view.is_empty() {
            Some(orig) // identity: nothing hidden
        } else {
            self.orig_to_view
                .get(orig)
                .copied()
                .flatten()
                .map(|v| v as usize)
        }
    }

    /// Viewport atom index -> original atom index.
    fn to_orig(&self, view: usize) -> usize {
        self.view_to_orig.get(view).copied().unwrap_or(view)
    }

    /// Remap 1-based interaction pairs into viewport space, dropping any pair
    /// with a hidden endpoint.
    fn map_pairs(&self, pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
        if !self.is_filtered() {
            return pairs.to_vec();
        }
        pairs
            .iter()
            .filter_map(|&(a, b)| {
                let va = self.to_view(a.checked_sub(1)?)?;
                let vb = self.to_view(b.checked_sub(1)?)?;
                Some((va + 1, vb + 1))
            })
            .collect()
    }
}

/// Default color for the base structure's own dot surface.
const BASE_SURFACE_COLOR: [f32; 3] = [0.35, 0.72, 0.95];

/// Palette cycled through when new overlay surfaces are added, so each loaded
/// file starts with a distinct color the user can then override.
const OVERLAY_SURFACE_PALETTE: [[f32; 3]; 6] = [
    [0.95, 0.35, 0.35], // red
    [0.45, 0.85, 0.45], // green
    [0.95, 0.72, 0.30], // orange
    [0.80, 0.45, 0.95], // purple
    [0.30, 0.85, 0.85], // cyan
    [0.95, 0.55, 0.80], // pink
];

/// Identity colours cycled through as document layers are created, so each layer
/// has a distinct swatch/eye tint in the LAYERS panel.
const LAYER_PALETTE: [[f32; 3]; 6] = [
    [0.30, 0.64, 1.00], // blue (accent)
    [0.88, 0.70, 0.25], // amber
    [0.35, 0.82, 0.76], // teal
    [0.80, 0.45, 0.95], // purple
    [0.45, 0.85, 0.45], // green
    [0.95, 0.55, 0.80], // pink
];

/// An additional dot surface loaded from a separate file and overlaid on top of
/// the base structure. Managed independently of the base structure through the
/// overlay tab UI, with a user-configurable color.
struct OverlaySurface {
    name: String,
    dots: Vec<Vec3>,
    color: [f32; 3],
    visible: bool,
}

/// One document layer: a complete, independent structure with its own files,
/// visibility, selection, trajectory and Martini/surface state. Every layer is
/// the same type; the only special one is the *active* layer, whose state is
/// checked out into `KuromameApp`'s working fields and drawn as the viewer's
/// full "main" molecule. Non-active layers render as spheres.
///
/// While a layer is active its heavy payload lives in the app's working fields
/// (moved out via [`KuromameApp::save_active_layer`]/`load_active_layer`); the
/// slot kept here then holds only a valid `name`/`visible` plus placeholder
/// payload. `name`/`visible` are never moved, so they stay valid for all layers.
#[derive(Default)]
struct Layer {
    name: String,
    /// Whether this layer is drawn as spheres while it is not the active layer.
    visible: bool,
    /// Identity colour shown as the layer's swatch / eye tint in the LAYERS
    /// panel, assigned from [`LAYER_PALETTE`] when the layer is created. A UI
    /// marker only; the 3D spheres keep their element colours.
    color: [f32; 3],
    /// Overlay opacity in `0.0..=1.0` (alpha) used when this layer is drawn as
    /// spheres (i.e. while it is not the active layer). `1.0` is fully opaque.
    opacity: f32,
    molecule: Option<Molecule>,
    base_molecule: Option<Molecule>,
    data: LoadedDataState,
    selection: SelectionState,
    trajectory: Vec<XtcFrame>,
    trajectory_path: Option<PathBuf>,
    traj_ui: TrajectoryUiState,
    visibility: VisibilityState,
    interaction_pairs: Vec<(usize, usize)>,
    surface_dots: Vec<Vec3>,
    surface_visible: bool,
    martini_ff: Option<MartiniForceField>,
    bead_types: Vec<String>,
    martini_visible: bool,
    // Per-structure UI state (mirrors the working copies in `UiState`).
    selector_input: String,
    ndx_selected_group_index: Option<usize>,
    ndx_visible: bool,
    ndx_selected_atom_count: usize,
}

impl Layer {
    /// A fresh, empty layer with the same initial field values the app used for
    /// its single structure (`surface_visible`/`martini_visible`/`ndx_visible`
    /// on, `playback_fps` 10, summary "No file loaded").
    fn new(name: String, color: [f32; 3]) -> Self {
        Self {
            name,
            visible: true,
            color,
            opacity: 1.0,
            molecule: None,
            base_molecule: None,
            data: LoadedDataState {
                loaded_summary: "No file loaded".to_string(),
                ..LoadedDataState::default()
            },
            selection: SelectionState::default(),
            trajectory: Vec::new(),
            trajectory_path: None,
            traj_ui: TrajectoryUiState {
                playback_fps: 10.0,
                interp_steps: 1,
                ..TrajectoryUiState::default()
            },
            visibility: VisibilityState::default(),
            interaction_pairs: Vec::new(),
            surface_dots: Vec::new(),
            surface_visible: true,
            martini_ff: None,
            bead_types: Vec::new(),
            martini_visible: true,
            selector_input: String::new(),
            ndx_selected_group_index: None,
            ndx_visible: true,
            ndx_selected_atom_count: 0,
        }
    }
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
        .res_name()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(atom.element.as_str());

    // Deterministic hash so the same residue name gets the same color each run.
    let hash = key.bytes().fold(2166136261u32, |acc, b| {
        (acc ^ (b as u32)).wrapping_mul(16777619)
    });
    let hue = (hash % 360) as f32 / 360.0;
    hsl_to_rgb(hue, 0.65, 0.52)
}

/// One entry in the "loaded files" overview shown in the left panel: a short
/// type badge (`GRO`/`TOP`/`NDX`/`XTC`/…), the file name, and a one-line detail
/// (atom/frame/group count). Lets the user see everything loaded at a glance.
pub struct LoadedFileRow {
    pub badge: &'static str,
    pub name: String,
    pub detail: String,
}

pub struct KuromameApp {
    molecule: Option<Molecule>,
    viewport: InteractiveMoleculeViewport,
    pub render_state: Option<egui_wgpu::RenderState>,
    data: LoadedDataState,
    selection: SelectionState,
    ui: UiState,
    hovered_atom: Arc<Mutex<Option<usize>>>,
    /// Viewport-space atom indices clicked since the last frame. The viewport
    /// event handler (a closure with no access to `self`) queues them here;
    /// `update` drains the queue into the app-side selection
    /// (`selection.selected_atom_indices`, original indices) each frame. Without
    /// this bridge, clicking an atom would highlight it in the 3D view but never
    /// reach the selection the panels and menu actions actually operate on.
    clicked_atoms: Arc<Mutex<Vec<usize>>>,
    trajectory: Vec<XtcFrame>,
    trajectory_path: Option<PathBuf>,
    base_molecule: Option<Molecule>,
    traj_ui: TrajectoryUiState,
    visibility: VisibilityState,
    /// Inter-molecular interaction pairs (1-based, original indices) kept so they
    /// can be remapped whenever the visible set changes.
    interaction_pairs: Vec<(usize, usize)>,
    /// Dot-surface positions (nm) parsed from the base PDB, empty when none.
    surface_dots: Vec<Vec3>,
    /// Whether the base dot surface is currently drawn.
    surface_visible: bool,
    /// Extra dot surfaces loaded from separate files and overlaid on top of the
    /// base structure, each with its own color. Managed by the overlay tab UI.
    overlay_surfaces: Vec<OverlaySurface>,
    /// Index of the overlay surface whose controls the tab UI currently shows.
    active_overlay: usize,
    /// All document layers. The active layer's heavy payload is checked out into
    /// the working fields above (`molecule`, `data`, `visibility`, …); every
    /// other layer holds its full state here and is drawn as spheres. There is
    /// always at least one layer.
    layers: Vec<Layer>,
    /// Index into `layers` of the active layer (the viewer's main molecule).
    active_layer: usize,
    /// Martini bead-type registry parsed from a loaded Martini force-field
    /// `.itp` (its `[ atomtypes ]` / `[ nonbond_params ]`). `None` until one is
    /// loaded; drives coarse-grained bead rendering.
    martini_ff: Option<MartiniForceField>,
    /// Bead type of each atom in `self.molecule` (original-index order), taken
    /// from the topology's `atom_type` or the atom name. Empty when no molecule.
    bead_types: Vec<String>,
    /// Whether Martini bead spheres are drawn (only has an effect once a Martini
    /// force field is loaded and beads resolve to known types).
    martini_visible: bool,
    /// Monotonic counter bumped on every viewport rebuild. Used to give each
    /// filtered molecule a distinct generation so the renderer's geometry cache
    /// (keyed on `(molecule_ptr, generation, …)`) actually rebuilds when the
    /// visible atom set changes — otherwise hiding/showing residues has no
    /// visible effect because every filtered molecule starts at generation 0 and
    /// lives at the same viewer address.
    view_revision: u64,
}

impl KuromameApp {
    fn apply_visual_theme(ctx: &egui::Context) {
        use app_ui::theme;

        let mut style = (*ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);

        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(20.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(11.0, egui::FontFamily::Proportional),
        );

        // Dark palette matching the "Viewer UI" design.
        let mut v = egui::Visuals::dark();
        v.dark_mode = true;
        v.override_text_color = Some(theme::TEXT);
        v.panel_fill = theme::PANEL;
        v.window_fill = theme::PANEL;
        v.extreme_bg_color = theme::INPUT_BG;
        v.faint_bg_color = theme::HOVER_BG;
        v.window_stroke = egui::Stroke::new(1.0, theme::BORDER);
        v.hyperlink_color = theme::ACCENT;
        v.selection.bg_fill = theme::ACCENT.gamma_multiply(0.35);
        v.selection.stroke = egui::Stroke::new(1.0, theme::ACCENT);

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = theme::PANEL;
        w.noninteractive.weak_bg_fill = theme::PANEL;
        w.noninteractive.bg_stroke = egui::Stroke::new(1.0, theme::BORDER);
        w.noninteractive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT);
        w.inactive.bg_fill = theme::HOVER_BG;
        w.inactive.weak_bg_fill = theme::HOVER_BG;
        w.inactive.bg_stroke = egui::Stroke::new(1.0, theme::BORDER2);
        w.inactive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT);
        w.inactive.corner_radius = egui::CornerRadius::same(7);
        w.hovered.bg_fill = theme::BORDER2;
        w.hovered.weak_bg_fill = theme::BORDER2;
        w.hovered.bg_stroke = egui::Stroke::new(1.0, theme::MUTED);
        w.hovered.fg_stroke = egui::Stroke::new(1.0, theme::TEXT);
        w.hovered.corner_radius = egui::CornerRadius::same(7);
        w.active.bg_fill = theme::ACCENT;
        w.active.weak_bg_fill = theme::ACCENT;
        w.active.bg_stroke = egui::Stroke::new(1.0, theme::ACCENT);
        w.active.fg_stroke = egui::Stroke::new(1.0, theme::ACCENT_FG);
        w.active.corner_radius = egui::CornerRadius::same(7);
        w.open.bg_fill = theme::HOVER_BG;
        w.open.bg_stroke = egui::Stroke::new(1.0, theme::BORDER2);
        w.open.fg_stroke = egui::Stroke::new(1.0, theme::TEXT);

        style.visuals = v;
        ctx.set_global_style(style);
    }

    /// Number of atoms in the currently loaded molecule (0 when none).
    pub fn atom_count(&self) -> usize {
        self.molecule.as_ref().map(|m| m.atoms.len()).unwrap_or(0)
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
        viewport.add_additional_render_box(Box::new(SimulationCellRender::new()));
        viewport.add_additional_render_box(Box::new(SurfaceMeshRender::new()));
        viewport.add_additional_render_box(Box::new(LayerOverlayRender::new()));

        let hovered_atom: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
        let hovered_atom_for_handler = Arc::clone(&hovered_atom);
        let clicked_atoms: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
        let clicked_atoms_for_handler = Arc::clone(&clicked_atoms);
        viewport.register_event_handler(Box::new(move |_vp, event| match event {
            ViewPortEvent::hovered { atom } => {
                if let Ok(mut g) = hovered_atom_for_handler.lock() {
                    *g = Some(atom);
                }
            }
            // Only queue the click here; the app owns the selection and pushes
            // the resulting red highlight back to the viewport in `update`, so a
            // click and every other selection path share one source of truth.
            ViewPortEvent::clicked { atom } => {
                if let Ok(mut g) = clicked_atoms_for_handler.lock() {
                    g.push(atom);
                }
            }
        }));

        Self {
            molecule: None,
            viewport,
            render_state: cc.wgpu_render_state.clone(),
            data: LoadedDataState {
                structure_file: None,
                structure_file_path: None,
                top_file: None,
                top_file_path: None,
                ndx_file: None,
                ndx_file_path: None,
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
            clicked_atoms,
            trajectory: Vec::new(),
            trajectory_path: None,
            base_molecule: None,
            traj_ui: TrajectoryUiState {
                current_frame: 0,
                is_playing: false,
                playback_fps: 10.0,
                interp_steps: 1,
                interp_sub: 0,
                last_advance_time: 0.0,
            },
            visibility: VisibilityState::default(),
            interaction_pairs: Vec::new(),
            surface_dots: Vec::new(),
            surface_visible: true,
            overlay_surfaces: Vec::new(),
            active_overlay: 0,
            layers: vec![Layer::new("Layer 1".to_string(), LAYER_PALETTE[0])],
            active_layer: 0,
            martini_ff: None,
            bead_types: Vec::new(),
            martini_visible: true,
            view_revision: 0,
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
        self.rebuild_viewport(false);
    }

    fn sync_viewer_molecule_and_focus(&mut self) {
        self.rebuild_viewport(true);
    }

    /// Push the molecule to the viewport, applying the current per-residue
    /// visibility filter, and re-derive every index-based render state (NDX,
    /// interaction pairs) in the resulting viewport-index space. This is the one
    /// place geometry is handed to the viewport.
    fn rebuild_viewport(&mut self, focus: bool) {
        // Keep bead types aligned with the current molecule before we (re)derive
        // any index-based render state below.
        self.recompute_bead_types();

        let Some(full) = self.molecule.as_ref() else {
            return;
        };

        let pushed_positions: Vec<Vec3>;
        if !self.visibility.any_hidden() {
            // Identity: hand over the full molecule, no remapping needed.
            self.visibility.view_to_orig.clear();
            self.visibility.orig_to_view.clear();
            pushed_positions = full.atoms.iter().map(|a| a.position).collect();
            self.viewport.set_molecule(full.clone());
        } else {
            let mut view_to_orig: Vec<usize> = Vec::with_capacity(full.atoms.len());
            let mut orig_to_view: Vec<Option<u32>> = vec![None; full.atoms.len()];
            let mut atoms: Vec<Atom> = Vec::new();
            for (orig, atom) in full.atoms.iter().enumerate() {
                let res = atom.res_name().unwrap_or("");
                let visible = self.visibility.res_visible.get(res).copied().unwrap_or(true);
                if visible {
                    orig_to_view[orig] = Some(view_to_orig.len() as u32);
                    view_to_orig.push(orig);
                    atoms.push(atom.clone());
                }
            }
            let mut bonds: Vec<Bond> = Vec::new();
            for bond in &full.bonds {
                if let (Some(a), Some(b)) = (orig_to_view[bond.atom_a], orig_to_view[bond.atom_b]) {
                    bonds.push(Bond {
                        atom_a: a as usize,
                        atom_b: b as usize,
                        order: bond.order,
                    });
                }
            }
            self.visibility.view_to_orig = view_to_orig;
            self.visibility.orig_to_view = orig_to_view;
            pushed_positions = atoms.iter().map(|a| a.position).collect();
            self.viewport.set_molecule(molecule_from_parts(atoms, bonds));
        }

        // `set_molecule` always installs a molecule at generation 0, and the
        // viewer stores it at a fixed address, so two successive filtered
        // molecules would share the renderer's geometry-cache key and the view
        // would not update. Bump the generation to a value that cycles so each
        // rebuild differs from the previous one, forcing a cache miss. The
        // position payload is unchanged; only the generation counter advances.
        let bump = (self.view_revision % 4) + 1;
        for _ in 0..bump {
            let _ = self.viewport.update_positions(&pushed_positions);
        }
        self.view_revision = self.view_revision.wrapping_add(1);

        if focus {
            self.viewport.focus_on_molecule_center();
        }

        // Re-apply index-based render states in the new viewport-index space.
        self.refresh_ndx_selection_state();
        self.refresh_interaction_pairs();
        self.refresh_martini_bead_state();
        // Re-project the selection (stored in original indices) into the new
        // viewport-index space so the red highlight follows residue-visibility
        // toggles and layer swaps instead of being dropped.
        self.sync_selection_to_viewport();
    }

    fn refresh_interaction_pairs(&mut self) {
        let pairs = self.visibility.map_pairs(&self.interaction_pairs);
        self.viewport
            .set_state_by_type(InteractionPairsState { pairs });
    }

    /// Re-derive the bead type of every atom in `self.molecule` (original-index
    /// order). Prefers the topology's per-atom `atom_type`; falls back to the
    /// atom name when there is no matching topology (e.g. a bare CG `.gro`).
    fn recompute_bead_types(&mut self) {
        let Some(mol) = self.molecule.as_ref() else {
            self.bead_types.clear();
            return;
        };
        let from_top = self
            .data
            .top_file
            .as_ref()
            .map(|t| t.expanded_atom_types())
            .filter(|types| types.len() == mol.atoms.len());

        self.bead_types = match from_top {
            Some(types) => types,
            None => mol
                .atoms
                .iter()
                .map(|a| a.name().unwrap_or_else(|| a.element.as_str()).to_string())
                .collect(),
        };
    }

    /// Push the current Martini bead styling to the viewport in viewport-index
    /// order (matching the possibly-filtered molecule). A no-op display when no
    /// Martini force field is loaded, so nothing changes for ordinary structures.
    /// Apply Martini per-bead sizing/colouring as per-atom radius/colour
    /// overrides on the main molecule (viewport-index order), so beads render as
    /// the main molecule (shading, picking and opacity included) rather than a
    /// separate overlay. Clears the overrides — restoring element rendering —
    /// when no Martini force field is loaded or the bead view is off. Atoms
    /// whose bead type is unknown fall back to their element radius/colour.
    fn refresh_martini_bead_state(&mut self) {
        let active =
            self.martini_visible && self.martini_ff.is_some() && self.molecule.is_some();
        if !active {
            self.viewport.set_atom_radii(None);
            self.viewport.set_atom_colors(None);
            return;
        }

        let (radii, colors) = {
            let ff = self.martini_ff.as_ref().unwrap();
            let mol = self.molecule.as_ref().unwrap();
            let filtered = self.visibility.is_filtered();
            let count = if filtered {
                self.visibility.view_to_orig.len()
            } else {
                mol.atoms.len()
            };
            let mut radii = Vec::with_capacity(count);
            let mut colors = Vec::with_capacity(count);
            for view in 0..count {
                let orig = if filtered {
                    self.visibility.view_to_orig[view]
                } else {
                    view
                };
                let atom = &mol.atoms[orig];
                match self
                    .bead_types
                    .get(orig)
                    .and_then(|bead| ff.radius_nm(bead).map(|r| (bead, r)))
                {
                    Some((bead, radius)) => {
                        let (r, g, b) = MartiniForceField::color(bead);
                        radii.push(radius);
                        colors.push([r, g, b, 1.0]);
                    }
                    None => {
                        radii.push(ball_stick_radius(&atom.element, false));
                        let c = default_color_fn(atom, false);
                        colors.push([c.0, c.1, c.2, c.3]);
                    }
                }
            }
            (radii, colors)
        };
        self.viewport.set_atom_radii(Some(radii));
        self.viewport.set_atom_colors(Some(colors));
    }

    /// Parse `path` as a Martini force field and, if it defines bead types,
    /// register it and refresh bead styling for any loaded structure. Returns the
    /// bead-type count when a force field was found.
    fn try_load_martini_ff(&mut self, path: &std::path::Path) -> Option<usize> {
        // Parse from the include-expanded content so a Martini force field that a
        // system `.top` pulls in via `#include` is still found.
        let expanded = TopFile::expand_includes(path).ok()?;
        let ff = MartiniForceField::parse(&expanded);
        if !ff.is_forcefield() {
            return None;
        }
        let count = ff.bead_type_count();
        self.martini_ff = Some(ff);
        self.recompute_bead_types();
        self.refresh_martini_bead_state();
        Some(count)
    }

    /// Whether a Martini force field is loaded (enables the bead-view toggle).
    pub fn has_martini_ff(&self) -> bool {
        self.martini_ff.is_some()
    }

    pub fn martini_visible(&self) -> bool {
        self.martini_visible
    }

    pub fn set_martini_visible(&mut self, visible: bool) {
        if self.martini_visible != visible {
            self.martini_visible = visible;
            self.refresh_martini_bead_state();
        }
    }

    /// Refresh the set of residue names known to the UI from the current
    /// molecule, keeping any existing show/hide choices for names that persist.
    fn refresh_res_names(&mut self) {
        let Some(mol) = self.molecule.as_ref() else {
            return;
        };
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for atom in &mol.atoms {
            seen.insert(atom.res_name().unwrap_or("").to_string());
        }
        self.visibility.res_visible.retain(|name, _| seen.contains(name));
        for name in seen {
            self.visibility.res_visible.entry(name).or_insert(true);
        }
    }

    /// Residue names with their current visibility, for the UI panel.
    pub fn res_visibility_list(&self) -> Vec<(String, bool)> {
        self.visibility
            .res_visible
            .iter()
            .map(|(name, &visible)| (name.clone(), visible))
            .collect()
    }

    pub fn has_res_names(&self) -> bool {
        !self.visibility.res_visible.is_empty()
    }

    pub fn set_res_visible(&mut self, name: &str, visible: bool) {
        if let Some(entry) = self.visibility.res_visible.get_mut(name) {
            if *entry != visible {
                *entry = visible;
                self.rebuild_viewport(false);
            }
        }
    }

    pub fn set_all_res_visible(&mut self, visible: bool) {
        let mut changed = false;
        for value in self.visibility.res_visible.values_mut() {
            if *value != visible {
                *value = visible;
                changed = true;
            }
        }
        if changed {
            self.rebuild_viewport(false);
        }
    }

    fn post_load_cleanup(&mut self) {
        // A freshly loaded structure invalidates any prior atom selection (its
        // indices refer to the old molecule); clear it so `rebuild_viewport`
        // does not project stale indices onto the new geometry.
        self.selection.selected_atom_indices.clear();
        self.refresh_res_names();
        // When a dot surface is present, draw it through the dedicated surface
        // renderer and hide the raw "DOT" atoms from the main geometry so they
        // do not show up as a blob of large spheres on top of the surface.
        if !self.surface_dots.is_empty() {
            self.visibility
                .res_visible
                .insert(SURFACE_RES_NAME.to_string(), false);
        }
        self.refresh_surface_state();
        self.sync_viewer_molecule_and_focus();
        // Redraw the other layers as spheres in case atom counts/positions moved.
        self.refresh_layer_overlays();
    }

    /// Collect the base surface (if visible) and every visible overlay surface
    /// into a single layered render state and push it to the viewport.
    fn refresh_surface_state(&mut self) {
        let mut layers: Vec<SurfaceLayer> = Vec::new();
        if self.surface_visible && !self.surface_dots.is_empty() {
            layers.push(SurfaceLayer {
                positions: self.surface_dots.clone(),
                color: (
                    BASE_SURFACE_COLOR[0],
                    BASE_SURFACE_COLOR[1],
                    BASE_SURFACE_COLOR[2],
                ),
            });
        }
        for overlay in &self.overlay_surfaces {
            if overlay.visible && !overlay.dots.is_empty() {
                layers.push(SurfaceLayer {
                    positions: overlay.dots.clone(),
                    color: (overlay.color[0], overlay.color[1], overlay.color[2]),
                });
            }
        }
        self.viewport.set_state_by_type(SurfaceMeshState { layers });
    }

    /// Open a file dialog to add an overlay surface from a PDB with a dot surface.
    pub fn open_overlay_surface_file(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("Surface PDB", &["pdb", "ent"])
            .set_title("Add overlay surface (PDB with DOT surface)")
            .pick_file()
        {
            self.load_overlay_surface_file(path);
        }
    }

    fn load_overlay_surface_file(&mut self, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                self.set_status(format!("Failed to read {file_name}"));
                return;
            }
        };

        let dots = PdbFile::load(&content).surface_dots();
        if dots.is_empty() {
            self.set_status(format!("{file_name} contains no DOT surface"));
            return;
        }

        let count = dots.len();
        let color = OVERLAY_SURFACE_PALETTE
            [self.overlay_surfaces.len() % OVERLAY_SURFACE_PALETTE.len()];
        self.overlay_surfaces.push(OverlaySurface {
            name: file_name.clone(),
            dots,
            color,
            visible: true,
        });
        self.active_overlay = self.overlay_surfaces.len() - 1;
        self.refresh_surface_state();
        self.set_status(format!("Added overlay surface {file_name} ({count} dots)"));
    }

    pub fn overlay_count(&self) -> usize {
        self.overlay_surfaces.len()
    }

    pub fn overlay_names(&self) -> Vec<String> {
        self.overlay_surfaces.iter().map(|o| o.name.clone()).collect()
    }

    pub fn active_overlay_index(&self) -> usize {
        self.active_overlay
    }

    pub fn set_active_overlay(&mut self, idx: usize) {
        if idx < self.overlay_surfaces.len() {
            self.active_overlay = idx;
        }
    }

    pub fn overlay_name(&self, idx: usize) -> Option<String> {
        self.overlay_surfaces.get(idx).map(|o| o.name.clone())
    }

    pub fn overlay_dot_count(&self, idx: usize) -> usize {
        self.overlay_surfaces.get(idx).map(|o| o.dots.len()).unwrap_or(0)
    }

    pub fn overlay_visible(&self, idx: usize) -> bool {
        self.overlay_surfaces.get(idx).map(|o| o.visible).unwrap_or(false)
    }

    pub fn set_overlay_visible(&mut self, idx: usize, visible: bool) {
        if let Some(overlay) = self.overlay_surfaces.get_mut(idx) {
            if overlay.visible != visible {
                overlay.visible = visible;
                self.refresh_surface_state();
            }
        }
    }

    pub fn overlay_color(&self, idx: usize) -> [f32; 3] {
        self.overlay_surfaces
            .get(idx)
            .map(|o| o.color)
            .unwrap_or([1.0, 1.0, 1.0])
    }

    pub fn set_overlay_color(&mut self, idx: usize, color: [f32; 3]) {
        if let Some(overlay) = self.overlay_surfaces.get_mut(idx) {
            if overlay.color != color {
                overlay.color = color;
                self.refresh_surface_state();
            }
        }
    }

    pub fn remove_overlay(&mut self, idx: usize) {
        if idx < self.overlay_surfaces.len() {
            self.overlay_surfaces.remove(idx);
            if self.active_overlay >= self.overlay_surfaces.len() {
                self.active_overlay = self.overlay_surfaces.len().saturating_sub(1);
            }
            self.refresh_surface_state();
        }
    }

    // --- Document layers ---------------------------------------------------

    /// Move the active layer's working state (the app's `molecule`/`data`/… and
    /// the per-structure `ui` bits) out of the app fields and into its slot in
    /// `layers`, leaving the working fields empty. `name`/`visible` in the slot
    /// are untouched (they never live in the working fields).
    fn save_active_layer(&mut self) {
        let a = self.active_layer;
        self.layers[a].molecule = self.molecule.take();
        self.layers[a].base_molecule = self.base_molecule.take();
        self.layers[a].data = std::mem::take(&mut self.data);
        self.layers[a].selection = std::mem::take(&mut self.selection);
        self.layers[a].trajectory = std::mem::take(&mut self.trajectory);
        self.layers[a].trajectory_path = self.trajectory_path.take();
        self.layers[a].traj_ui = std::mem::take(&mut self.traj_ui);
        self.layers[a].visibility = std::mem::take(&mut self.visibility);
        self.layers[a].interaction_pairs = std::mem::take(&mut self.interaction_pairs);
        self.layers[a].surface_dots = std::mem::take(&mut self.surface_dots);
        self.layers[a].surface_visible = self.surface_visible;
        self.layers[a].martini_ff = self.martini_ff.take();
        self.layers[a].bead_types = std::mem::take(&mut self.bead_types);
        self.layers[a].martini_visible = self.martini_visible;
        self.layers[a].selector_input = std::mem::take(&mut self.ui.selector_input);
        self.layers[a].ndx_selected_group_index = self.ui.ndx_selected_group_index;
        self.layers[a].ndx_visible = self.ui.ndx_visible;
        self.layers[a].ndx_selected_atom_count = self.ui.ndx_selected_atom_count;
    }

    /// Check `layers[idx]` out into the working fields, making it the active
    /// layer. Inverse of [`save_active_layer`](Self::save_active_layer); the
    /// caller is responsible for refreshing the view afterwards.
    fn load_active_layer(&mut self, idx: usize) {
        self.active_layer = idx;
        self.molecule = self.layers[idx].molecule.take();
        self.base_molecule = self.layers[idx].base_molecule.take();
        self.data = std::mem::take(&mut self.layers[idx].data);
        self.selection = std::mem::take(&mut self.layers[idx].selection);
        self.trajectory = std::mem::take(&mut self.layers[idx].trajectory);
        self.trajectory_path = self.layers[idx].trajectory_path.take();
        self.traj_ui = std::mem::take(&mut self.layers[idx].traj_ui);
        self.visibility = std::mem::take(&mut self.layers[idx].visibility);
        self.interaction_pairs = std::mem::take(&mut self.layers[idx].interaction_pairs);
        self.surface_dots = std::mem::take(&mut self.layers[idx].surface_dots);
        self.surface_visible = self.layers[idx].surface_visible;
        self.martini_ff = self.layers[idx].martini_ff.take();
        self.bead_types = std::mem::take(&mut self.layers[idx].bead_types);
        self.martini_visible = self.layers[idx].martini_visible;
        self.ui.selector_input = std::mem::take(&mut self.layers[idx].selector_input);
        self.ui.ndx_selected_group_index = self.layers[idx].ndx_selected_group_index;
        self.ui.ndx_visible = self.layers[idx].ndx_visible;
        self.ui.ndx_selected_atom_count = self.layers[idx].ndx_selected_atom_count;
    }

    /// Push the active layer to the viewport as the main molecule (or clear it
    /// when the layer is empty), restore its simulation cell, and redraw the
    /// non-active layers as spheres. Call after any active-layer swap.
    fn refresh_active_view(&mut self, focus: bool) {
        if self.molecule.is_some() {
            self.rebuild_viewport(focus);
        } else {
            // Empty layer: clear the main molecule and its index-based overlays.
            self.viewport
                .set_molecule(molecule_from_parts(Vec::new(), Vec::new()));
            // Bump the generation like rebuild_viewport does, so the renderer's
            // geometry cache drops the previously-active layer's atoms instead of
            // leaving them on screen.
            let bump = (self.view_revision % 4) + 1;
            for _ in 0..bump {
                let _ = self.viewport.update_positions(&[]);
            }
            self.view_revision = self.view_revision.wrapping_add(1);
            self.refresh_ndx_selection_state();
            self.refresh_interaction_pairs();
            self.refresh_martini_bead_state();
            // Empty layer: this also clears any stale highlight, since the
            // freshly loaded layer's selection is what gets projected.
            self.sync_selection_to_viewport();
        }
        self.refresh_surface_state();
        self.refresh_active_sim_cell();
        self.refresh_layer_overlays();
        // Apply the now-active layer's stored opacity to the main molecule.
        let active_opacity = self.layers[self.active_layer].opacity;
        self.viewport.set_molecule_opacity(active_opacity);
    }

    /// Restore the simulation-cell box for the active layer from its current
    /// trajectory frame, else its GRO box, else none.
    fn refresh_active_sim_cell(&mut self) {
        let box_diag = if let Some(frame) = self.trajectory.get(self.traj_ui.current_frame) {
            (
                frame.box_matrix[0][0],
                frame.box_matrix[1][1],
                frame.box_matrix[2][2],
            )
        } else if let Some(gro) = self.data.structure_file.as_ref().and_then(|s| s.gro()) {
            gro.box_line
        } else {
            (0.0, 0.0, 0.0)
        };
        self.viewport
            .set_state_by_type(SimulationCellRenderState::new(box_diag));
    }

    /// Rebuild the sphere geometry for every non-active, visible layer and push
    /// it to the viewport. Respects each layer's own residue-visibility filter.
    fn refresh_layer_overlays(&mut self) {
        let active = self.active_layer;
        let mut geoms: Vec<LayerOverlayGeom> = Vec::new();
        for (i, layer) in self.layers.iter().enumerate() {
            if i == active || !layer.visible {
                continue;
            }
            let Some(mol) = layer.molecule.as_ref() else {
                continue;
            };
            let filtered = layer.visibility.any_hidden();
            let atoms: Vec<OverlayAtom> = mol
                .atoms
                .iter()
                .filter(|atom| {
                    !filtered || {
                        let res = atom.res_name().unwrap_or("");
                        layer
                            .visibility
                            .res_visible
                            .get(res)
                            .copied()
                            .unwrap_or(true)
                    }
                })
                .map(|a| {
                    let (r, g, b, _) = default_color_fn(a, false);
                    OverlayAtom {
                        position: a.position,
                        radius: ball_stick_radius(&a.element, false),
                        // Element colour, faded by the layer's opacity (alpha).
                        color: (r, g, b, layer.opacity),
                    }
                })
                .collect();
            if !atoms.is_empty() {
                geoms.push(LayerOverlayGeom { atoms });
            }
        }
        self.viewport
            .set_state_by_type(LayerOverlayState { layers: geoms });
    }

    /// Switch which layer is active (drawn as the main molecule). No-op when the
    /// index is already active or out of range.
    pub fn set_active_layer(&mut self, idx: usize) {
        if idx >= self.layers.len() || idx == self.active_layer {
            return;
        }
        self.save_active_layer();
        self.load_active_layer(idx);
        self.refresh_active_view(false);
        self.set_status(format!("Layer {} active", idx + 1));
    }

    /// Create a new empty layer, make it active, and prompt to load a structure
    /// into it. The normal file loaders then target this layer.
    pub fn add_layer(&mut self) {
        self.save_active_layer();
        let name = format!("Layer {}", self.layers.len() + 1);
        let color = LAYER_PALETTE[self.layers.len() % LAYER_PALETTE.len()];
        self.layers.push(Layer::new(name, color));
        let idx = self.layers.len() - 1;
        self.load_active_layer(idx);
        self.refresh_active_view(false);
        self.open_structure_into_active_layer();
    }

    /// Remove a layer, keeping at least one, and re-activate a neighbour.
    pub fn remove_layer(&mut self, idx: usize) {
        if idx >= self.layers.len() {
            return;
        }
        // Persist the active layer so every slot holds its own full state, then
        // drop the requested one.
        self.save_active_layer();
        self.layers.remove(idx);
        if self.layers.is_empty() {
            self.layers.push(Layer::new("Layer 1".to_string(), LAYER_PALETTE[0]));
        }
        let new_active = if self.active_layer == idx {
            idx.min(self.layers.len() - 1)
        } else if self.active_layer > idx {
            self.active_layer - 1
        } else {
            self.active_layer
        };
        self.load_active_layer(new_active);
        self.refresh_active_view(false);
    }

    /// Open a structure/topology file dialog and load the pick into the active
    /// (typically just-created) layer via the existing loaders.
    fn open_structure_into_active_layer(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("Structures", &["gro", "pdb", "ent", "cif", "mol2"])
            .add_filter("Topology", &["top", "itp"])
            .set_title("Load structure into new layer")
            .pick_file()
        {
            self.load_structure_path(path);
        }
    }

    /// Dispatch a path to the right loader by extension (all operate on the
    /// active layer).
    fn load_structure_path(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "gro" => self.load_gro_file_only(path),
            "top" | "itp" => self.load_top_file_only(path),
            _ => self.load_file(path),
        }
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    pub fn layer_names(&self) -> Vec<String> {
        (0..self.layers.len()).map(|i| self.layer_label(i)).collect()
    }

    pub fn active_layer_index(&self) -> usize {
        self.active_layer
    }

    pub fn active_layer_name(&self) -> String {
        self.layer_label(self.active_layer)
    }

    pub fn layer_name(&self, idx: usize) -> Option<String> {
        (idx < self.layers.len()).then(|| self.layer_label(idx))
    }

    /// Display label for a layer: its loaded structure's file name when present,
    /// otherwise the layer's default `"Layer N"` name. The active layer's file
    /// path lives in the working `data`; parked layers keep theirs in the slot.
    fn layer_label(&self, idx: usize) -> String {
        let data = if idx == self.active_layer {
            &self.data
        } else {
            match self.layers.get(idx) {
                Some(l) => &l.data,
                None => return String::new(),
            }
        };
        data.structure_file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                self.layers
                    .get(idx)
                    .map(|l| l.name.clone())
                    .unwrap_or_default()
            })
    }

    /// Atom count of a layer — from the working molecule for the active layer,
    /// from the parked slot otherwise.
    pub fn layer_atom_count(&self, idx: usize) -> usize {
        if idx == self.active_layer {
            self.molecule.as_ref().map(|m| m.atoms.len()).unwrap_or(0)
        } else {
            self.layers
                .get(idx)
                .and_then(|l| l.molecule.as_ref())
                .map(|m| m.atoms.len())
                .unwrap_or(0)
        }
    }

    pub fn layer_visible(&self, idx: usize) -> bool {
        self.layers.get(idx).map(|l| l.visible).unwrap_or(false)
    }

    /// The layer's identity colour (swatch / eye tint in the LAYERS panel).
    pub fn layer_color(&self, idx: usize) -> [f32; 3] {
        self.layers.get(idx).map(|l| l.color).unwrap_or([0.5, 0.5, 0.5])
    }

    /// The layer's overlay opacity in `0.0..=1.0`.
    pub fn layer_opacity(&self, idx: usize) -> f32 {
        self.layers.get(idx).map(|l| l.opacity).unwrap_or(1.0)
    }

    /// Set the layer's opacity (clamped to `0.0..=1.0`). For the active layer it
    /// fades the main molecule (atoms + bonds) via the viewport; for any other
    /// layer it fades that layer's sphere overlay.
    pub fn set_layer_opacity(&mut self, idx: usize, opacity: f32) {
        let clamped = opacity.clamp(0.0, 1.0);
        let Some(layer) = self.layers.get_mut(idx) else {
            return;
        };
        if layer.opacity == clamped {
            return;
        }
        layer.opacity = clamped;
        if idx == self.active_layer {
            self.viewport.set_molecule_opacity(clamped);
        } else {
            self.refresh_layer_overlays();
        }
    }

    pub fn set_layer_visible(&mut self, idx: usize, visible: bool) {
        if let Some(layer) = self.layers.get_mut(idx) {
            if layer.visible != visible {
                layer.visible = visible;
                self.refresh_layer_overlays();
            }
        }
    }

    /// Whether the currently loaded structure carries a dot surface.
    pub fn has_surface(&self) -> bool {
        !self.surface_dots.is_empty()
    }

    /// Number of dots in the loaded surface (0 when none).
    pub fn surface_dot_count(&self) -> usize {
        self.surface_dots.len()
    }

    pub fn surface_visible(&self) -> bool {
        self.surface_visible
    }

    pub fn set_surface_visible(&mut self, visible: bool) {
        if self.surface_visible != visible {
            self.surface_visible = visible;
            self.refresh_surface_state();
        }
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
        let orig_indices = if self.ui.ndx_visible {
            self.current_ndx_indices()
        } else {
            Vec::new()
        };

        // Map into viewport space and drop atoms that are currently hidden.
        let atom_indices: Vec<usize> = orig_indices
            .iter()
            .filter_map(|&orig| self.visibility.to_view(orig))
            .collect();

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
        let gro_path = self.data.structure_file_path.clone().filter(|_| {
            matches!(self.data.structure_file, Some(StructureFile::Gro(_)))
        });
        let current_file_path = self.data.structure_file_path.clone().filter(|_| {
            matches!(self.data.structure_file, Some(StructureFile::Pdb(_)))
        });
        let ndx_path = self.data.ndx_file_path.clone();

        let mut reloaded_any = false;

        match (top_path, gro_path) {
            (Some(top), Some(gro)) => {
                self.load_top_and_gro_for_resname_sync(top, gro);
                reloaded_any = true;
            }
            (Some(top), None) => {
                self.load_top_file_only(top);
                reloaded_any = true;
            }
            (None, Some(gro)) => {
                self.load_gro_file_only(gro);
                reloaded_any = true;
            }
            (None, None) => {
                if let Some(path) = current_file_path {
                    self.load_file(path);
                    reloaded_any = true;
                }
            }
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
        self.sync_selection_to_viewport();
    }

    fn toggle_hbond_selection(&mut self) {
        self.selection.with_hbond_chk = !self.selection.with_hbond_chk;
    }

    /// Push the app-side atom selection (`selection.selected_atom_indices`, in
    /// original-molecule index space) to the viewport's red highlight, projected
    /// into the current viewport-index space and dropping atoms hidden by the
    /// residue filter. This is the single point where selection state reaches the
    /// rendered view, so every selection path (click, selector expression,
    /// "Select Between", clear) stays visually consistent.
    fn sync_selection_to_viewport(&mut self) {
        let selected_atoms: Vec<usize> = self
            .selection
            .selected_atom_indices
            .iter()
            .filter_map(|&orig| self.visibility.to_view(orig))
            .collect();
        self.viewport.set_state_by_type(SelectedAtomRenderState {
            selected_atoms,
            color: [1.0, 0.0, 0.0, 1.0],
        });
    }

    /// Fold any atom clicks the viewport captured since the last frame into the
    /// app-side selection (toggling each), then reflect the result in the view.
    /// Bridges the viewport's click events, which arrive in viewport-index space,
    /// to the selection stored in original-index space.
    fn process_atom_clicks(&mut self) {
        let clicks: Vec<usize> = match self.clicked_atoms.lock() {
            Ok(mut g) if !g.is_empty() => std::mem::take(&mut *g),
            _ => return,
        };
        for view_idx in clicks {
            let orig = self.visibility.to_orig(view_idx);
            let now_selected = self.toggle_selected_atom(orig);
            if now_selected && self.selection.with_hbond_chk {
                self.add_connected_hydrogens(orig);
            }
        }
        self.sync_selection_to_viewport();
    }

    fn atom_name_at(&self, atom_index: usize) -> Option<String> {
        if let Some(mol) = &self.molecule {
            if let Some(atom) = mol.atoms.get(atom_index) {
                if let Some(name) = atom.name() {
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

        if let Some(gro) = self.data.structure_file.as_ref().and_then(|s| s.gro()) {
            if let Some(atom) = gro.atoms().nth(atom_index) {
                let name = atom.atom_name.trimmed();
                if !name.is_empty() {
                    return Some(name.to_ascii_uppercase());
                }
            }
        }

        if let Some(pdb) = self.data.structure_file.as_ref().and_then(|s| s.pdb()) {
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
        self.sync_selection_to_viewport();

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
                ctrl && !shift && i.key_pressed(egui::Key::T),
                ctrl && !shift && i.key_pressed(egui::Key::G),
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
        if shortcuts.7 {
            self.open_top_file();
        }
        if shortcuts.8 {
            self.open_gro_file();
        }
    }

    fn hovered_atom_info(&self, atom_index: usize) -> Option<String> {
        let mut atom_name: Option<String> = None;
        let mut res_name: Option<String> = None;

        if let Some(mol) = &self.molecule {
            if let Some(atom) = mol.atoms.get(atom_index) {
                if let Some(name) = atom.name() {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() {
                        atom_name = Some(trimmed.to_string());
                    }
                }

                if atom_name.is_none() && !atom.element.trim().is_empty() {
                    atom_name = Some(atom.element.trim().to_string());
                }

                if let Some(name) = atom.res_name() {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() {
                        res_name = Some(trimmed.to_string());
                    }
                }
            }
        }

        if let Some(pdb) = self.data.structure_file.as_ref().and_then(|s| s.pdb()) {
            if let Some(atom) = pdb.atoms().nth(atom_index) {
                if atom_name.is_none() && !atom.name.trim().is_empty() {
                    atom_name = Some(atom.name.trim().to_string());
                }
                if res_name.is_none() && !atom.res_name.trim().is_empty() {
                    res_name = Some(atom.res_name.trim().to_string());
                }
            }
        }

        if let Some(gro) = self.data.structure_file.as_ref().and_then(|s| s.gro()) {
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

        let gro_resnames = self.data.structure_file.as_ref().and_then(|s| s.gro()).map(|gro| {
            gro.atoms()
                .map(|atom| atom.res_name.trimmed().to_string())
                .collect::<Vec<_>>()
        });

        let pdb_resnames = self.data.structure_file.as_ref().and_then(|s| s.pdb()).map(|pdb| {
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
                atom.meta
                    .get_or_insert_with(|| Box::new(AtomMeta::default()))
                    .res_name = Some(name);
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
            .pick_file()
        {
            self.load_file(path);
        }
    }

    pub fn open_top_file(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("TOP/ITP Files", &["top", "itp"])
            .set_title("Select TOP file")
            .pick_file()
        {
            self.load_top_file_only(path);
        }
    }

    pub fn open_gro_file(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("GRO Files", &["gro"])
            .set_title("Select GRO file")
            .pick_file()
        {
            self.load_gro_file_only(path);
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

    fn generate_and_set_molecule_from_stored_files(&mut self) -> bool {
        let top = match self.data.top_file.clone() {
            Some(t) => t,
            None => return false,
        };
        let gro = match self.data.structure_file.as_ref().and_then(|s| s.gro()) {
            Some(g) => g.clone(),
            None => return false,
        };

        // A force-field-only `.itp` (e.g. the Martini master file) has no
        // molecule template, so there is no connectivity to apply — build the
        // molecule straight from the GRO (distance-inferred bonds) instead of
        // handing `generate_molecule_with_gro` an empty bond list.
        if top.expanded_atom_types().is_empty() {
            let boxsize = gro.box_line;
            let mol = gro.to_molecule_with_metadata(true, None);
            self.interaction_pairs.clear();
            self.surface_dots.clear();
            self.set_molecule_and_frame(mol);
            self.viewport
                .set_state_by_type(SimulationCellRenderState::new(boxsize));
            return true;
        }

        match top.generate_molecule_with_gro(&gro) {
            Ok((molecule, interaction_pairs)) => {
                let boxsize = gro.box_line;
                // Stored in original index space; rebuild_viewport() remaps and
                // pushes them whenever the visible set changes.
                self.interaction_pairs = interaction_pairs;
                self.surface_dots.clear();
                self.set_molecule_and_frame(molecule);
                self.viewport
                    .set_state_by_type(SimulationCellRenderState::new(boxsize));
                true
            }
            Err(err) => {
                self.set_status(err);
                false
            }
        }
    }

    fn update_loaded_summary(&mut self) {
        let top_name = self
            .data
            .top_file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_string);
        let coord_name = self
            .data
            .structure_file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_string);

        let summary = match (top_name, coord_name) {
            (Some(t), Some(c)) => format!("TOP: {}\n{}", t, c),
            (Some(t), None) => format!("TOP: {} (no coord file)", t),
            (None, Some(c)) => c,
            _ => String::new(),
        };
        self.set_loaded_summary(summary);
    }

    /// The primary structure's type badge (`GRO`/`PDB`) and file name, for the
    /// left-panel header. `None` when the active layer has no structure loaded.
    pub fn structure_badge(&self) -> Option<&'static str> {
        match self.data.structure_file {
            Some(StructureFile::Gro(_)) => Some("GRO"),
            Some(StructureFile::Pdb(_)) => Some("PDB"),
            None => None,
        }
    }

    pub fn structure_file_name(&self) -> Option<String> {
        self.data
            .structure_file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_string)
    }

    /// Every file loaded into the active layer, as badge rows, so the user can
    /// see the whole loaded state at a glance. The primary structure comes first
    /// (also shown as the header hero); topology, index, trajectory, the dot
    /// surface and any Martini force field follow. Overlay surfaces have their
    /// own panel and are not repeated here.
    pub fn loaded_files(&self) -> Vec<LoadedFileRow> {
        let mut rows = Vec::new();
        let file_name = |p: &Option<PathBuf>| -> Option<String> {
            p.as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(str::to_string)
        };

        if let (Some(badge), Some(name)) = (self.structure_badge(), self.structure_file_name()) {
            rows.push(LoadedFileRow {
                badge,
                name,
                detail: format!("{} atoms", self.atom_count()),
            });
        }

        if let Some(name) = file_name(&self.data.top_file_path) {
            let badge = if name.to_ascii_lowercase().ends_with(".itp") {
                "ITP"
            } else {
                "TOP"
            };
            rows.push(LoadedFileRow {
                badge,
                name,
                detail: "topology".to_string(),
            });
        }

        if let Some(name) = file_name(&self.data.ndx_file_path) {
            let groups = self
                .data
                .ndx_file
                .as_ref()
                .map(|n| n.groups.len())
                .unwrap_or(0);
            rows.push(LoadedFileRow {
                badge: "NDX",
                name,
                detail: format!("{groups} groups"),
            });
        }

        if let Some(name) = file_name(&self.trajectory_path) {
            rows.push(LoadedFileRow {
                badge: "XTC",
                name,
                detail: format!("{} frames", self.trajectory.len()),
            });
        }

        if !self.surface_dots.is_empty() {
            rows.push(LoadedFileRow {
                badge: "SURF",
                name: "dot surface".to_string(),
                detail: format!("{} dots", self.surface_dots.len()),
            });
        }

        if let Some(ff) = self.martini_ff.as_ref() {
            rows.push(LoadedFileRow {
                badge: "FF",
                name: "Martini".to_string(),
                detail: format!("{} bead types", ff.bead_type_count()),
            });
        }

        rows
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

        self.data.top_file = Some(top);
        let martini_types = self.try_load_martini_ff(&top_path);
        self.data.top_file_path = Some(top_path);
        self.data.structure_file = Some(StructureFile::Gro(gro));
        self.data.structure_file_path = Some(gro_path);

        self.generate_and_set_molecule_from_stored_files();
        self.update_loaded_summary();
        self.mark_clean();
        match martini_types {
            Some(n) => self.set_status(format!("Loaded Martini FF + GRO ({} bead types)", n)),
            None => self.set_status("Loaded TOP+GRO"),
        }
        self.post_load_cleanup();
    }

    fn load_top_file_only(&mut self, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let top = match TopFile::load_from_path(&path) {
            Ok(t) => t,
            Err(err) => {
                self.set_status(err);
                return;
            }
        };

        self.data.top_file = Some(top);
        let martini_types = self.try_load_martini_ff(&path);
        self.data.top_file_path = Some(path);

        let has_gro = self
            .data
            .structure_file
            .as_ref()
            .and_then(|s| s.gro())
            .is_some();

        if has_gro {
            self.generate_and_set_molecule_from_stored_files();
            self.update_loaded_summary();
            self.mark_clean();
            self.post_load_cleanup();
            match martini_types {
                Some(n) => self.set_status(format!(
                    "Loaded Martini force field: {} ({} bead types)",
                    file_name, n
                )),
                None => self.set_status(format!("Loaded TOP: {}", file_name)),
            }
        } else {
            self.update_loaded_summary();
            self.mark_clean();
            match martini_types {
                Some(n) => self.set_status(format!(
                    "Loaded Martini force field: {} ({} bead types). Load a GRO to display beads.",
                    file_name, n
                )),
                None => self.set_status(format!(
                    "Loaded TOP: {}. Load a GRO file to display the molecule.",
                    file_name
                )),
            }
        }
    }

    fn load_gro_file_only(&mut self, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let gro = match GroFile::load_from_path(&path) {
            Ok(g) => g,
            Err(_) => {
                self.set_status("Failed to read GRO file");
                return;
            }
        };

        let boxsize = gro.box_line;
        self.data.structure_file = Some(StructureFile::Gro(gro));
        self.data.structure_file_path = Some(path);
        self.viewport
            .set_state_by_type(SimulationCellRenderState::new(boxsize));

        if self.data.top_file.is_some() {
            self.generate_and_set_molecule_from_stored_files();
        } else {
            let mol = self
                .data
                .structure_file
                .as_ref()
                .and_then(|s| s.gro())
                .unwrap()
                .to_molecule_with_metadata(true, None);
            self.interaction_pairs.clear();
            self.surface_dots.clear();
            self.set_molecule_and_frame(mol);
        }

        self.update_loaded_summary();
        self.mark_clean();
        self.set_status(format!("Loaded GRO: {}", file_name));
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
                self.interaction_pairs.clear();
                self.surface_dots = pdb.surface_dots();
                self.set_molecule_and_frame(mol);
                self.data.clear_structures();
                self.data.structure_file = Some(StructureFile::Pdb(pdb));
                self.data.structure_file_path = Some(path);
                self.update_loaded_summary();
                self.mark_clean();
                self.set_status(format!("Loaded PDB: {}", file_name));
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
                self.interaction_pairs.clear();
                self.surface_dots.clear();
                self.set_molecule_and_frame(mol);
                self.data.clear_structures();
                self.data.structure_file = Some(StructureFile::Pdb(pdb_from_mol2));
                self.data.structure_file_path = Some(path);
                self.update_loaded_summary();
                self.mark_clean();
                self.set_status(format!("Loaded MOL2: {}", file_name));
            }
            Err(_) => self.set_status("Failed to load MOL2 file"),
        }
    }

    fn set_molecule_and_frame(&mut self, molecule: Molecule) {
        self.molecule = Some(molecule);
        // Syncing to the viewport is handled by post_load_cleanup() — callers are responsible.
    }

    pub fn open_xtc_file(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("XTC Trajectory", &["xtc"])
            .set_title("Select XTC trajectory file")
            .pick_file()
        {
            self.load_xtc_file(path);
        }
    }

    fn load_xtc_file(&mut self, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let xtc = match XtcFile::load_from_path(&path) {
            Ok(x) => x,
            Err(e) => {
                self.set_status(format!("Failed to load XTC: {e}"));
                return;
            }
        };

        if xtc.frames.is_empty() {
            self.set_status("XTC file contains no frames");
            return;
        }

        // Validate atom count against current molecule
        if let Some(mol) = &self.molecule {
            if mol.atoms.len() != xtc.natoms {
                self.set_status(format!(
                    "XTC atom count ({}) does not match loaded structure ({})",
                    xtc.natoms,
                    mol.atoms.len()
                ));
                return;
            }
            self.base_molecule = Some(mol.clone());
        } else {
            // No reference structure: create minimal atoms (positions only, no bonds/names)
            let atoms = (0..xtc.natoms)
                .map(|i| view_atom(Vec3::new(0.0, 0.0, 0.0), "C", i, None))
                .collect();
            self.base_molecule = Some(molecule_from_parts(atoms, Vec::new()));
        }

        let frame_count = xtc.frames.len();
        self.trajectory = xtc.frames;
        self.trajectory_path = Some(path);
        self.traj_ui.current_frame = 0;
        self.traj_ui.is_playing = false;

        self.apply_trajectory_frame(0);
        self.set_status(format!("Loaded XTC: {} ({} frames)", file_name, frame_count));
    }

    fn apply_trajectory_frame(&mut self, idx: usize) {
        let Some(frame) = self.trajectory.get(idx) else {
            return;
        };

        // Update box for simulation cell render
        let box_diag = (
            frame.box_matrix[0][0],
            frame.box_matrix[1][1],
            frame.box_matrix[2][2],
        );

        let positions: Vec<Vec3> = frame
            .positions
            .iter()
            .map(|p| Vec3::new(p[0], p[1], p[2]))
            .collect();

        self.apply_positions(positions, box_diag);
        self.traj_ui.current_frame = idx;
        self.traj_ui.interp_sub = 0;
    }

    /// Display a linearly-interpolated frame `t` (0..1) of the way from real
    /// frame `idx` to `idx + 1` (wrapping at the end). Used by smoothed
    /// playback; the integer base frame (`current_frame`) is left unchanged.
    fn apply_interpolated_frame(&mut self, idx: usize, t: f32) {
        let n = self.trajectory.len();
        if n == 0 {
            return;
        }
        let next = (idx + 1) % n;
        let (Some(a), Some(b)) = (self.trajectory.get(idx), self.trajectory.get(next)) else {
            return;
        };
        if a.positions.len() != b.positions.len() {
            // Atom count changed between frames: fall back to the base frame.
            return self.apply_trajectory_frame(idx);
        }

        let lerp = |x: f32, y: f32| x + (y - x) * t;
        let box_diag = (
            lerp(a.box_matrix[0][0], b.box_matrix[0][0]),
            lerp(a.box_matrix[1][1], b.box_matrix[1][1]),
            lerp(a.box_matrix[2][2], b.box_matrix[2][2]),
        );

        // Minimum-image (nearest-image) interpolation: when an atom wraps across
        // a periodic boundary between the two frames its raw displacement spans
        // almost the whole box, which plain lerp would draw as a fast sweep
        // across the cell. Instead, fold each per-axis displacement into
        // [-L/2, L/2] so we interpolate along the shortest path, then start from
        // frame `a`'s coordinate. `L = 0` (no box on that axis) disables the
        // correction for that axis.
        let box_len = [a.box_matrix[0][0], a.box_matrix[1][1], a.box_matrix[2][2]];
        let min_image = |pa: f32, pb: f32, l: f32| -> f32 {
            let mut d = pb - pa;
            if l > 0.0 {
                d -= l * (d / l).round();
            }
            pa + d * t
        };
        let positions: Vec<Vec3> = a
            .positions
            .iter()
            .zip(b.positions.iter())
            .map(|(pa, pb)| {
                Vec3::new(
                    min_image(pa[0], pb[0], box_len[0]),
                    min_image(pa[1], pb[1], box_len[1]),
                    min_image(pa[2], pb[2], box_len[2]),
                )
            })
            .collect();

        self.apply_positions(positions, box_diag);
    }

    /// Push a set of atom positions (and simulation-cell box) into the viewport,
    /// reusing the current molecule's bonds/metadata/camera when the atom count
    /// matches. Shared by exact and interpolated frame display.
    fn apply_positions(&mut self, positions: Vec<Vec3>, box_diag: (f32, f32, f32)) {
        self.viewport
            .set_state_by_type(SimulationCellRenderState::new(box_diag));

        let same_atom_count = self
            .molecule
            .as_ref()
            .map(|mol| mol.atoms.len() == positions.len())
            .unwrap_or(false);

        if same_atom_count {
            // Smooth playback: move atoms in place, keeping bonds, metadata and the
            // user's camera. moleucle_3dview_rs 0.6 updates the GPU buffers without
            // rebuilding the molecule.
            if let Some(mol) = &mut self.molecule {
                for (atom, &pos) in mol.atoms.iter_mut().zip(positions.iter()) {
                    atom.position = pos;
                }
            }
            // The viewport may hold a filtered subset; feed it positions in
            // viewport-index order.
            if self.visibility.is_filtered() {
                let view_positions: Vec<Vec3> = self
                    .visibility
                    .view_to_orig
                    .iter()
                    .map(|&orig| positions[orig])
                    .collect();
                let _ = self.viewport.update_positions(&view_positions);
            } else {
                let _ = self.viewport.update_positions(&positions);
            }
        } else if let Some(base) = self.base_molecule.clone() {
            // First frame (or the molecule was swapped): establish the molecule and
            // fit the camera once.
            let mut mol = base;
            for (atom, &pos) in mol.atoms.iter_mut().zip(positions.iter()) {
                atom.position = pos;
            }
            self.molecule = Some(mol);
            self.refresh_res_names();
            self.sync_viewer_molecule();
        }
    }

    pub fn toggle_playback(&mut self) {
        self.traj_ui.is_playing = !self.traj_ui.is_playing;
    }

    pub fn go_to_first_frame(&mut self) {
        self.traj_ui.is_playing = false;
        self.apply_trajectory_frame(0);
    }

    pub fn go_to_last_frame(&mut self) {
        self.traj_ui.is_playing = false;
        let last = self.trajectory.len().saturating_sub(1);
        self.apply_trajectory_frame(last);
    }

    pub fn step_frame(&mut self, delta: i32) {
        self.traj_ui.is_playing = false;
        if self.trajectory.is_empty() {
            return;
        }
        let n = self.trajectory.len() as i32;
        let next = ((self.traj_ui.current_frame as i32 + delta).rem_euclid(n)) as usize;
        self.apply_trajectory_frame(next);
    }

    fn advance_trajectory_if_playing(&mut self, current_time: f64) {
        if !self.traj_ui.is_playing || self.trajectory.is_empty() {
            return;
        }
        // With smoothing each real-frame transition is split into `steps`
        // sub-steps. To keep the trajectory playing at the same wall-clock speed
        // (playback_fps real frames per second) regardless of smoothing, tick
        // the sub-steps `steps` times as fast.
        let steps = self.traj_ui.interp_steps.max(1);
        let interval = 1.0 / (self.traj_ui.playback_fps as f64 * steps as f64);
        if current_time - self.traj_ui.last_advance_time < interval {
            return;
        }
        self.traj_ui.last_advance_time = current_time;

        let sub = self.traj_ui.interp_sub + 1;
        if sub >= steps {
            // Cross into the next real frame; resets interp_sub to 0.
            let next = (self.traj_ui.current_frame + 1) % self.trajectory.len();
            self.apply_trajectory_frame(next);
        } else {
            let t = sub as f32 / steps as f32;
            self.apply_interpolated_frame(self.traj_ui.current_frame, t);
            self.traj_ui.interp_sub = sub;
        }
    }

    pub fn trajectory_frame_count(&self) -> usize {
        self.trajectory.len()
    }

    pub fn trajectory_current_frame(&self) -> usize {
        self.traj_ui.current_frame
    }

    pub fn trajectory_current_time(&self) -> f32 {
        self.trajectory
            .get(self.traj_ui.current_frame)
            .map(|f| f.time)
            .unwrap_or(0.0)
    }

    pub fn trajectory_is_playing(&self) -> bool {
        self.traj_ui.is_playing
    }

    pub fn trajectory_playback_fps(&mut self) -> &mut f32 {
        &mut self.traj_ui.playback_fps
    }

    /// Smoothing subdivision: `1` = off, `N` = insert `N - 1` interpolated
    /// frames between each pair of real frames during playback.
    pub fn trajectory_interp_steps(&mut self) -> &mut u32 {
        &mut self.traj_ui.interp_steps
    }

    pub fn set_trajectory_frame(&mut self, idx: usize) {
        self.apply_trajectory_frame(idx);
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped_paths: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });

        self.load_paths(dropped_paths);
    }

    /// Load a batch of files, routing each by extension exactly as a drag-and-drop
    /// does: a `.top`+`.gro` pair is cross-loaded for residue-name sync, otherwise
    /// the single most relevant file is loaded. Shared by drag-and-drop and the
    /// command-line entry point (files passed as arguments / opened via a file
    /// association). A no-op on an empty list.
    pub fn load_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }

        let mut top_path: Option<PathBuf> = None;
        let mut gro_path: Option<PathBuf> = None;
        let mut ndx_path: Option<PathBuf> = None;
        let mut xtc_path: Option<PathBuf> = None;
        let mut other_path: Option<PathBuf> = None;

        for path in &paths {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                match ext.to_ascii_lowercase().as_str() {
                    "top" | "itp" => top_path = Some(path.clone()),
                    "gro" => gro_path = Some(path.clone()),
                    "ndx" => ndx_path = Some(path.clone()),
                    "xtc" => xtc_path = Some(path.clone()),
                    _ => other_path = Some(path.clone()),
                }
            }
        }

        if let (Some(top), Some(gro)) = (top_path.clone(), gro_path.clone()) {
            self.load_top_and_gro_for_resname_sync(top, gro);
            return;
        }

        if let Some(top) = top_path {
            self.load_top_file_only(top);
            return;
        }

        if let Some(gro) = gro_path {
            self.load_gro_file_only(gro);
            return;
        }

        if let Some(path) = xtc_path {
            self.load_xtc_file(path);
            return;
        }

        if let Some(path) = ndx_path {
            self.load_ndx_file(path);
            return;
        }

        if let Some(path) = other_path {
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

        self.sync_selection_to_viewport();
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

        if let Some(pdb) = self.data.structure_file.as_mut().and_then(|s| s.pdb_mut()) {
            let mut atoms_vec: Vec<&mut AtomRecord> = pdb.atoms_mut().collect();
            for &idx in &indices_to_update {
                if let Some(atom) = atoms_vec.get_mut(idx) {
                    atom.res_name = new_name.clone();
                }
            }
        }

        if let Some(gro) = self.data.structure_file.as_mut().and_then(|s| s.gro_mut()) {
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
        self.sync_selection_to_viewport();
        self.set_status("Residue names updated");
    }

    fn export_structure(&mut self) {
        if self.data.top_file.is_none() && self.data.structure_file.is_none() {
            if let Some(mol) = &self.molecule {
                self.data.structure_file = Some(StructureFile::Pdb(PdbFile::from_molecule(mol)));
            }
        }

        if let Some(path) = FileDialog::new().save_file() {
            let saved = if let Some(top) = &self.data.top_file {
                let content = top.dump();
                std::fs::write(&path, content).is_ok()
            } else if let Some(gro) = self.data.structure_file.as_ref().and_then(|s| s.gro()) {
                let content = gro.dump();
                std::fs::write(&path, content).is_ok()
            } else if let Some(pdb) = self.data.structure_file.as_mut().and_then(|s| s.pdb_mut()) {
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

        // Update hover info from the previous frame's viewport pick result. The
        // picked index is in viewport space; translate it to the full molecule.
        let last_hovered = self.hovered_atom.lock().ok().and_then(|mut g| g.take());
        self.ui.hovered_atom_info = last_hovered
            .map(|view| self.visibility.to_orig(view))
            .and_then(|atom| self.hovered_atom_info(atom))
            .unwrap_or_else(|| "Hover an atom for details".to_string());

        self.handle_dropped_files(&ctx);
        self.handle_keyboard_shortcuts(&ctx);
        // Fold clicks the viewport captured last frame into the selection before
        // the panels (which read the selection count / enablement) are drawn.
        self.process_atom_clicks();

        // Advance trajectory playback
        if self.traj_ui.is_playing {
            let t = ctx.input(|i| i.time);
            self.advance_trajectory_if_playing(t);
            ctx.request_repaint();
        }

        // egui 0.35: panels are shown into the root `ui`, not the context.
        app_ui::render_menu_bar(self, ui);
        app_ui::render_bottom_status_bar(self, ui);
        app_ui::render_left_panel(self, ui);
        app_ui::render_overlay_panel(self, ui);
        app_ui::render_bottom_dock(self, ui);
        app_ui::render_edit_dialog(self, &ctx);

        // Top-left overlay text: filename · frame.
        let overlay_label = if self.trajectory_frame_count() > 0 {
            format!(
                "{} · frame {}",
                self.data.loaded_summary,
                self.trajectory_current_frame() + 1
            )
        } else {
            self.data.loaded_summary.clone()
        };

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(app_ui::theme::BG))
            .show(ui, |ui| {
                let Some(render_state) = &self.render_state else {
                    ui.heading("WGPU backend is unavailable");
                    ui.label("Start with the wgpu backend enabled in eframe.");
                    return;
                };

                if let Err(err) = self.viewport.show(ui, render_state) {
                    ui.colored_label(egui::Color32::RED, format!("Render failed: {err}"));
                }

                // Corner overlays drawn on top of the 3D view.
                let rect = ui.max_rect();
                let painter = ui.painter().clone();
                painter.text(
                    rect.left_top() + egui::vec2(14.0, 12.0),
                    egui::Align2::LEFT_TOP,
                    &overlay_label,
                    egui::FontId::proportional(11.0),
                    app_ui::theme::MUTED2,
                );
                painter.text(
                    rect.left_bottom() + egui::vec2(14.0, -12.0),
                    egui::Align2::LEFT_BOTTOM,
                    "X · Y · Z",
                    egui::FontId::proportional(10.0),
                    app_ui::theme::MUTED2,
                );

                // Top-right reset-view button.
                let btn_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.right() - 40.0, rect.top() + 12.0),
                    egui::vec2(28.0, 28.0),
                );
                if ui
                    .put(btn_rect, egui::Button::new("⟳"))
                    .on_hover_text("Reset view")
                    .clicked()
                {
                    self.viewport.focus_on_molecule_center();
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

        // The viewport's click events fire during `viewport.show()` above, i.e.
        // after `process_atom_clicks` already ran this frame. Schedule another
        // frame so a just-captured click is folded into the selection promptly
        // rather than waiting for an unrelated repaint.
        let clicks_pending = self
            .clicked_atoms
            .lock()
            .map(|g| !g.is_empty())
            .unwrap_or(false);
        if clicks_pending {
            ctx.request_repaint();
        }
    }

    fn on_exit(&mut self) {
        if let Some(render_state) = &self.render_state {
            self.viewport.free_egui_texture(render_state);
        }
    }
}
