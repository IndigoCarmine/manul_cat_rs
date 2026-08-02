use crate::component::ComponentState;
use crate::image_export::{self, ExportSettings};
use crate::parsing::{
    AtomRecord, GroFile, MartiniForceField, Mol2File, NdxFile, PdbFile, SURFACE_RES_NAME, TopFile,
    XtcFile, XtcFrame,
};
use crate::selection::{
    AtomTable, EvalCtx, HELP_TEXT, Statement, Targets, evaluate, parse_statement, to_indices,
};
use crate::view_rs::{To3dViewMolecule, molecule_from_parts, view_atom};
use eframe::egui::{self};
use lin_alg::f32::Vec3;
use moleucle_3dview_rs::molecule::{AtomMeta, Bond};
use moleucle_3dview_rs::{
    Atom, AtomGroup, AtomGroupRender, AtomGroupState, AtomPairRender, AtomPairState, AxesRender,
    AxesState, ImageExportRequest, InteractiveMoleculeViewport, Molecule, OverlaySphere,
    PointCloudLayer, PointCloudRender, PointCloudState, SelectedAtomRender,
    SelectedAtomRenderState, SimulationCellRender, SimulationCellState, SphereSet, SphereSetRender,
    SphereSetState, ViewPortEvent, ball_stick_radius, default_color_fn,
};
use rfd::FileDialog;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

/// Which loader consumes the path(s) a background picker thread returns.
///
/// Native file dialogs block the thread they run on, so we run them off the UI
/// thread and tag the result with the loader that should handle it once
/// `update`/`ui` polls it back. See [`KuromameApp::spawn_pick`].
///
/// `Copy` so the tag can be handed to a background load worker (and echoed back
/// on the finished result) without ceremony — it is a fieldless enum.
#[derive(Clone, Copy)]
enum PickKind {
    /// Structure into the active layer via [`KuromameApp::load_file`].
    Structure,
    /// TOP/ITP topology only.
    Top,
    /// GRO structure only.
    Gro,
    /// NDX index groups.
    Ndx,
    /// XTC trajectory.
    Xtc,
    /// Overlay dot-surface PDB.
    OverlaySurface,
    /// Structure into a *new* layer, dispatched by extension.
    StructureLayer,
    /// A TOP + GRO pair (paths ordered `[top, gro]`) for resname sync.
    TopGroPair,
}

/// Result handed back from a background file-picker thread. `paths` is empty
/// when the user cancelled the dialog.
struct PickedFiles {
    kind: PickKind,
    paths: Vec<PathBuf>,
}

// --- Async file loading -----------------------------------------------------
//
// Picking a path is already off-thread (see `spawn_pick`); the *read + parse*
// that follows is the part that actually stalls the UI for a big/slow file, so
// it runs on a second worker thread too. The worker produces a pure-data
// [`LoadPayload`] (no viewport / no `self`), the UI thread applies it. Progress
// is shared through a [`LoadProgress`] the worker writes and the status bar
// reads each frame.

/// Shared progress handle: written by the load worker, read by the UI each
/// frame. `total == 0` means the size is unknown (e.g. a topology whose
/// `#include`s span several files) so the UI shows an indeterminate (animated)
/// bar rather than a misleading fraction.
#[derive(Default)]
struct LoadProgress {
    done: AtomicU64,
    total: AtomicU64,
    stage: Mutex<String>,
    cancel: AtomicBool,
}

impl LoadProgress {
    fn set_stage(&self, s: impl Into<String>) {
        if let Ok(mut g) = self.stage.lock() {
            *g = s.into();
        }
    }

    fn stage_text(&self) -> String {
        self.stage.lock().map(|g| g.clone()).unwrap_or_default()
    }

    fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        Some((self.done.load(Ordering::Relaxed) as f32 / total as f32).clamp(0.0, 1.0))
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A `Read` wrapper that adds each chunk it reads to a [`LoadProgress`], giving
/// byte-level progress for free — parsers read through it unchanged. Returns an
/// error when cancellation is requested so the in-progress parse unwinds instead
/// of finishing work the user asked to abandon.
struct ProgressReader<R> {
    inner: R,
    progress: Arc<LoadProgress>,
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.progress.is_cancelled() {
            // Must NOT be `ErrorKind::Interrupted`: every std read helper the
            // parsers use (read_to_string/read_to_end/read_exact/read_until and
            // the `Lines` iterator) treats Interrupted as "retry" and loops back
            // to call `read` again. Since `cancel` stays set, that would spin the
            // worker forever instead of unwinding. Any other kind is propagated,
            // so the parse returns `Err` and spawn_load's `is_cancelled()` path
            // collapses it to the cancel sentinel.
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "load cancelled",
            ));
        }
        let n = self.inner.read(buf)?;
        self.progress.done.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// Pure parsed data produced by a load worker — carries no viewport types and no
/// `self`, so it is `Send` and can cross the thread boundary. The UI thread turns
/// it into molecule/viewport state in [`KuromameApp::apply_finished_load`].
enum LoadPayload {
    Pdb(PdbFile),
    Mol2(Mol2File),
    Gro(GroFile),
    Top(TopFile, Option<MartiniForceField>),
    TopGro {
        top: TopFile,
        martini: Option<MartiniForceField>,
        gro: GroFile,
    },
    Ndx(NdxFile),
    Xtc(XtcFile),
    Overlay(Vec<Vec3>), // surface dots
}

/// A finished background load handed back to the UI thread. `paths` echoes the
/// request so the applier can store the source paths; `result` is the parsed
/// payload or an error message (the sentinel `"__cancelled__"` when cancelled).
struct FinishedLoad {
    /// Which pick produced this load. The applier actually routes on the parsed
    /// [`LoadPayload`] variant (it already names the target), so `kind` is kept
    /// only as a self-describing echo of the request.
    #[allow(dead_code)]
    kind: PickKind,
    paths: Vec<PathBuf>,
    result: Result<LoadPayload, String>,
}

/// State for an in-flight background load: the channel the worker sends its
/// [`FinishedLoad`] on, and the shared progress the status bar polls.
struct PendingLoad {
    rx: Receiver<FinishedLoad>,
    progress: Arc<LoadProgress>,
}

/// Error sentinel a worker sends when the load was cancelled, so the applier can
/// stay silent (beyond a short status) instead of surfacing an I/O error.
const CANCELLED_SENTINEL: &str = "__cancelled__";

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

/// Show/colour state for one NDX group. Kept in a vec parallel to the loaded
/// [`NdxFile`]'s groups and rebuilt whenever an NDX file is imported, so index
/// `i` here always describes group `i` there.
#[derive(Clone)]
struct NdxGroupUi {
    /// Whether this group contributes atoms to the NDX highlight. Independent of
    /// the block's master `ndx_visible` toggle, which hides all groups at once.
    enabled: bool,
    color: [f32; 3],
}

/// Severity of one line in the command log, which only drives its colour.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One line of command output.
///
/// The status bar holds a single overwriting `String`, which cannot show a
/// parse error's caret line or the several resolution notes one expression can
/// produce — hence a real log alongside it.
pub struct LogEntry {
    pub level: LogLevel,
    pub text: String,
}

/// How many command-log lines and history entries are kept.
const LOG_CAPACITY: usize = 200;
const HISTORY_CAPACITY: usize = 200;

/// Sphere/cylinder mesh resolution forced for a full-size export.
///
/// The interactive view runs a LOD that trades detail for frame rate, which is
/// invisible at a few hundred pixels and unmistakably faceted at a couple of
/// thousand. Only the final render pays for this; the preview keeps whatever the
/// view is already on.
const EXPORT_MESH_RESOLUTION: usize = 32;

/// State behind the image-export dialog.
///
/// The dialog shows a small render of the current view, the user drags the
/// region they want on it, and the export re-renders just that region at full
/// size. Both renders go through
/// [`InteractiveMoleculeViewport::render_image`], which resizes the shared color
/// target and blocks on a GPU readback — so they are queued here and run at the
/// top of the next frame, before the viewport draws itself.
#[derive(Default)]
struct ExportUiState {
    open: bool,
    settings: ExportSettings,
    /// Cached preview, uploaded from the last preview render.
    preview: Option<egui::TextureHandle>,
    /// Viewport size the cached preview was rendered for; a window resize
    /// invalidates it.
    preview_view_size: (u32, u32),
    /// Set when the preview no longer reflects the settings.
    preview_dirty: bool,
    /// Drag origin in normalised view coordinates while a region is being drawn.
    drag_anchor: Option<[f32; 2]>,
    /// Destination picked by the Save button, consumed on the next frame.
    pending_save: Option<PathBuf>,
    /// Last failure, shown inside the dialog.
    error: Option<String>,
}

