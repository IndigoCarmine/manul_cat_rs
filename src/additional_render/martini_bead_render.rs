use moleucle_3dview_rs::{
    AdditionalRender, GpuPipeline, Molecule, RenderFrameState, Scene, SharedRenderStates,
    render_state::get_state_clone_by_type,
};

/// Draws Martini coarse-grained beads: one sphere per particle, sized by the
/// bead's Lennard-Jones sigma and coloured by its chemical class (see
/// [`crate::parsing::MartiniForceField`]).
///
/// The per-bead radius/colour are supplied through [`MartiniBeadState`], indexed
/// to match the molecule currently handed to the viewport (view-index order).
/// Positions are read live from `frame_state.molecule`, so beads follow
/// trajectory playback for free, exactly like [`crate::ndx_selection_render`].
///
/// Beads render as sphere impostors so a whole coarse-grained system stays cheap
/// even at hundreds of thousands of particles.
pub struct MartiniBeadRender;

/// One bead's appearance. `radius` is in nanometers (the viewer's unit).
#[derive(Clone, Copy)]
pub struct BeadStyle {
    pub radius: f32,
    pub color: (f32, f32, f32),
}

/// Per-atom bead styling stored in `SharedRenderStates`. `styles[i]` styles the
/// viewport's atom `i`; `None` means "not a Martini bead — leave the default
/// element sphere to represent it".
#[derive(Clone, Default)]
pub struct MartiniBeadState {
    pub styles: Vec<Option<BeadStyle>>,
    pub visible: bool,
}

impl MartiniBeadRender {
    pub fn new() -> Self {
        Self
    }
}

impl AdditionalRender for MartiniBeadRender {
    fn gpu_pipeline(&self) -> GpuPipeline {
        GpuPipeline::SphereImpostor
    }

    fn update_scene(&self, scene: &mut Scene, frame_state: &RenderFrameState<'_>) {
        let Some(molecule): Option<&Molecule> = frame_state.molecule else {
            return;
        };
        let Some(states): Option<&SharedRenderStates> = frame_state.shared_states else {
            return;
        };
        let Some(state) = get_state_clone_by_type::<MartiniBeadState>(states) else {
            return;
        };
        if !state.visible {
            return;
        }

        for (atom, style) in molecule.atoms.iter().zip(state.styles.iter()) {
            let Some(style) = style else {
                continue;
            };
            self.add_sphere(scene, frame_state, atom.position, style.radius, style.color);
        }
    }
}
