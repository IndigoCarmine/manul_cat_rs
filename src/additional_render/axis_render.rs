use crate::simulation_cell_render::SimulationCellRenderState;
use lin_alg::f32::Vec3;
use moleucle_3dview_rs::{
    AdditionalRender, GpuPipeline, RenderFrameState, Scene, SharedRenderStates,
    render_state::get_state_clone_by_type,
};

/// Draws an X/Y/Z orientation triad at the world origin — which, for a GROMACS
/// system, is the corner of the simulation box. Three solid arrows follow the
/// universal colour convention (X red, Y green, Z blue), each capped with a small
/// ball marking its positive direction, so the viewer can always tell which way
/// the coordinate axes point.
///
/// The arrows are sized to the simulation box when one is loaded (so each colour
/// runs along the box edge leaving the origin), otherwise to the molecule's
/// extent. Drawn solid (`Triangles` pipeline) so they stand out against the
/// wireframe cell and the molecule.
pub struct AxisRender;

impl AxisRender {
    pub fn new() -> Self {
        Self
    }
}

/// Whether the XYZ axis triad is drawn. Stored in `SharedRenderStates`; an absent
/// state counts as visible, so the axes show by default until toggled off.
#[derive(Clone)]
pub struct AxisRenderState {
    pub visible: bool,
}

impl AdditionalRender for AxisRender {
    fn gpu_pipeline(&self) -> GpuPipeline {
        GpuPipeline::Triangles
    }

    fn update_scene(&self, scene: &mut Scene, frame_state: &RenderFrameState<'_>) {
        let states: &SharedRenderStates = match frame_state.shared_states {
            Some(states) => states,
            None => return,
        };

        let visible = get_state_clone_by_type::<AxisRenderState>(states)
            .map(|state| state.visible)
            .unwrap_or(true);
        if !visible {
            return;
        }

        // Per-axis length: prefer the simulation box so each coloured arrow lines
        // up with the box edge leaving the origin; fall back to the molecule's
        // bounding radius (or a small default) when there is no box.
        let box_size = get_state_clone_by_type::<SimulationCellRenderState>(states)
            .map(|state| state.size())
            .unwrap_or((0.0, 0.0, 0.0));
        let fallback = frame_state
            .molecule
            .map(|molecule| molecule.radius())
            .filter(|radius| *radius > 0.0)
            .unwrap_or(2.0);
        let axis_len = |edge: f32| if edge > 0.0 { edge } else { fallback };
        let (lx, ly, lz) = (axis_len(box_size.0), axis_len(box_size.1), axis_len(box_size.2));

        // Scale the shaft/tip to the triad so it reads at any zoom, with a small
        // floor so it never vanishes on a tiny structure.
        let max_len = lx.max(ly).max(lz);
        let shaft_radius = (max_len * 0.012).max(0.02);
        let tip_radius = (max_len * 0.03).max(0.05);
        let origin = Vec3::new(0.0, 0.0, 0.0);

        let axes = [
            (Vec3::new(lx, 0.0, 0.0), (0.90, 0.20, 0.20)), // X — red
            (Vec3::new(0.0, ly, 0.0), (0.20, 0.75, 0.25)), // Y — green
            (Vec3::new(0.0, 0.0, lz), (0.25, 0.45, 0.95)), // Z — blue
        ];
        for (end, (r, g, b)) in axes {
            self.add_cylinder(scene, origin, end, shaft_radius, (r, g, b, 1.0));
            self.add_sphere(scene, frame_state, end, tip_radius, (r, g, b, 1.0));
        }
    }
}