struct UiState {
    status_msg: String,
    show_edit_dialog: bool,
    new_res_name: String,
    hovered_atom_info: String,
    selector_input: String,
    /// Contents of the bottom command bar.
    command_input: String,
    /// Commands as entered, oldest first. Global, not per layer — the history is
    /// the user's, not the structure's.
    command_history: Vec<String>,
    /// Position while walking `command_history` with the arrow keys; `None` when
    /// the user is editing a fresh line.
    history_cursor: Option<usize>,
    command_log: Vec<LogEntry>,
    /// Set for one frame to pull keyboard focus into the command bar (Ctrl+P).
    focus_command_bar: bool,
    /// Whether the log area shows a few lines or a taller scroll.
    command_log_expanded: bool,
    ndx_groups: Vec<NdxGroupUi>,
    ndx_visible: bool,
    /// Alpha of the NDX highlight spheres in `0.0..=1.0`. Separate from the
    /// layer's structure opacity, so a faded structure can still carry a solid
    /// NDX colouring (and vice versa).
    ndx_opacity: f32,
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

/// The index mapping between the full molecule (`self.molecule`, original
/// indices) and the possibly-filtered molecule actually handed to the viewport
/// (view indices).
///
/// Which atoms are filtered out is decided by [`ComponentState`]; this type
/// only records the resulting renumbering.
///
/// The maps are empty when nothing is hidden, which means "identity" — the
/// viewport gets the full molecule and `to_view`/`to_orig` are no-ops. This
/// keeps the common (all-visible) case allocation-free and behaviour-identical
/// to before the feature existed.
#[derive(Default)]
struct VisibilityState {
    view_to_orig: Vec<usize>,
    orig_to_view: Vec<Option<u32>>,
}

impl VisibilityState {
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
            .filter_map(|&(a, b)| Some((self.to_view(a)?, self.to_view(b)?)))
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

/// Colours handed to NDX groups as a file is imported, so several groups drawn
/// at once stay tellable apart. Starts with the orange the NDX highlight has
/// always used, so a freshly imported file looks the way it always has. The user
/// can override any group's colour from the NDX GROUPS panel.
const NDX_GROUP_PALETTE: [[f32; 3]; 8] = [
    [1.00, 0.60, 0.00], // orange
    [0.30, 0.64, 1.00], // blue
    [0.45, 0.85, 0.45], // green
    [0.95, 0.35, 0.35], // red
    [0.80, 0.45, 0.95], // purple
    [0.30, 0.85, 0.85], // cyan
    [0.95, 0.55, 0.80], // pink
    [0.88, 0.70, 0.25], // amber
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
    /// This layer's COMPONENTS partition — its own splits, merges and show/hide
    /// choices, independent of every other layer's.
    components: ComponentState,
    visibility: VisibilityState,
    interaction_pairs: Vec<(usize, usize)>,
    surface_dots: Vec<Vec3>,
    surface_visible: bool,
    martini_ff: Option<MartiniForceField>,
    bead_types: Vec<String>,
    martini_visible: bool,
    // Per-structure UI state (mirrors the working copies in `UiState`).
    selector_input: String,
    ndx_groups: Vec<NdxGroupUi>,
    ndx_visible: bool,
    ndx_opacity: f32,
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
            components: ComponentState::default(),
            visibility: VisibilityState::default(),
            interaction_pairs: Vec::new(),
            surface_dots: Vec::new(),
            surface_visible: true,
            martini_ff: None,
            bead_types: Vec::new(),
            martini_visible: true,
            selector_input: String::new(),
            ndx_groups: Vec::new(),
            ndx_visible: true,
            ndx_opacity: 1.0,
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
    /// The active layer's COMPONENTS partition: which named group every atom
    /// belongs to and whether that group is drawn. Decides what
    /// [`Self::rebuild_viewport`] filters out; `visibility` then records the
    /// renumbering that filtering produced.
    components: ComponentState,
    visibility: VisibilityState,
    /// Image-export dialog state. Global, not per layer — it describes the
    /// camera framing, which the viewport owns, not the structure.
    export_ui: ExportUiState,
    /// Per-atom lookup tables for the command language, rebuilt lazily whenever
    /// the molecule itself changes (not on visibility-only rebuilds). Guarded by
    /// `atom_table_dirty`, mirroring the `bead_types_dirty` pattern.
    atom_table: Option<AtomTable>,
    atom_table_dirty: bool,
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
    /// Whether the XYZ orientation triad is drawn at the world origin. A global
    /// view preference (not per-layer); mirrored into the viewport's
    /// [`AxesState`] whenever it changes.
    axis_visible: bool,
    /// Edge lengths of the active layer's simulation box, in nm; `(0, 0, 0)`
    /// when there is none. Kept here because the axis triad is sized from it,
    /// and the two overlay states have to be written together.
    sim_cell: (f32, f32, f32),
    /// `false` once `bead_types` matches the current molecule/topology. Lets
    /// `recompute_bead_types` skip its O(atoms) rebuild (which re-expands the whole
    /// topology) on visibility-only viewport rebuilds — bead types depend on the
    /// molecule and topology, not on which residues are hidden. Set whenever atom
    /// identity or the topology changes.
    bead_types_dirty: bool,
    /// Receiver for an in-flight file dialog running on a background thread, so
    /// the UI keeps rendering while the native picker is open. `update`/`ui`
    /// polls this and dispatches the chosen path(s) to the matching loader.
    /// `None` when no dialog is open. At most one picker runs at a time.
    pending_pick: Option<Receiver<PickedFiles>>,
    /// In-flight background file *load* (read + parse), if any. Analogous to
    /// `pending_pick` but for the second async stage: once paths are known the
    /// read+parse runs on a worker so a big/slow file no longer freezes the UI.
    /// `update`/`ui` polls it and applies the finished payload on the UI thread.
    /// `None` when nothing is loading; at most one load runs at a time.
    pending_load: Option<PendingLoad>,
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
        viewport.add_additional_render_box(Box::new(AtomPairRender::new()));
        viewport.add_additional_render_box(Box::new(AtomGroupRender::new()));
        viewport.add_additional_render_box(Box::new(SimulationCellRender::new()));
        viewport.add_additional_render_box(Box::new(PointCloudRender::new()));
        viewport.add_additional_render_box(Box::new(SphereSetRender::new()));
        viewport.add_additional_render_box(Box::new(AxesRender::new()));
        // Show the XYZ orientation triad by default so the coordinate frame is
        // always visible; the user can hide it from the view options.
        viewport.set_state_by_type(AxesState {
            visible: true,
            length: None,
        });

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
                command_input: String::new(),
                command_history: Vec::new(),
                history_cursor: None,
                command_log: Vec::new(),
                focus_command_bar: false,
                command_log_expanded: false,
                ndx_groups: Vec::new(),
                ndx_visible: true,
                ndx_opacity: 1.0,
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
            components: ComponentState::default(),
            visibility: VisibilityState::default(),
            export_ui: ExportUiState::default(),
            atom_table: None,
            atom_table_dirty: true,
            interaction_pairs: Vec::new(),
            surface_dots: Vec::new(),
            surface_visible: true,
            overlay_surfaces: Vec::new(),
            active_overlay: 0,
            layers: vec![Layer::new("Layer 1".to_string(), LAYER_PALETTE[0])],
            active_layer: 0,
            martini_ff: None,
            bead_types: Vec::new(),
            bead_types_dirty: true,
            martini_visible: true,
            axis_visible: true,
            sim_cell: (0.0, 0.0, 0.0),
            pending_pick: None,
            pending_load: None,
        }
    }

