use moleucle_3dview_rs::{
    AdditionalRender, GpuPipeline, Molecule, RenderFrameState, Scene, SharedRenderStates,
    render_state::get_state_clone_by_type, vdw_radius,
};

#[derive(Clone)]
pub struct NdxSelectionRender {
    radius: f32,
}

/// One NDX group's atoms (viewport indices) and the colour they are drawn in.
/// The app assigns every atom to at most one group, so overlapping groups never
/// stack two spheres on the same position.
#[derive(Clone)]
pub struct NdxSelectionGroup {
    pub atom_indices: Vec<usize>,
    pub color: (f32, f32, f32),
}

/// State type for the NDX groups currently drawn, stored in SharedRenderStates.
#[derive(Clone, Default)]
pub struct NdxSelectionState {
    pub groups: Vec<NdxSelectionGroup>,
    pub visible: bool,
}

impl NdxSelectionRender {
    pub fn new() -> Self {
        Self {
            radius: vdw_radius("C") * 0.5,
        }
    }

    pub fn set_radius(&mut self, radius: f32) {
        self.radius = radius;
    }
}

impl AdditionalRender for NdxSelectionRender {
    fn gpu_pipeline(&self) -> GpuPipeline {
        GpuPipeline::SphereImpostor
    }

    fn update_scene(&self, scene: &mut Scene, frame_state: &RenderFrameState<'_>) {
        let molecule: &Molecule = if let Some(molecule) = frame_state.molecule {
            molecule
        } else {
            return;
        };
        let states: &SharedRenderStates = if let Some(states) = frame_state.shared_states {
            states
        } else {
            return;
        };

        let Some(state) = get_state_clone_by_type::<NdxSelectionState>(states) else {
            return;
        };

        if !state.visible {
            return;
        }

        for group in &state.groups {
            for &atom_index in &group.atom_indices {
                let Some(atom) = molecule.atoms.get(atom_index) else {
                    continue;
                };

                self.add_sphere(
                    scene,
                    frame_state,
                    atom.position,
                    self.radius,
                    (group.color.0, group.color.1, group.color.2, 1.0),
                );
            }
        }
    }
}
