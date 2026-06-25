use moleucle_3dview_rs::{
    AdditionalRender, Molecule, RenderFrameState, Scene, SharedRenderStates,
    render_state::get_state_clone_by_type, vdw_radius,
};

#[derive(Clone)]
pub struct InterMolecularInteractionRender {
    color: (f32, f32, f32),
    radius: f32,
}

/// State type for interaction pairs stored in SharedRenderStates
#[derive(Clone)]
pub struct InteractionPairsState {
    pub pairs: Vec<(usize, usize)>,
}

impl InterMolecularInteractionRender {
    pub fn new() -> Self {
        Self {
            color: (1.0, 0.0, 0.0),        // Default to red color
            radius: vdw_radius("C") * 0.2, // Default radius based on carbon VDW radius
        }
    }

    pub fn set_color(&mut self, color: (f32, f32, f32)) {
        self.color = color;
    }

    pub fn set_radius(&mut self, radius: f32) {
        self.radius = radius;
    }
}

impl AdditionalRender for InterMolecularInteractionRender {
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

        // Retrieve interaction pairs from shared state using type as key
        let Some(state) = get_state_clone_by_type::<InteractionPairsState>(states) else {
            return;
        };

        if state.pairs.is_empty() {
            return;
        }

        for &(ai, aj) in &state.pairs {
            let Some(ai) = ai.checked_sub(1) else {
                continue;
            };
            let Some(aj) = aj.checked_sub(1) else {
                continue;
            };

            let Some(atom_a) = molecule.atoms.get(ai) else {
                continue;
            };
            let Some(atom_b) = molecule.atoms.get(aj) else {
                continue;
            };

            let direction = atom_b.position - atom_a.position;
            if direction.magnitude() < 1.0e-6 {
                continue;
            }

            self.add_cylinder(
                scene,
                atom_a.position,
                atom_b.position,
                self.radius,
                self.color,
            );
        }
    }
}