    /// Run a native file dialog on a background thread and remember its receiver
    /// so the UI stays responsive while the picker is open. The `dialog` closure
    /// runs the (blocking) [`FileDialog`] and returns the chosen paths (empty on
    /// cancel); [`Self::poll_pending_pick`] dispatches the result to the loader
    /// named by `kind`.
    fn spawn_pick<F>(&mut self, kind: PickKind, dialog: F)
    where
        F: FnOnce() -> Vec<PathBuf> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let paths = dialog();
            // Ignore the send error: it only means the app dropped the receiver
            // (e.g. a newer dialog replaced this one) and no longer wants it.
            let _ = tx.send(PickedFiles { kind, paths });
        });
        self.pending_pick = Some(rx);
    }

    /// Poll the in-flight file picker, if any, and dispatch a finished result to
    /// the matching loader. Non-blocking; call once per frame.
    fn poll_pending_pick(&mut self) {
        let Some(rx) = self.pending_pick.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(picked) => {
                self.pending_pick = None;
                self.dispatch_pick(picked);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.pending_pick = None,
        }
    }

    /// Route a picked path (or TOP/GRO pair) to a background *load* worker for its
    /// kind. The read+parse — the part that stalls the UI on a big file — runs off
    /// thread; the parsed payload is applied by [`Self::apply_finished_load`] once
    /// [`Self::poll_pending_load`] receives it.
    fn dispatch_pick(&mut self, picked: PickedFiles) {
        let PickedFiles { kind, paths } = picked;
        if let PickKind::TopGroPair = kind {
            // Paths arrive as `[top, gro]`; keep that order so the worker pairs
            // them correctly.
            if paths.len() == 2 {
                self.spawn_load(kind, paths);
            } else {
                self.set_status("TOP/GRO pair selection cancelled");
            }
            return;
        }
        if paths.is_empty() {
            return; // dialog cancelled
        }
        self.spawn_load(kind, paths);
    }

    /// Spawn a worker that reads+parses `paths` for `kind` off the UI thread and
    /// sends back a [`FinishedLoad`]. Only one load runs at a time; a new one
    /// replaces any in-flight `pending_load` (the old worker's send then simply
    /// fails, exactly like the picker).
    fn spawn_load(&mut self, kind: PickKind, paths: Vec<PathBuf>) {
        let progress = Arc::new(LoadProgress::default());
        progress.set_stage("Loading");
        let (tx, rx) = mpsc::channel();
        let worker_progress = Arc::clone(&progress);
        std::thread::spawn(move || {
            let result = KuromameApp::run_parse(kind, &paths, &worker_progress);
            // A cancel that lands mid-parse surfaces as an I/O error; collapse
            // any outcome under a set cancel flag to the sentinel so the applier
            // stays quiet instead of reporting "load cancelled" as a read failure.
            let result = if worker_progress.is_cancelled() {
                Err(CANCELLED_SENTINEL.to_string())
            } else {
                result
            };
            // Ignore the send error: it only means a newer load replaced this one.
            let _ = tx.send(FinishedLoad { kind, paths, result });
        });
        self.pending_load = Some(PendingLoad { rx, progress });
        self.set_status("Loading …");
    }

    /// Poll the in-flight load, if any, and apply a finished result. Non-blocking;
    /// call once per frame (mirrors [`Self::poll_pending_pick`]).
    fn poll_pending_load(&mut self) {
        let Some(pending) = self.pending_load.as_ref() else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(finished) => {
                self.pending_load = None;
                self.apply_finished_load(finished);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.pending_load = None,
        }
    }

    /// Whether a background file load is currently in flight.
    pub fn load_in_progress(&self) -> bool {
        self.pending_load.is_some()
    }

    /// Progress fraction (0..1) of the in-flight load, or `None` when nothing is
    /// loading or the total size is unknown (indeterminate bar).
    pub fn load_fraction(&self) -> Option<f32> {
        self.pending_load.as_ref().and_then(|p| p.progress.fraction())
    }

    /// Short stage label for the in-flight load ("Loading", "Parsing topology",
    /// …), empty when nothing is loading.
    pub fn load_stage(&self) -> String {
        self.pending_load
            .as_ref()
            .map(|p| p.progress.stage_text())
            .unwrap_or_default()
    }

    /// Ask the in-flight load worker to stop. The worker checks this flag (and its
    /// `ProgressReader` unwinds on the next read), then sends the cancel sentinel
    /// that [`Self::apply_finished_load`] turns into a "Load cancelled" status.
    pub fn request_load_cancel(&mut self) {
        if let Some(pending) = self.pending_load.as_ref() {
            pending.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Worker entry point: dispatch to the right pure parse helper for `kind`.
    /// Runs on the load thread — takes no `self` and touches no viewport state.
    fn run_parse(
        kind: PickKind,
        paths: &[PathBuf],
        progress: &Arc<LoadProgress>,
    ) -> Result<LoadPayload, String> {
        let first = || {
            paths
                .first()
                .map(PathBuf::as_path)
                .ok_or_else(|| "No file to load".to_string())
        };
        match kind {
            PickKind::Structure => Self::parse_structure(first()?, progress),
            PickKind::StructureLayer => Self::parse_structure_layer(first()?, progress),
            PickKind::Top => Self::parse_top(first()?, progress),
            PickKind::Gro => Self::parse_gro(first()?, progress),
            PickKind::Ndx => Self::parse_ndx(first()?, progress),
            PickKind::Xtc => Self::parse_xtc(first()?, progress),
            PickKind::OverlaySurface => Self::parse_overlay(first()?, progress),
            PickKind::TopGroPair => {
                let top = first()?;
                let gro = paths
                    .get(1)
                    .map(PathBuf::as_path)
                    .ok_or_else(|| "No file to load".to_string())?;
                Self::parse_topgro(top, gro, progress)
            }
        }
    }

    // --- Pure parse helpers (run on the load worker) -----------------------
    //
    // Each opens the file(s), reads through a `ProgressReader` for byte-level
    // progress + cancellation, and returns pure data. They mirror the read+parse
    // step of the old synchronous loaders exactly (same error messages), so the
    // UI-thread apply step below is the only place that mutates `self`.

    /// Open `path` for reading, record its byte length as the progress total, and
    /// wrap it so reads report progress and honour cancellation.
    fn open_progress_reader(
        path: &Path,
        progress: &Arc<LoadProgress>,
    ) -> std::io::Result<ProgressReader<File>> {
        let file = File::open(path)?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        progress.total.store(len, Ordering::Relaxed);
        progress.done.store(0, Ordering::Relaxed);
        Ok(ProgressReader {
            inner: file,
            progress: Arc::clone(progress),
        })
    }

    /// Read the whole file into a `String` through the progress reader (for the
    /// parsers that consume `&str`).
    fn read_file_to_string(path: &Path, progress: &Arc<LoadProgress>) -> std::io::Result<String> {
        let mut reader = Self::open_progress_reader(path, progress)?;
        let mut s = String::new();
        reader.read_to_string(&mut s)?;
        Ok(s)
    }

    /// Structure into the active layer (PDB/MOL2 only), mirroring `load_file`'s
    /// extension routing.
    fn parse_structure(path: &Path, progress: &Arc<LoadProgress>) -> Result<LoadPayload, String> {
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            return Err("Unsupported file type".to_string());
        };
        match ext.to_lowercase().as_str() {
            "pdb" | "ent" => Self::parse_pdb(path, progress),
            "mol2" => Self::parse_mol2(path, progress),
            _ => Err("Unsupported file type".to_string()),
        }
    }

    /// Structure into a new layer, dispatched by extension, mirroring
    /// `load_structure_path` (GRO / TOP-ITP / else PDB-MOL2).
    fn parse_structure_layer(
        path: &Path,
        progress: &Arc<LoadProgress>,
    ) -> Result<LoadPayload, String> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "gro" => Self::parse_gro(path, progress),
            "top" | "itp" => Self::parse_top(path, progress),
            _ => Self::parse_structure(path, progress),
        }
    }

    fn parse_pdb(path: &Path, progress: &Arc<LoadProgress>) -> Result<LoadPayload, String> {
        let content = Self::read_file_to_string(path, progress)
            .map_err(|_| "Failed to load PDB file".to_string())?;
        Ok(LoadPayload::Pdb(PdbFile::load(&content)))
    }

    fn parse_mol2(path: &Path, progress: &Arc<LoadProgress>) -> Result<LoadPayload, String> {
        let content = Self::read_file_to_string(path, progress)
            .map_err(|_| "Failed to load MOL2 file".to_string())?;
        Ok(LoadPayload::Mol2(Mol2File::load(&content)))
    }

    fn parse_gro(path: &Path, progress: &Arc<LoadProgress>) -> Result<LoadPayload, String> {
        let gro = Self::read_gro(path, progress)?;
        Ok(LoadPayload::Gro(gro))
    }

    /// Read a GRO through the progress reader. Shared by `parse_gro` and the
    /// TOP+GRO pair. Any I/O/parse failure maps to the message the old loader
    /// used so the status text is unchanged.
    fn read_gro(path: &Path, progress: &Arc<LoadProgress>) -> Result<GroFile, String> {
        let reader = Self::open_progress_reader(path, progress)
            .map_err(|_| "Failed to read GRO file".to_string())?;
        // GroFile::load_from_reader needs `BufRead`; the ProgressReader is only
        // `Read`, so buffer it. Progress then advances in buffer-sized steps,
        // which is fine for a bar.
        GroFile::load_from_reader(BufReader::new(reader))
            .map_err(|_| "Failed to read GRO file".to_string())
    }

    fn parse_ndx(path: &Path, progress: &Arc<LoadProgress>) -> Result<LoadPayload, String> {
        let content = Self::read_file_to_string(path, progress)
            .map_err(|_| "Failed to read NDX file".to_string())?;
        let ndx = NdxFile::parse(&content).map_err(|err| format!("NDX parse failed: {err}"))?;
        Ok(LoadPayload::Ndx(ndx))
    }

    fn parse_overlay(path: &Path, progress: &Arc<LoadProgress>) -> Result<LoadPayload, String> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let content = Self::read_file_to_string(path, progress)
            .map_err(|_| format!("Failed to read {file_name}"))?;
        let dots = PdbFile::load(&content).surface_dots();
        if dots.is_empty() {
            return Err(format!("{file_name} contains no DOT surface"));
        }
        Ok(LoadPayload::Overlay(dots))
    }

    fn parse_xtc(path: &Path, progress: &Arc<LoadProgress>) -> Result<LoadPayload, String> {
        let reader = Self::open_progress_reader(path, progress)
            .map_err(|e| format!("Failed to load XTC: {e}"))?;
        // Buffer the (unbuffered) file+progress reader: the XTC decoder issues
        // many small reads. Cancellation still works — the ProgressReader under
        // the buffer returns a (non-retriable) I/O error, which unwinds the parse.
        let xtc = XtcFile::load_from_reader(BufReader::new(reader))
            .map_err(|e| format!("Failed to load XTC: {e}"))?;
        Ok(LoadPayload::Xtc(xtc))
    }

    fn parse_top(path: &Path, progress: &Arc<LoadProgress>) -> Result<LoadPayload, String> {
        // `#include` expansion spans several files, so there is no single byte
        // total to track — leave `total == 0` for an indeterminate bar.
        progress.total.store(0, Ordering::Relaxed);
        progress.set_stage("Parsing topology");
        let top = TopFile::load_from_path(path)?;
        let martini = Self::parse_martini_ff(path);
        Ok(LoadPayload::Top(top, martini))
    }

    fn parse_topgro(
        top_path: &Path,
        gro_path: &Path,
        progress: &Arc<LoadProgress>,
    ) -> Result<LoadPayload, String> {
        progress.total.store(0, Ordering::Relaxed);
        progress.set_stage("Parsing topology");
        let top = TopFile::load_from_path(top_path)?;
        let martini = Self::parse_martini_ff(top_path);
        progress.set_stage("Reading coordinates");
        let gro = Self::read_gro(gro_path, progress)?;
        Ok(LoadPayload::TopGro { top, martini, gro })
    }

    /// Parse the Martini force field a topology pulls in via `#include`, if the
    /// include-expanded content really is one. The pure counterpart of the parse
    /// step in [`Self::apply_martini_ff`] — mirrors the old `try_load_martini_ff`
    /// without touching `self` so it can run on the worker.
    fn parse_martini_ff(path: &Path) -> Option<MartiniForceField> {
        TopFile::expand_includes(path)
            .ok()
            .map(|expanded| MartiniForceField::parse(&expanded))
            .filter(|ff| ff.is_forcefield())
    }

    // --- Apply a finished load (UI thread) ---------------------------------

    /// Apply a worker's [`FinishedLoad`] on the UI thread: route the parsed
    /// payload into `self`/viewport with the exact same side effects and status
    /// messages the old synchronous loaders produced after their parse step.
    fn apply_finished_load(&mut self, finished: FinishedLoad) {
        let FinishedLoad { paths, result, .. } = finished;
        let payload = match result {
            Ok(payload) => payload,
            Err(msg) if msg == CANCELLED_SENTINEL => {
                self.set_status("Load cancelled");
                return;
            }
            Err(msg) => {
                self.set_status(msg);
                return;
            }
        };

        // `paths` echoes the original pick: one path for every kind except the
        // TOP/GRO pair, which carries `[top, gro]`.
        let mut paths = paths.into_iter();
        match payload {
            LoadPayload::Pdb(pdb) => {
                if let Some(path) = paths.next() {
                    self.apply_pdb(pdb, path);
                    self.post_load_cleanup();
                }
            }
            LoadPayload::Mol2(mol2) => {
                if let Some(path) = paths.next() {
                    self.apply_mol2(mol2, path);
                    self.post_load_cleanup();
                }
            }
            LoadPayload::Gro(gro) => {
                if let Some(path) = paths.next() {
                    self.apply_gro(gro, path);
                }
            }
            LoadPayload::Top(top, martini) => {
                if let Some(path) = paths.next() {
                    self.apply_top(top, martini, path);
                }
            }
            LoadPayload::TopGro { top, martini, gro } => {
                if let (Some(top_path), Some(gro_path)) = (paths.next(), paths.next()) {
                    self.apply_topgro(top, martini, gro, top_path, gro_path);
                }
            }
            LoadPayload::Ndx(ndx) => {
                if let Some(path) = paths.next() {
                    self.apply_ndx(ndx, path);
                }
            }
            LoadPayload::Xtc(xtc) => {
                if let Some(path) = paths.next() {
                    self.apply_xtc(xtc, path);
                }
            }
            LoadPayload::Overlay(dots) => {
                if let Some(path) = paths.next() {
                    self.apply_overlay(dots, path);
                }
            }
        }
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.ui.status_msg = msg.into();
    }

    /// Append to the command log, oldest lines falling off the front.
    fn log(&mut self, level: LogLevel, text: impl Into<String>) {
        self.ui.command_log.push(LogEntry {
            level,
            text: text.into(),
        });
        let overflow = self.ui.command_log.len().saturating_sub(LOG_CAPACITY);
        if overflow > 0 {
            self.ui.command_log.drain(..overflow);
        }
    }

    fn log_info(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Info, text);
    }

    fn log_error(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Error, text);
    }

    pub fn command_log(&self) -> &[LogEntry] {
        &self.ui.command_log
    }

    pub fn clear_command_log(&mut self) {
        self.ui.command_log.clear();
    }

    pub fn command_input(&self) -> &str {
        &self.ui.command_input
    }

    pub fn command_input_mut(&mut self) -> &mut String {
        &mut self.ui.command_input
    }

    pub fn command_log_expanded(&self) -> bool {
        self.ui.command_log_expanded
    }

    pub fn toggle_command_log_expanded(&mut self) {
        self.ui.command_log_expanded = !self.ui.command_log_expanded;
    }

    // ------------------------------------------------------------ image export

    pub fn export_dialog_open(&self) -> bool {
        self.export_ui.open
    }

    /// Open the image-export dialog, queueing a first preview render.
    pub fn open_export_image_dialog(&mut self) {
        if self.molecule.is_none() {
            self.set_status("Load a structure before exporting an image");
            return;
        }
        self.export_ui.open = true;
        self.export_ui.error = None;
        self.export_ui.preview_dirty = true;
    }

    pub fn close_export_image_dialog(&mut self) {
        self.export_ui.open = false;
        self.export_ui.drag_anchor = None;
        // Drop the cached preview so reopening cannot show a stale camera.
        self.export_ui.preview = None;
    }

    pub fn export_settings(&self) -> &ExportSettings {
        &self.export_ui.settings
    }

    /// Mutable settings plus the dirty flag, so the dialog can edit them without
    /// having to remember to invalidate the preview.
    pub fn edit_export_settings(&mut self) -> &mut ExportSettings {
        self.export_ui.preview_dirty = true;
        &mut self.export_ui.settings
    }

    pub fn export_preview(&self) -> Option<&egui::TextureHandle> {
        self.export_ui.preview.as_ref()
    }

    pub fn export_error(&self) -> Option<&str> {
        self.export_ui.error.as_deref()
    }

    pub fn export_drag_anchor(&self) -> Option<[f32; 2]> {
        self.export_ui.drag_anchor
    }

    pub fn set_export_drag_anchor(&mut self, anchor: Option<[f32; 2]>) {
        self.export_ui.drag_anchor = anchor;
    }

    /// On-screen viewport size in pixels, which every region calculation is
    /// measured against.
    pub fn viewport_pixel_size(&self) -> (u32, u32) {
        self.viewport.viewport_size()
    }

    /// Size the export would come out at with the current settings.
    pub fn export_output_size(&self) -> (u32, u32) {
        self.export_ui
            .settings
            .output_size(self.viewport_pixel_size())
    }

    pub fn set_export_region(&mut self, region: Option<[f32; 4]>) {
        self.export_ui.settings.region = region;
        self.export_ui.preview_dirty = true;
    }

    /// Re-derive the region from the chosen aspect preset: a locked preset gets
    /// the largest centred rectangle that fits, `Screen`/`Free` get the whole view.
    pub fn reset_export_region_for_aspect(&mut self) {
        let view = self.viewport_pixel_size();
        let view_aspect = image_export::region_pixel_aspect([0.0, 0.0, 1.0, 1.0], view);
        let ratio = self.export_ui.settings.aspect.ratio(view_aspect);
        let region = image_export::centred_region(ratio, view);
        self.set_export_region(region);
    }

    /// Queue a save to `path`; the render happens at the top of the next frame.
    pub fn request_export_image(&mut self, path: PathBuf) {
        self.export_ui.pending_save = Some(path);
    }

    /// Ask for a destination and queue the export. Blocks on the native dialog,
    /// the same way [`Self::export_structure`] does.
    pub fn pick_export_image_path(&mut self) {
        let stem = self
            .data
            .structure_file_path
            .as_ref()
            .or(self.data.top_file_path.as_ref())
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "molecule".to_string());

        if let Some(path) = FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name(format!("{stem}.png"))
            .set_title("Export image")
            .save_file()
        {
            // `image::save` picks its encoder from the extension, so make sure
            // there is one rather than failing after the render.
            let path = if path.extension().is_none() {
                path.with_extension("png")
            } else {
                path
            };
            self.request_export_image(path);
        }
    }

    /// Run any queued preview/export render.
    ///
    /// Must be called *before* the viewport draws itself: `render_image` resizes
    /// the shared color target, and `show` is what puts it back.
    fn process_pending_image_export(&mut self, ctx: &egui::Context) {
        if !self.export_ui.open && self.export_ui.pending_save.is_none() {
            return;
        }
        // Cloning is cheap — RenderState is a bundle of Arcs — and it keeps the
        // viewport borrow below independent of `self.render_state`.
        let Some(render_state) = self.render_state.clone() else {
            return;
        };
        let view = self.viewport.viewport_size();
        if view.0 == 0 || view.1 == 0 {
            return; // the viewport has not been laid out yet
        }

        // A window resize changes the view aspect, so both the preview and any
        // centred region have to be recomputed.
        if self.export_ui.preview_view_size != view {
            self.export_ui.preview_view_size = view;
            self.export_ui.preview_dirty = true;
        }

        if self.export_ui.open && self.export_ui.preview_dirty {
            let (pw, ph) = self.export_ui.settings.preview_size(view);
            let request = ImageExportRequest {
                width: pw,
                height: ph,
                // The preview always shows the whole view — the region is chosen
                // on top of it, so cropping it would be circular.
                region: None,
                clear_color: self.export_ui.settings.clear_color(),
                supersample: 1,
                // The preview is only a framing aid, and it re-renders on every
                // settings change — leave the detail alone and keep it cheap.
                mesh_resolution: None,
            };
            match self.viewport.render_image(&render_state, &request) {
                Ok(image) => {
                    let color = egui::ColorImage::from_rgba_unmultiplied(
                        [image.width as usize, image.height as usize],
                        &image.rgba,
                    );
                    self.export_ui.preview = Some(ctx.load_texture(
                        "export_preview",
                        color,
                        egui::TextureOptions::LINEAR,
                    ));
                    self.export_ui.error = None;
                }
                Err(err) => self.export_ui.error = Some(err),
            }
            self.export_ui.preview_dirty = false;
        }

        if let Some(path) = self.export_ui.pending_save.take() {
            let (width, height) = self.export_ui.settings.output_size(view);
            let request = ImageExportRequest {
                width,
                height,
                region: self.export_ui.settings.region,
                clear_color: self.export_ui.settings.clear_color(),
                supersample: self.export_ui.settings.supersample,
                // A figure gets blown up well past the on-screen size, where the
                // interactive LOD's mesh reads as visibly faceted spheres.
                mesh_resolution: Some(EXPORT_MESH_RESOLUTION),
            };
            let result = self
                .viewport
                .render_image(&render_state, &request)
                .and_then(|image| {
                    image_export::write_png(&path, image.width, image.height, image.rgba)
                        .map(|()| (image.width, image.height))
                });
            match result {
                Ok((w, h)) => {
                    self.export_ui.error = None;
                    self.export_ui.open = false;
                    self.set_status(format!(
                        "Saved {w}x{h} image to {}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    ));
                }
                Err(err) => {
                    self.set_status("Image export failed");
                    self.export_ui.error = Some(err);
                }
            }
        }
    }

    /// Ask the UI to put keyboard focus in the command bar next frame.
    pub fn request_command_focus(&mut self) {
        self.ui.focus_command_bar = true;
    }

    /// Consume the pending focus request, if any.
    pub fn take_command_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.ui.focus_command_bar)
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

        if !self.components.any_hidden() {
            // Identity: hand over the full molecule, no remapping needed.
            self.visibility.view_to_orig.clear();
            self.visibility.orig_to_view.clear();
            self.viewport.set_molecule(full.clone());
        } else {
            let mut view_to_orig: Vec<usize> = Vec::with_capacity(full.atoms.len());
            let mut orig_to_view: Vec<Option<u32>> = vec![None; full.atoms.len()];
            let mut atoms: Vec<Atom> = Vec::new();
            for (orig, atom) in full.atoms.iter().enumerate() {
                if self.components.is_atom_visible(orig) {
                    orig_to_view[orig] = Some(view_to_orig.len() as u32);
                    view_to_orig.push(orig);
                    atoms.push(atom.clone());
                }
            }
            let mut bonds: Vec<Bond> = Vec::new();
            for bond in &full.bonds {
                // Index with `get`: a bond endpoint can point past the atom list
                // when the topology and the coordinate file disagree on the atom
                // count, and a bad bond must degrade to a missing stick, not a
                // panic in the middle of a repaint.
                let (va, vb) = (
                    orig_to_view.get(bond.atom_a).copied().flatten(),
                    orig_to_view.get(bond.atom_b).copied().flatten(),
                );
                if let (Some(a), Some(b)) = (va, vb) {
                    bonds.push(Bond {
                        atom_a: a as usize,
                        atom_b: b as usize,
                        order: bond.order,
                    });
                }
            }
            self.visibility.view_to_orig = view_to_orig;
            self.visibility.orig_to_view = orig_to_view;
            self.viewport.set_molecule(molecule_from_parts(atoms, bonds));
        }

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
            .set_state_by_type(AtomPairState { pairs });
    }

    /// Re-derive the bead type of every atom in `self.molecule` (original-index
    /// order). Prefers the topology's per-atom `atom_type`; falls back to the
    /// atom name when there is no matching topology (e.g. a bare CG `.gro`).
    fn recompute_bead_types(&mut self) {
        // Bead types depend only on the molecule and topology, not on the residue
        // visibility filter, so a visibility-only rebuild leaves them unchanged.
        // Skip the O(atoms) re-expansion of the whole topology in that common case.
        if !self.bead_types_dirty {
            return;
        }
        self.bead_types_dirty = false;
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

    /// Register a Martini force field parsed off-thread (see
    /// [`Self::parse_martini_ff`]) and refresh bead styling for any loaded
    /// structure. Returns the bead-type count when a force field was found.
    ///
    /// The force field belongs to the topology, so a topology that is *not*
    /// Martini (`ff == None`) drops any previously registered one rather than
    /// leaving the old beads applied to the new structure.
    fn apply_martini_ff(&mut self, ff: Option<MartiniForceField>) -> Option<usize> {
        let count = ff.as_ref().map(|ff| ff.bead_type_count());
        self.martini_ff = ff;
        // The topology this force field came with was just (re)loaded, so the
        // per-atom bead types need re-deriving.
        self.bead_types_dirty = true;
        self.recompute_bead_types();
        self.refresh_martini_bead_state();
        count
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

    /// Whether the XYZ orientation triad is drawn.
    pub fn axis_visible(&self) -> bool {
        self.axis_visible
    }

    /// Show or hide the XYZ orientation triad. Pushes the new visibility to the
    /// viewport's [`AxesState`]; the axis render reads it on the next frame.
    pub fn set_axis_visible(&mut self, visible: bool) {
        if self.axis_visible != visible {
            self.axis_visible = visible;
            self.refresh_axes_state();
        }
    }

    /// Bring the COMPONENTS partition in line with the current molecule.
    ///
    /// A partition whose atom count still matches is *kept* — that is what lets
    /// a user's splits and merges survive a trajectory frame or a reload of the
    /// same system. Anything else is rebuilt into the default
    /// one-component-per-residue-name layout, which is exactly what the old
    /// `refresh_res_names` produced, so a fresh load looks unchanged.
    fn refresh_components(&mut self) {
        let Some(mol) = self.molecule.as_ref() else {
            return;
        };
        if self.components.matches_atom_count(mol.atoms.len()) && !self.components.is_empty() {
            return;
        }
        let had_components = !self.components.is_empty();
        let mol = mol.clone();
        let previous_atoms = self.components.atom_count();
        self.components.rebuild_from_molecule(&mol);
        self.atom_table_dirty = true;
        if had_components {
            self.log_info(format!(
                "components reset (atom count changed {} -> {})",
                previous_atoms,
                mol.atoms.len()
            ));
        }
    }

    /// Components with their visibility and size, for the UI panel.
    pub fn component_list(&self) -> Vec<(String, bool, usize)> {
        self.components
            .components()
            .iter()
            .map(|c| (c.name.clone(), c.visible, c.atoms.len()))
            .collect()
    }

    pub fn has_components(&self) -> bool {
        !self.components.is_empty()
    }

    pub fn set_component_visible(&mut self, name: &str, visible: bool) {
        if matches!(self.components.set_visible(name, visible), Ok(true)) {
            self.rebuild_viewport(false);
        }
    }

    pub fn set_all_components_visible(&mut self, visible: bool) {
        if self.components.set_all_visible(visible) {
            self.rebuild_viewport(false);
        }
    }

    /// (Re)build the per-atom lookup tables the command language evaluates
    /// against. Cheap to call; does nothing unless the molecule changed.
    fn ensure_atom_table(&mut self) {
        if !self.atom_table_dirty && self.atom_table.is_some() {
            return;
        }
        if let Some(mol) = self.molecule.as_ref() {
            self.atom_table = Some(AtomTable::from_molecule(mol));
            self.atom_table_dirty = false;
        }
    }

    /// Evaluate an expression against the current molecule, logging how each
    /// bare word resolved. Returns the matched atoms as original indices.
    fn eval_expr(&mut self, expr: &crate::selection::Expr) -> Option<Vec<u32>> {
        self.ensure_atom_table();
        // Move the table out so `&mut self` stays available for logging; it is
        // put straight back, and nothing in between can observe the gap.
        let Some(table) = self.atom_table.take() else {
            self.log_error("no molecule loaded");
            return None;
        };
        let pairs = self.components.name_atom_pairs();
        let selected = self.selection.selected_atom_indices.clone();
        let mut notes = Vec::new();
        let result = {
            let ctx = EvalCtx {
                table: &table,
                components: &pairs,
                selected: &selected,
            };
            evaluate(expr, &ctx, &mut notes)
        };
        self.atom_table = Some(table);

        for note in notes {
            self.log_info(format!("  {note}"));
        }
        match result {
            Ok(mask) => Some(to_indices(&mask)),
            Err(err) => {
                self.log_error(err.to_string());
                self.set_status("Selection failed");
                None
            }
        }
    }

    /// Run one line from the command bar.
    ///
    /// Everything here is display-only: components are regrouped, but no PDB /
    /// GRO / TOP record is touched and `mark_modified` is deliberately not
    /// called, so splitting and merging never dirties the user's files.
    pub fn run_command(&mut self, input: &str) {
        let line = input.trim().to_string();
        if line.is_empty() {
            return;
        }
        self.push_history(&line);
        self.log_info(format!("> {line}"));

        let stmt = match parse_statement(&line) {
            Ok(stmt) => stmt,
            Err(err) => {
                // The `> {line}` echo above already shows the input, so log only
                // the caret and hint.
                self.log_error(err.caret(&line));
                self.set_status(err.to_string());
                return;
            }
        };

        // Everything except `list`/`help` needs a structure to talk about.
        let needs_molecule = !matches!(stmt, Statement::List | Statement::Help);
        if needs_molecule && self.molecule.is_none() {
            self.log_error("no molecule loaded");
            self.set_status("No molecule loaded");
            return;
        }

        match stmt {
            Statement::Count(expr) => {
                if let Some(atoms) = self.eval_expr(&expr) {
                    let msg = format!("matched {} atoms", atoms.len());
                    self.log_info(format!("  {msg}"));
                    self.set_status(msg);
                }
            }

            Statement::Assign { name, expr } => {
                let Some(atoms) = self.eval_expr(&expr) else {
                    return;
                };
                if atoms.is_empty() {
                    self.log_error(format!("'{name}' not created: the selection matched 0 atoms"));
                    self.set_status("Selection matched 0 atoms");
                    return;
                }
                let outcome = match self.components.assign(
                    &name,
                    &atoms,
                    Some(line.clone()),
                    self.molecule.as_ref().expect("checked above"),
                ) {
                    Ok(outcome) => outcome,
                    Err(err) => {
                        self.fail(err.to_string());
                        return;
                    }
                };
                let verb = if outcome.created { "created" } else { "updated" };
                let mut msg = format!("{name}: {verb}, {} atoms", outcome.claimed);
                if !outcome.absorbed.is_empty() {
                    msg.push_str(&format!(" (merged in {})", outcome.absorbed.join(", ")));
                }
                self.log_info(format!("  {msg}"));
                self.set_status(msg);
                self.rebuild_viewport(false);
            }

            Statement::Show(targets) => self.apply_visibility(targets, true),
            Statement::Hide(targets) => self.apply_visibility(targets, false),

            Statement::Only(name) => match self.components.only(&name) {
                Ok(()) => {
                    self.set_status(format!("Showing only {name}"));
                    self.rebuild_viewport(false);
                }
                Err(err) => self.fail(err.to_string()),
            },

            Statement::Del(names) => {
                let mut moved = 0usize;
                for name in &names {
                    match self
                        .components
                        .dissolve(name, self.molecule.as_ref().expect("checked above"))
                    {
                        Ok(count) => moved += count,
                        Err(err) => {
                            self.fail(err.to_string());
                            return;
                        }
                    }
                }
                let msg = format!("dissolved {} ({moved} atoms returned)", names.join(", "));
                self.log_info(format!("  {msg}"));
                self.set_status(msg);
                self.rebuild_viewport(false);
            }

            Statement::Rename { from, to } => match self.components.rename(&from, &to) {
                Ok(()) => self.set_status(format!("Renamed {from} to {to}")),
                Err(err) => self.fail(err.to_string()),
            },

            Statement::List => {
                if self.components.is_empty() {
                    self.log_info("  (no components — load a structure)");
                    return;
                }
                for (name, visible, count) in self.component_list() {
                    let label = if name.is_empty() { "(no residue)" } else { &name };
                    let source = self
                        .components
                        .find(&name)
                        .and_then(|i| self.components.components()[i].source.clone())
                        .map(|s| format!("   {s}"))
                        .unwrap_or_default();
                    self.log_info(format!(
                        "  {label:<14} {count:>8} atoms  {}{source}",
                        if visible { "shown " } else { "hidden" }
                    ));
                }
                self.set_status(format!("{} components", self.components.len()));
            }

            Statement::Reset => {
                let mol = self.molecule.clone().expect("checked above");
                self.components.rebuild_from_molecule(&mol);
                let msg = format!("reset to {} residue components", self.components.len());
                self.log_info(format!("  {msg}"));
                self.set_status(msg);
                self.rebuild_viewport(false);
            }

            Statement::Help => {
                for line in HELP_TEXT.lines() {
                    self.log_info(line);
                }
            }
        }
    }

    /// Shared tail of `show` / `hide`.
    fn apply_visibility(&mut self, targets: Targets, visible: bool) {
        let word = if visible { "Showing" } else { "Hiding" };
        let changed = match targets {
            Targets::All => {
                let changed = self.components.set_all_visible(visible);
                self.set_status(format!("{word} all components"));
                changed
            }
            Targets::Named(names) => {
                let mut changed = false;
                for name in &names {
                    match self.components.set_visible(name, visible) {
                        Ok(did) => changed |= did,
                        Err(err) => {
                            self.fail(err.to_string());
                            return;
                        }
                    }
                }
                self.set_status(format!("{word} {}", names.join(", ")));
                changed
            }
        };
        // A rebuild re-uploads the whole molecule; skip it when the command was
        // a no-op (hiding what is already hidden).
        if changed {
            self.rebuild_viewport(false);
        }
    }

    /// Report a command failure to both the log and the status bar.
    fn fail(&mut self, msg: String) {
        self.log_error(format!("  {msg}"));
        self.set_status(msg);
    }

    /// Record a command, skipping an immediate repeat so holding Enter does not
    /// fill the history with one line.
    fn push_history(&mut self, line: &str) {
        self.ui.history_cursor = None;
        if self.ui.command_history.last().map(String::as_str) == Some(line) {
            return;
        }
        self.ui.command_history.push(line.to_string());
        let overflow = self
            .ui
            .command_history
            .len()
            .saturating_sub(HISTORY_CAPACITY);
        if overflow > 0 {
            self.ui.command_history.drain(..overflow);
        }
    }

    /// Step through the command history. `delta` is -1 for older, +1 for newer.
    /// Walking past the newest entry restores an empty line.
    pub fn recall_history(&mut self, delta: i32) {
        if self.ui.command_history.is_empty() {
            return;
        }
        let last = self.ui.command_history.len() - 1;
        self.ui.history_cursor = match (self.ui.history_cursor, delta) {
            (None, d) if d < 0 => Some(last),
            (None, _) => None,
            (Some(i), d) if d < 0 => Some(i.saturating_sub(1)),
            (Some(i), _) if i >= last => None,
            (Some(i), _) => Some(i + 1),
        };
        self.ui.command_input = match self.ui.history_cursor {
            Some(i) => self.ui.command_history[i].clone(),
            None => String::new(),
        };
    }

    fn post_load_cleanup(&mut self) {
        // A new structure invalidates the command language's per-atom tables
        // even when the atom count happens to match the old one.
        self.atom_table_dirty = true;
        // A newly loaded structure may have a different atom count than a
        // trajectory still held from an earlier file. Applying that trajectory
        // would either index the new molecule with stale positions or silently
        // resurrect the old molecule from `base_molecule`, so drop a trajectory
        // that no longer matches. One whose atom count still fits (the same
        // system reloaded) is kept so a reload does not throw it away.
        if let Some(mol) = self.molecule.as_ref() {
            let atom_count = mol.atoms.len();
            let stale = self
                .trajectory
                .first()
                .is_some_and(|frame| frame.positions.len() != atom_count);
            if stale {
                self.trajectory.clear();
                self.trajectory_path = None;
                self.base_molecule = None;
                self.traj_ui.current_frame = 0;
                self.traj_ui.is_playing = false;
                self.traj_ui.interp_sub = 0;
            }
        }

        // A freshly loaded structure invalidates any prior atom selection (its
        // indices refer to the old molecule); clear it so `rebuild_viewport`
        // does not project stale indices onto the new geometry.
        self.selection.selected_atom_indices.clear();
        self.refresh_components();
        // When a dot surface is present, draw it through the dedicated surface
        // renderer and hide the raw "DOT" atoms from the main geometry so they
        // do not show up as a blob of large spheres on top of the surface.
        if !self.surface_dots.is_empty() {
            let _ = self.components.set_visible(SURFACE_RES_NAME, false);
        }
        self.refresh_surface_state();
        self.sync_viewer_molecule_and_focus();
        // Redraw the other layers as spheres in case atom counts/positions moved.
        self.refresh_layer_overlays();
    }

    /// Collect the base surface (if visible) and every visible overlay surface
    /// into a single layered render state and push it to the viewport.
    fn refresh_surface_state(&mut self) {
        let mut layers: Vec<PointCloudLayer> = Vec::new();
        if self.surface_visible && !self.surface_dots.is_empty() {
            layers.push(PointCloudLayer {
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
                layers.push(PointCloudLayer {
                    positions: overlay.dots.clone(),
                    color: (overlay.color[0], overlay.color[1], overlay.color[2]),
                });
            }
        }
        self.viewport.set_state_by_type(PointCloudState { layers });
    }

    /// Open a file dialog to add an overlay surface from a PDB with a dot surface.
    pub fn open_overlay_surface_file(&mut self) {
        self.spawn_pick(PickKind::OverlaySurface, || {
            FileDialog::new()
                .add_filter("Surface PDB", &["pdb", "ent"])
                .set_title("Add overlay surface (PDB with DOT surface)")
                .pick_file()
                .into_iter()
                .collect()
        });
    }

    /// Apply an overlay surface's dots (parsed off-thread by
    /// [`Self::parse_overlay`]) as a new, distinctly-coloured overlay.
    fn apply_overlay(&mut self, dots: Vec<Vec3>, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

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
        self.layers[a].components = std::mem::take(&mut self.components);
        self.layers[a].visibility = std::mem::take(&mut self.visibility);
        self.layers[a].interaction_pairs = std::mem::take(&mut self.interaction_pairs);
        self.layers[a].surface_dots = std::mem::take(&mut self.surface_dots);
        self.layers[a].surface_visible = self.surface_visible;
        self.layers[a].martini_ff = self.martini_ff.take();
        self.layers[a].bead_types = std::mem::take(&mut self.bead_types);
        self.layers[a].martini_visible = self.martini_visible;
        self.layers[a].selector_input = std::mem::take(&mut self.ui.selector_input);
        self.layers[a].ndx_groups = std::mem::take(&mut self.ui.ndx_groups);
        self.layers[a].ndx_visible = self.ui.ndx_visible;
        self.layers[a].ndx_opacity = self.ui.ndx_opacity;
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
        self.components = std::mem::take(&mut self.layers[idx].components);
        self.visibility = std::mem::take(&mut self.layers[idx].visibility);
        // The incoming layer's atom table has to be rebuilt for its molecule.
        self.atom_table = None;
        self.atom_table_dirty = true;
        self.interaction_pairs = std::mem::take(&mut self.layers[idx].interaction_pairs);
        self.surface_dots = std::mem::take(&mut self.layers[idx].surface_dots);
        self.surface_visible = self.layers[idx].surface_visible;
        self.martini_ff = self.layers[idx].martini_ff.take();
        self.bead_types = std::mem::take(&mut self.layers[idx].bead_types);
        // The restored bead types already match this layer's molecule.
        self.bead_types_dirty = false;
        self.martini_visible = self.layers[idx].martini_visible;
        self.ui.selector_input = std::mem::take(&mut self.layers[idx].selector_input);
        self.ui.ndx_groups = std::mem::take(&mut self.layers[idx].ndx_groups);
        self.ui.ndx_visible = self.layers[idx].ndx_visible;
        self.ui.ndx_opacity = self.layers[idx].ndx_opacity;
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
        self.set_sim_cell(box_diag);
    }

    /// Push the simulation box to the viewport, and size the XYZ triad to match
    /// so each coloured arm runs along the box edge leaving the origin.
    ///
    /// The two states are written together because the axis overlay takes its
    /// length as an explicit input rather than reading the cell state itself —
    /// that keeps the two overlays independent in the library.
    fn set_sim_cell(&mut self, size: (f32, f32, f32)) {
        self.sim_cell = size;
        self.viewport
            .set_state_by_type(SimulationCellState::new(Vec3::new(size.0, size.1, size.2)));
        self.refresh_axes_state();
    }

    /// Push the current axis visibility and triad length to the viewport.
    fn refresh_axes_state(&mut self) {
        let (x, y, z) = self.sim_cell;
        let length = (x > 0.0 || y > 0.0 || z > 0.0).then(|| Vec3::new(x, y, z));
        self.viewport.set_state_by_type(AxesState {
            visible: self.axis_visible,
            length,
        });
    }

    /// Rebuild the sphere geometry for every non-active, visible layer and push
    /// it to the viewport. Respects each layer's own residue-visibility filter.
    fn refresh_layer_overlays(&mut self) {
        let active = self.active_layer;
        let mut geoms: Vec<SphereSet> = Vec::new();
        for (i, layer) in self.layers.iter().enumerate() {
            if i == active || !layer.visible {
                continue;
            }
            let Some(mol) = layer.molecule.as_ref() else {
                continue;
            };
            let filtered = layer.components.any_hidden();
            let atoms: Vec<OverlaySphere> = mol
                .atoms
                .iter()
                .enumerate()
                .filter(|(orig, _)| !filtered || layer.components.is_atom_visible(*orig))
                .map(|(_, a)| {
                    let (r, g, b, _) = default_color_fn(a, false);
                    OverlaySphere {
                        position: a.position,
                        radius: ball_stick_radius(&a.element, false),
                        // Element colour, faded by the layer's opacity (alpha).
                        color: (r, g, b, layer.opacity),
                    }
                })
                .collect();
            if !atoms.is_empty() {
                geoms.push(SphereSet { spheres: atoms });
            }
        }
        self.viewport
            .set_state_by_type(SphereSetState { sets: geoms });
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
        self.spawn_pick(PickKind::StructureLayer, || {
            FileDialog::new()
                .add_filter("Structures", &["gro", "pdb", "ent", "cif", "mol2"])
                .add_filter("Topology", &["top", "itp"])
                .set_title("Load structure into new layer")
                .pick_file()
                .into_iter()
                .collect()
        });
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

    /// Leave each atom in only the *last* group that lists it, so overlapping
    /// NDX groups draw one sphere per atom instead of stacking several on the
    /// same position (which would z-fight).
    ///
    /// Groups routinely overlap — GROMACS ships `System` alongside `Protein`,
    /// `SOL` and friends — and later groups are the narrower ones, so letting
    /// them win keeps `Protein` showing through `System` rather than buried
    /// under it. `groups` is in NDX file order.
    fn resolve_ndx_overlaps(groups: &mut [Vec<usize>]) {
        let mut claimed: HashSet<usize> = HashSet::new();
        for atoms in groups.iter_mut().rev() {
            atoms.retain(|atom| claimed.insert(*atom));
        }
    }

    /// Rebuild the NDX highlight from every enabled group, mapping each one's
    /// entries into viewport space, dropping the atoms the residue filter
    /// currently hides, and resolving overlaps between the groups.
    fn refresh_ndx_selection_state(&mut self) {
        // The enabled groups' NDX file indices and their atoms in viewport
        // space, kept parallel and in file order.
        let mut group_indices: Vec<usize> = Vec::new();
        let mut atoms_per_group: Vec<Vec<usize>> = Vec::new();
        if self.ui.ndx_visible
            && let Some(ndx) = self.data.ndx_file.as_ref()
        {
            let atom_count = self.molecule.as_ref().map(|m| m.atoms.len()).unwrap_or(0);
            for (idx, group) in ndx.groups.iter().enumerate() {
                if !self.ui.ndx_groups.get(idx).is_some_and(|g| g.enabled) {
                    continue;
                }
                group_indices.push(idx);
                atoms_per_group.push(
                    Self::normalized_ndx_indices(&group.entries, atom_count)
                        .into_iter()
                        .filter_map(|orig| self.visibility.to_view(orig))
                        .collect(),
                );
            }
        }

        Self::resolve_ndx_overlaps(&mut atoms_per_group);

        let groups: Vec<AtomGroup> = group_indices
            .into_iter()
            .zip(atoms_per_group)
            .map(|(idx, atom_indices)| {
                let [r, g, b] = self.ndx_group_color(idx);
                AtomGroup {
                    atom_indices,
                    color: (r, g, b),
                }
            })
            .collect();

        self.ui.ndx_selected_atom_count = groups.iter().map(|g| g.atom_indices.len()).sum();
        self.viewport.set_state_by_type(AtomGroupState {
            groups,
            visible: self.ui.ndx_visible,
            opacity: self.ui.ndx_opacity,
        });
    }

    pub fn open_ndx_file(&mut self) {
        self.spawn_pick(PickKind::Ndx, || {
            FileDialog::new()
                .add_filter("NDX Files", &["ndx"])
                .set_title("Import NDX file")
                .pick_file()
                .into_iter()
                .collect()
        });
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

    /// Synchronous NDX load (drag-and-drop / reload / CLI). The file-dialog path
    /// loads through the async worker instead; both funnel into [`Self::apply_ndx`].
    fn load_ndx_file(&mut self, path: PathBuf) {
        let progress = Arc::new(LoadProgress::default());
        match Self::parse_ndx(&path, &progress) {
            Ok(LoadPayload::Ndx(ndx)) => self.apply_ndx(ndx, path),
            Ok(_) => unreachable!("parse_ndx only yields Ndx"),
            Err(msg) => self.set_status(msg),
        }
    }

    /// Apply a parsed NDX file: rebuild the per-group UI, store it, and draw the
    /// first group. Shared by the sync and async load paths.
    fn apply_ndx(&mut self, ndx: NdxFile, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();

        let group_count = ndx.groups.len();
        // Give every group a palette colour up front so the list shows a stable
        // swatch per group, but only draw the first one. A GROMACS file leads
        // with `System`, so enabling everything on import would paint the whole
        // structure at once; the user opts the rest in from the panel.
        self.ui.ndx_groups = (0..group_count)
            .map(|idx| NdxGroupUi {
                enabled: idx == 0,
                color: NDX_GROUP_PALETTE[idx % NDX_GROUP_PALETTE.len()],
            })
            .collect();
        self.data.ndx_file = Some(ndx);
        self.data.ndx_file_path = Some(path);
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

    pub fn ndx_group_enabled(&self, group_index: usize) -> bool {
        self.ui
            .ndx_groups
            .get(group_index)
            .is_some_and(|group| group.enabled)
    }

    pub fn set_ndx_group_enabled(&mut self, group_index: usize, enabled: bool) {
        let Some(group) = self.ui.ndx_groups.get_mut(group_index) else {
            return;
        };
        if group.enabled == enabled {
            return;
        }
        group.enabled = enabled;
        self.refresh_ndx_selection_state();

        let name = self
            .data
            .ndx_file
            .as_ref()
            .and_then(|ndx| ndx.groups.get(group_index))
            .map(|group| group.name.as_str())
            .unwrap_or("group");
        self.set_status(format!(
            "NDX group {} {} ({} atoms rendered)",
            name,
            if enabled { "shown" } else { "hidden" },
            self.ui.ndx_selected_atom_count
        ));
    }

    pub fn set_all_ndx_groups_enabled(&mut self, enabled: bool) {
        if self.ui.ndx_groups.is_empty() {
            return;
        }
        for group in &mut self.ui.ndx_groups {
            group.enabled = enabled;
        }
        self.refresh_ndx_selection_state();
        self.set_status(format!(
            "All NDX groups {} ({} atoms rendered)",
            if enabled { "shown" } else { "hidden" },
            self.ui.ndx_selected_atom_count
        ));
    }

    pub fn ndx_group_color(&self, group_index: usize) -> [f32; 3] {
        self.ui
            .ndx_groups
            .get(group_index)
            .map(|group| group.color)
            .unwrap_or(NDX_GROUP_PALETTE[0])
    }

    pub fn set_ndx_group_color(&mut self, group_index: usize, color: [f32; 3]) {
        let Some(group) = self.ui.ndx_groups.get_mut(group_index) else {
            return;
        };
        if group.color == color {
            return;
        }
        group.color = color;
        self.refresh_ndx_selection_state();
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

    /// Alpha of the NDX highlight spheres in `0.0..=1.0`. Held per layer and
    /// independent of [`layer_opacity`](Self::layer_opacity), which fades the
    /// structure itself.
    pub fn ndx_opacity(&self) -> f32 {
        self.ui.ndx_opacity
    }

    /// Set the NDX highlight alpha (clamped to `0.0..=1.0`) for the active layer.
    pub fn set_ndx_opacity(&mut self, opacity: f32) {
        let clamped = opacity.clamp(0.0, 1.0);
        if self.ui.ndx_opacity == clamped {
            return;
        }
        self.ui.ndx_opacity = clamped;
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
                ctrl && !shift && i.key_pressed(egui::Key::P),
                ctrl && !shift && i.key_pressed(egui::Key::E),
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
        if shortcuts.9 {
            // Nothing else in the app grabs keyboard focus on its own, so the
            // command bar can only be reached by a request the UI picks up on
            // the next frame.
            self.ui.focus_command_bar = true;
        }
        if shortcuts.10 {
            self.open_export_image_dialog();
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
        // Residue names feed both the default component names and `resname`
        // selections, so any rewrite here invalidates the cached atom table.
        self.atom_table_dirty = true;
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
        self.spawn_pick(PickKind::Structure, || {
            FileDialog::new()
                .add_filter("PDB Files", &["pdb", "ent", "cif"])
                .add_filter("MOL2 Files", &["mol2"])
                .pick_file()
                .into_iter()
                .collect()
        });
    }

    pub fn open_top_file(&mut self) {
        self.spawn_pick(PickKind::Top, || {
            FileDialog::new()
                .add_filter("TOP/ITP Files", &["top", "itp"])
                .set_title("Select TOP file")
                .pick_file()
                .into_iter()
                .collect()
        });
    }

    pub fn open_gro_file(&mut self) {
        self.spawn_pick(PickKind::Gro, || {
            FileDialog::new()
                .add_filter("GRO Files", &["gro"])
                .set_title("Select GRO file")
                .pick_file()
                .into_iter()
                .collect()
        });
    }

    pub fn open_top_and_gro_for_resname_sync(&mut self) {
        self.spawn_pick(PickKind::TopGroPair, || {
            let top_path = FileDialog::new()
                .add_filter("TOP Files", &["top"])
                .set_title("Select TOP file")
                .pick_file();
            let gro_path = FileDialog::new()
                .add_filter("GRO Files", &["gro"])
                .set_title("Select GRO file")
                .pick_file();
            match (top_path, gro_path) {
                (Some(top), Some(gro)) => vec![top, gro],
                _ => Vec::new(),
            }
        });
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
            self.set_sim_cell(boxsize);
            return true;
        }

        // The topology's expanded atom list and the GRO's are the same system
        // seen twice; if they disagree the pairing is wrong (a stale/short
        // `conf.gro`, a truncated one, or a GRO whose first malformed line cut
        // the atom list short). Refuse the pair with a message rather than
        // building a molecule whose bonds index atoms that do not exist — the
        // same contract the XTC path already applies.
        let top_atom_count = top.expanded_atom_types().len();
        if top_atom_count != gro.atoms.len() {
            self.set_status(format!(
                "TOP atom count ({}) does not match GRO atom count ({})",
                top_atom_count,
                gro.atoms.len()
            ));
            return false;
        }

        match top.generate_molecule_with_gro(&gro) {
            Ok((molecule, interaction_pairs)) => {
                let boxsize = gro.box_line;
                // Stored in original index space; rebuild_viewport() remaps and
                // pushes them whenever the visible set changes.
                self.interaction_pairs = interaction_pairs;
                self.surface_dots.clear();
                self.set_molecule_and_frame(molecule);
                self.set_sim_cell(boxsize);
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

    /// Synchronous TOP+GRO load (drag-and-drop / reload / CLI). The file-dialog
    /// path loads through the async worker instead; both funnel into
    /// [`Self::apply_topgro`].
    fn load_top_and_gro_for_resname_sync(&mut self, top_path: PathBuf, gro_path: PathBuf) {
        let progress = Arc::new(LoadProgress::default());
        match Self::parse_topgro(&top_path, &gro_path, &progress) {
            Ok(LoadPayload::TopGro { top, martini, gro }) => {
                self.apply_topgro(top, martini, gro, top_path, gro_path)
            }
            Ok(_) => unreachable!("parse_topgro only yields TopGro"),
            Err(msg) => self.set_status(msg),
        }
    }

    /// Apply a parsed TOP+GRO pair (residue-name sync). Shared by the sync and
    /// async load paths; `martini` is the force field the worker already parsed.
    fn apply_topgro(
        &mut self,
        top: TopFile,
        martini: Option<MartiniForceField>,
        gro: GroFile,
        top_path: PathBuf,
        gro_path: PathBuf,
    ) {
        self.data.top_file = Some(top);
        let martini_types = self.apply_martini_ff(martini);
        self.data.top_file_path = Some(top_path);
        self.data.structure_file = Some(StructureFile::Gro(gro));
        self.data.structure_file_path = Some(gro_path);

        let built = self.generate_and_set_molecule_from_stored_files();
        self.update_loaded_summary();
        self.mark_clean();
        // Keep the rejection message when the pair did not match; overwriting it
        // with "Loaded" would hide the only clue the user gets.
        if built {
            match martini_types {
                Some(n) => self.set_status(format!("Loaded Martini FF + GRO ({} bead types)", n)),
                None => self.set_status("Loaded TOP+GRO"),
            }
        }
        self.post_load_cleanup();
    }

    /// Synchronous TOP-only load (drag-and-drop / reload / CLI). The file-dialog
    /// path loads through the async worker instead; both funnel into
    /// [`Self::apply_top`].
    fn load_top_file_only(&mut self, path: PathBuf) {
        let progress = Arc::new(LoadProgress::default());
        match Self::parse_top(&path, &progress) {
            Ok(LoadPayload::Top(top, martini)) => self.apply_top(top, martini, path),
            Ok(_) => unreachable!("parse_top only yields Top"),
            Err(msg) => self.set_status(msg),
        }
    }

    /// Apply a parsed TOP file: register it and its Martini force field, then
    /// either build the molecule against an already-loaded GRO or wait for one.
    /// Shared by the sync and async load paths.
    fn apply_top(&mut self, top: TopFile, martini: Option<MartiniForceField>, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        self.data.top_file = Some(top);
        let martini_types = self.apply_martini_ff(martini);
        self.data.top_file_path = Some(path);

        let has_gro = self
            .data
            .structure_file
            .as_ref()
            .and_then(|s| s.gro())
            .is_some();

        if has_gro {
            let built = self.generate_and_set_molecule_from_stored_files();
            self.update_loaded_summary();
            self.mark_clean();
            self.post_load_cleanup();
            // Preserve the mismatch message set by the builder.
            if built {
                match martini_types {
                    Some(n) => self.set_status(format!(
                        "Loaded Martini force field: {} ({} bead types)",
                        file_name, n
                    )),
                    None => self.set_status(format!("Loaded TOP: {}", file_name)),
                }
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

    /// Synchronous GRO-only load (drag-and-drop / reload / CLI). The file-dialog
    /// path loads through the async worker instead; both funnel into
    /// [`Self::apply_gro`].
    fn load_gro_file_only(&mut self, path: PathBuf) {
        let progress = Arc::new(LoadProgress::default());
        match Self::parse_gro(&path, &progress) {
            Ok(LoadPayload::Gro(gro)) => self.apply_gro(gro, path),
            Ok(_) => unreachable!("parse_gro only yields Gro"),
            Err(msg) => self.set_status(msg),
        }
    }

    /// Apply a parsed GRO file: build the molecule (against a loaded TOP when
    /// present, else distance-inferred bonds) and set the simulation cell. Shared
    /// by the sync and async load paths.
    fn apply_gro(&mut self, gro: GroFile, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let boxsize = gro.box_line;
        self.data.structure_file = Some(StructureFile::Gro(gro));
        self.data.structure_file_path = Some(path);
        self.set_sim_cell(boxsize);

        let mut built = true;
        if self.data.top_file.is_some() {
            built = self.generate_and_set_molecule_from_stored_files();
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
        // Preserve the mismatch message when the GRO did not fit the loaded TOP.
        if built {
            self.set_status(format!("Loaded GRO: {}", file_name));
        }
        self.post_load_cleanup();
    }

    /// Synchronous structure load (drag-and-drop / reload / CLI), PDB/MOL2 by
    /// extension. The file-dialog path loads through the async worker instead;
    /// both funnel into [`Self::apply_pdb`]/[`Self::apply_mol2`]. `post_load_cleanup`
    /// runs on success exactly as before (it used to sit in this wrapper).
    pub fn load_file(&mut self, path: PathBuf) {
        let progress = Arc::new(LoadProgress::default());
        match Self::parse_structure(&path, &progress) {
            Ok(LoadPayload::Pdb(pdb)) => {
                self.apply_pdb(pdb, path);
                self.post_load_cleanup();
            }
            Ok(LoadPayload::Mol2(mol2)) => {
                self.apply_mol2(mol2, path);
                self.post_load_cleanup();
            }
            Ok(_) => unreachable!("parse_structure only yields Pdb/Mol2"),
            Err(msg) => self.set_status(msg),
        }
    }

    /// Apply a parsed PDB structure into the active layer. Shared by the sync and
    /// async load paths; the caller runs `post_load_cleanup` afterwards.
    fn apply_pdb(&mut self, pdb: PdbFile, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
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

    /// Apply a parsed MOL2 structure into the active layer. Shared by the sync and
    /// async load paths; the caller runs `post_load_cleanup` afterwards.
    fn apply_mol2(&mut self, mol2: Mol2File, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
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

    fn set_molecule_and_frame(&mut self, mut molecule: Molecule) {
        // Every molecule enters the app through here, so this is the one place
        // that can guarantee the invariant the renderer relies on: it indexes
        // `mol.atoms[bond.atom_a]` unchecked, so a single bond pointing past the
        // atom list (a topology declaring more atoms than the coordinate file
        // supplies, a truncated GRO, ...) would kill the process on the next
        // paint. Drop those bonds instead.
        let dropped = Self::drop_out_of_range_bonds(&mut molecule);
        if dropped > 0 {
            self.set_status(format!(
                "Ignored {} bond(s) referring to atoms the coordinate file does not contain",
                dropped
            ));
        }
        self.molecule = Some(molecule);
        // A new molecule (different atoms) invalidates the cached bead types.
        self.bead_types_dirty = true;
        // Syncing to the viewport is handled by post_load_cleanup() — callers are responsible.
    }

    /// Remove bonds whose endpoints are not valid atom indices, returning how
    /// many were dropped. Bond indices come from file content (a `.top`'s
    /// `[ bonds ]`, a MOL2 bond block) and are never validated against the
    /// coordinates they are paired with.
    fn drop_out_of_range_bonds(molecule: &mut Molecule) -> usize {
        let n = molecule.atoms.len();
        let before = molecule.bonds.len();
        molecule.bonds.retain(|b| b.atom_a < n && b.atom_b < n);
        before - molecule.bonds.len()
    }

    pub fn open_xtc_file(&mut self) {
        self.spawn_pick(PickKind::Xtc, || {
            FileDialog::new()
                .add_filter("XTC Trajectory", &["xtc"])
                .set_title("Select XTC trajectory file")
                .pick_file()
                .into_iter()
                .collect()
        });
    }

    /// Apply a parsed XTC trajectory: validate its atom count against the current
    /// structure, set the base molecule and trajectory, and show frame 0. Shared
    /// by the sync and async load paths.
    fn apply_xtc(&mut self, xtc: XtcFile, path: PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

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
        self.set_sim_cell(box_diag);

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
            // viewport-index order. Index through `.get()` rather than `[orig]`:
            // `view_to_orig` is rebuilt to match the molecule, but should it ever
            // lag behind a shorter `positions` (a trajectory frame narrower than
            // the current structure), an unchecked index would panic mid-repaint.
            // Fall back to the origin for any missing atom so the length still
            // matches what the viewport expects.
            if self.visibility.is_filtered() {
                let view_positions: Vec<Vec3> = self
                    .visibility
                    .view_to_orig
                    .iter()
                    .map(|&orig| positions.get(orig).copied().unwrap_or(Vec3::new(0.0, 0.0, 0.0)))
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
            // The base molecule was swapped in; its bead types may differ.
            self.bead_types_dirty = true;
            self.atom_table_dirty = true;
            self.refresh_components();
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

        // Route through the async worker (same as the file-dialog path) so a big
        // dropped/CLI file reads off the UI thread with a progress bar instead of
        // freezing — dropping a large `.xtc` is the very case the user hit.
        if let (Some(top), Some(gro)) = (top_path.clone(), gro_path.clone()) {
            self.spawn_load(PickKind::TopGroPair, vec![top, gro]);
            return;
        }

        if let Some(top) = top_path {
            self.spawn_load(PickKind::Top, vec![top]);
            return;
        }

        if let Some(gro) = gro_path {
            self.spawn_load(PickKind::Gro, vec![gro]);
            return;
        }

        if let Some(path) = xtc_path {
            self.spawn_load(PickKind::Xtc, vec![path]);
            return;
        }

        if let Some(path) = ndx_path {
            self.spawn_load(PickKind::Ndx, vec![path]);
            return;
        }

        if let Some(path) = other_path {
            self.spawn_load(PickKind::Structure, vec![path]);
        }
    }

    pub fn render_ui(&mut self, ctx: &egui::Context) {
        app_ui::render_edit_dialog(self, ctx);
    }

    fn select_shortest_path(&mut self, start: usize, end: usize) {
        let Some(mol) = &self.molecule else {
            return;
        };

        // Find the atoms on the shortest bonded path between start and end
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

        // 2. Breadth-first search for the shortest path, recording each atom's
        //    predecessor. This used to be a recursive DFS enumerating *every*
        //    simple path, which on file-sized graphs is fatal: recursion depth
        //    follows the longest path, so a few thousand bonded atoms blow the
        //    1 MB main-thread stack (no unwind, no panic message, the process is
        //    simply gone), and on cyclic graphs — which distance-inferred bonds
        //    always produce — the number of simple paths is exponential, so the
        //    UI thread never returns. BFS is O(V+E), iterative and allocates on
        //    the heap, and it answers what the caller actually asks for.
        let mut parent: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(start);
        parent.insert(start, start);

        let mut found = false;
        while let Some(current) = queue.pop_front() {
            if current == end {
                found = true;
                break;
            }
            if let Some(neighbors) = adj.get(&current) {
                for &neighbor in neighbors {
                    // `parent` doubles as the visited set, so every atom is
                    // expanded at most once.
                    if !parent.contains_key(&neighbor) {
                        parent.insert(neighbor, current);
                        queue.push_back(neighbor);
                    }
                }
            }
        }

        if !found {
            // Not bonded to each other: nothing lies "between" them.
            return Vec::new();
        }

        // 3. Walk the predecessors back to `start`, then return sorted.
        let mut all_path_atoms = std::collections::HashSet::new();
        let mut node = end;
        loop {
            all_path_atoms.insert(node);
            let Some(&prev) = parent.get(&node) else { break };
            if prev == node {
                break; // reached `start`, whose parent is itself
            }
            node = prev;
        }

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

        // The renamed atoms belong under a different residue now. While the
        // partition is still the residue-derived default, re-derive it so the
        // new name shows up in COMPONENTS as it always has. Once the user has
        // split or merged anything by hand, their grouping wins — silently
        // throwing it away would be worse than leaving it stale, so say so.
        if self.components.is_default_partition() {
            if let Some(mol) = self.molecule.clone() {
                self.components.rebuild_from_molecule(&mol);
            }
        } else {
            self.log_info(
                "residue names changed; COMPONENTS kept as-is (run 'reset' to re-derive)",
            );
        }

        // Clear selection
        self.selection.selected_atom_indices.clear();
        self.sync_selection_to_viewport();
        self.rebuild_viewport(false);
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

        // Deliver any file chosen by a background picker thread, then any parsed
        // payload from a background load worker. Keep repainting while either is
        // in flight so results are dispatched promptly and the progress bar
        // animates (egui otherwise idles with no pending input events).
        self.poll_pending_pick();
        self.poll_pending_load();
        if self.pending_pick.is_some() || self.pending_load.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        self.handle_dropped_files(&ctx);
        self.handle_keyboard_shortcuts(&ctx);
        // Fold clicks the viewport captured last frame into the selection before
        // the panels (which read the selection count / enablement) are drawn.
        self.process_atom_clicks();

        // Advance trajectory playback
        if self.traj_ui.is_playing {
            let t = ctx.input(|i| i.time);
            self.advance_trajectory_if_playing(t);
            // Wake again only when the next (sub-)frame is actually due instead of
            // busy-repainting at the monitor's refresh rate: playback advances at
            // `playback_fps` × `interp_steps`, so most max-rate repaints would just
            // redraw an unchanged (non-dirty) scene and burn CPU/GPU. Schedule the
            // next repaint for when `advance_trajectory_if_playing` will next act.
            let steps = self.traj_ui.interp_steps.max(1);
            let interval = 1.0 / (self.traj_ui.playback_fps.max(0.1) as f64 * steps as f64);
            let due_in = (self.traj_ui.last_advance_time + interval - t).max(0.0);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(due_in));
        }

        // Run any queued image-export render before the viewport is drawn: it
        // resizes the shared color target, and `viewport.show` below is what
        // puts it back to the on-screen size.
        self.process_pending_image_export(&ctx);

        // egui 0.35: panels are shown into the root `ui`, not the context.
        app_ui::render_menu_bar(self, ui);
        app_ui::render_bottom_status_bar(self, ui);
        // Shown after the status bar so it stacks directly above it, and before
        // the left panel so it spans the full window width.
        app_ui::render_command_bar(self, ui);
        app_ui::render_left_panel(self, ui);
        app_ui::render_overlay_panel(self, ui);
        app_ui::render_bottom_dock(self, ui);
        app_ui::render_edit_dialog(self, &ctx);
        app_ui::render_export_dialog(self, &ctx);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_ndx_groups_keep_each_atom_in_the_last_group() {
        // A broad group (`System`-like) listed before two narrower ones, all
        // three overlapping.
        let mut groups = vec![
            vec![0, 1, 2, 3, 4], // System
            vec![1, 2],          // Protein
            vec![3],             // Ligand
        ];
        KuromameApp::resolve_ndx_overlaps(&mut groups);

        assert_eq!(
            groups[0],
            vec![0, 4],
            "System keeps only what nothing else claims"
        );
        assert_eq!(
            groups[1],
            vec![1, 2],
            "Protein wins over the earlier System"
        );
        assert_eq!(groups[2], vec![3], "Ligand wins over the earlier System");
    }

    #[test]
    fn resolve_ndx_overlaps_draws_every_atom_exactly_once() {
        let mut groups = vec![vec![5, 6, 7], vec![6, 7, 8], vec![7, 8, 9]];
        KuromameApp::resolve_ndx_overlaps(&mut groups);

        let drawn: Vec<usize> = groups.iter().flatten().copied().collect();
        let mut unique = drawn.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(drawn.len(), unique.len(), "no atom is drawn by two groups");
        assert_eq!(unique, vec![5, 6, 7, 8, 9], "no atom is dropped either");
    }

    #[test]
    fn resolve_ndx_overlaps_leaves_disjoint_groups_alone() {
        let mut groups = vec![vec![0, 1], vec![2, 3]];
        KuromameApp::resolve_ndx_overlaps(&mut groups);
        assert_eq!(groups, vec![vec![0, 1], vec![2, 3]]);
    }

    /// Build a chain-free molecule of `n` atoms plus the given bonds.
    fn test_molecule(n: usize, bonds: &[(usize, usize)]) -> Molecule {
        let atoms = (0..n)
            .map(|i| view_atom(Vec3::new(i as f32, 0.0, 0.0), "C", i, None))
            .collect();
        let bonds = bonds
            .iter()
            .map(|&(a, b)| Bond {
                atom_a: a,
                atom_b: b,
                order: 1,
            })
            .collect();
        molecule_from_parts(atoms, bonds)
    }

    #[test]
    fn out_of_range_bonds_are_dropped_from_the_molecule() {
        // A TOP describing 4 atoms loaded against a 2-atom GRO produces exactly
        // this: bonds indexing atoms the coordinates never provided.
        let mut mol = test_molecule(2, &[(0, 1), (1, 2), (2, 3)]);
        let dropped = KuromameApp::drop_out_of_range_bonds(&mut mol);

        assert_eq!(dropped, 2, "both bonds reaching past atom 1 are dropped");
        assert_eq!(mol.bonds.len(), 1);
        assert_eq!((mol.bonds[0].atom_a, mol.bonds[0].atom_b), (0, 1));
    }

    #[test]
    fn well_formed_bonds_survive_untouched() {
        let mut mol = test_molecule(3, &[(0, 1), (1, 2)]);
        assert_eq!(KuromameApp::drop_out_of_range_bonds(&mut mol), 0);
        assert_eq!(mol.bonds.len(), 2);
    }

    #[test]
    fn select_between_takes_the_shortest_route_around_a_ring() {
        // Six-membered ring: 0-1-2-3-4-5-0. Between 0 and 3 both arcs are three
        // bonds long, so either is acceptable, but the result must never be the
        // whole ring (the old all-simple-paths walk returned every atom).
        let mol = test_molecule(6, &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0)]);
        let path = KuromameApp::find_atoms_between_dfs(&mol, 0, 3);

        assert_eq!(path.len(), 4, "start, end and the two atoms in between");
        assert!(path.contains(&0) && path.contains(&3));
    }

    #[test]
    fn select_between_survives_a_chain_far_longer_than_the_stack() {
        // The previous recursive DFS pushed one frame per atom, so a chain this
        // long killed the process with STATUS_STACK_OVERFLOW on Windows' 1 MB
        // main-thread stack. BFS must simply return the whole chain.
        const N: usize = 20_000;
        let bonds: Vec<(usize, usize)> = (0..N - 1).map(|i| (i, i + 1)).collect();
        let mol = test_molecule(N, &bonds);

        let path = KuromameApp::find_atoms_between_dfs(&mol, 0, N - 1);
        assert_eq!(path.len(), N);
    }

    #[test]
    fn select_between_returns_nothing_for_unconnected_atoms() {
        let mol = test_molecule(4, &[(0, 1), (2, 3)]);
        assert!(KuromameApp::find_atoms_between_dfs(&mol, 0, 3).is_empty());
    }

    #[test]
    fn progress_reader_forwards_bytes_and_tracks_done() {
        // Every byte read through the wrapper reaches the consumer unchanged and
        // is counted in `done`, which is what drives the byte-fraction bar.
        let data = b"hello world".to_vec();
        let progress = Arc::new(LoadProgress::default());
        let mut reader = ProgressReader {
            inner: std::io::Cursor::new(data.clone()),
            progress: Arc::clone(&progress),
        };
        let mut out = Vec::new();
        let n = reader.read_to_end(&mut out).expect("read succeeds");
        assert_eq!(n, data.len());
        assert_eq!(out, data, "bytes pass through untouched");
        assert_eq!(
            progress.done.load(Ordering::Relaxed),
            data.len() as u64,
            "done reflects the bytes read"
        );
    }

    #[test]
    fn progress_reader_errors_when_cancelled() {
        // A set cancel flag makes the very next read fail with a non-retriable
        // error, so an in-flight parse unwinds instead of finishing abandoned
        // work. The kind must NOT be `Interrupted`: std's read helpers treat that
        // as "retry" and, since `cancel` stays set, would spin the worker forever.
        let progress = Arc::new(LoadProgress::default());
        progress.cancel.store(true, Ordering::Relaxed);
        let mut reader = ProgressReader {
            inner: std::io::Cursor::new(vec![1u8, 2, 3]),
            progress: Arc::clone(&progress),
        };
        let mut buf = [0u8; 3];
        let err = reader.read(&mut buf).expect_err("cancelled read errors");
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
        assert_ne!(
            err.kind(),
            std::io::ErrorKind::Interrupted,
            "must not be Interrupted or std read helpers would retry forever"
        );
        assert_eq!(
            progress.done.load(Ordering::Relaxed),
            0,
            "nothing was consumed on a cancelled read"
        );
    }
}
