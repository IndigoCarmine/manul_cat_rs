use lin_alg::f32::Vec3;
use moleucle_3dview_rs::{
    AdditionalRender, GpuPipeline, RenderFrameState, Scene, SharedRenderStates,
    render_state::get_state_clone_by_type,
};

/// Draws every non-active document layer as element-coloured spheres on top of
/// the active layer's main molecule, so all loaded structures can be compared at
/// once. Only the active layer is drawn as the viewer's full ball+stick molecule
/// (the pinned viewer renders bonds/picking for a single molecule); the rest are
/// sphere impostors, which stays cheap for large systems.
///
/// The per-layer sphere geometry is supplied through [`LayerOverlayState`] and is
/// recomputed by the app whenever the active layer, a layer's visibility, or a
/// layer's structure changes.
pub struct LayerOverlayRender;

/// A single overlay atom: position (nm), sphere radius (nm) and RGBA colour, all
/// precomputed by the app. The alpha component carries the owning layer's
/// opacity so faded layers blend through to what is behind them.
#[derive(Clone, Copy)]
pub struct OverlayAtom {
    pub position: Vec3,
    pub radius: f32,
    pub color: (f32, f32, f32, f32),
}

/// One non-active layer's spheres.
#[derive(Clone)]
pub struct LayerOverlayGeom {
    pub atoms: Vec<OverlayAtom>,
}

/// State for the non-active layer overlays stored in `SharedRenderStates`. Only
/// the layers that should currently be drawn are included.
#[derive(Clone, Default)]
pub struct LayerOverlayState {
    pub layers: Vec<LayerOverlayGeom>,
}

impl LayerOverlayRender {
    pub fn new() -> Self {
        Self
    }
}

impl AdditionalRender for LayerOverlayRender {
    fn gpu_pipeline(&self) -> GpuPipeline {
        GpuPipeline::SphereImpostor
    }

    fn update_scene(&self, scene: &mut Scene, frame_state: &RenderFrameState<'_>) {
        let Some(states): Option<&SharedRenderStates> = frame_state.shared_states else {
            return;
        };
        let Some(state) = get_state_clone_by_type::<LayerOverlayState>(states) else {
            return;
        };

        for layer in &state.layers {
            for atom in &layer.atoms {
                self.add_sphere(scene, frame_state, atom.position, atom.radius, atom.color);
            }
        }
    }
}
