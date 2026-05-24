use moleucle_3dview_rs::{
    AdditionalRender, Molecule, RenderFrameState, Scene, SharedRenderStates,
    render_state::get_state_clone_by_type, vdw_radius,
};

#[derive(Clone)]
pub struct NdxSelectionRender {
    color: (f32, f32, f32),
    radius: f32,
}

/// State type for NDX-selected atom indices stored in SharedRenderStates.
#[derive(Clone, Default)]
pub struct NdxSelectionState {
    pub atom_indices: Vec<usize>,
    pub visible: bool,
}

impl NdxSelectionRender {
    pub fn new() -> Self {
        Self {
            color: (1.0, 0.6, 0.0),
            radius: vdw_radius("C") * 0.24,
        }
    }

    pub fn set_color(&mut self, color: (f32, f32, f32)) {
        self.color = color;
    }

    pub fn set_radius(&mut self, radius: f32) {
        self.radius = radius;
    }
}

impl AdditionalRender for NdxSelectionRender {
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

        if !state.visible || state.atom_indices.is_empty() {
            return;
        }

        for &atom_index in &state.atom_indices {
            let Some(atom) = molecule.atoms.get(atom_index) else {
                continue;
            };
            self.add_sphere(scene, atom.position, self.radius, self.color);
        }
    }
}
