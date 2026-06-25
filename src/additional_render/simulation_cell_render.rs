use lin_alg::f32::Vec3;
use moleucle_3dview_rs::{
    AdditionalRender, RenderFrameState, Scene, SharedRenderStates,
    render_state::get_state_clone_by_type,
};

pub struct SimulationCellRender {
    color: (f32, f32, f32),
}
impl SimulationCellRender {
    pub fn new() -> Self {
        Self {
            color: (0.5, 0.5, 0.5),
        }
    }
}

#[derive(Clone)]
pub struct SimulationCellRenderState {
    size: (f32, f32, f32),
}
impl SimulationCellRenderState {
    pub fn new(size: (f32, f32, f32)) -> Self {
        Self { size }
    }
}

impl AdditionalRender for SimulationCellRender {
    fn update_scene(&self, scene: &mut Scene, frame_state: &RenderFrameState<'_>) {
        let states: &SharedRenderStates = if let Some(states) = frame_state.shared_states {
            states
        } else {
            return;
        };

        let state = get_state_clone_by_type::<SimulationCellRenderState>(states)
            .unwrap_or(SimulationCellRenderState::new((0.0, 0.0, 0.0)));

        let size = state.size;
        let lines = [
            //x-axis_direction
            [[0.0, 0.0, 0.0], [size.0, 0.0, 0.0]],
            [[0.0, size.1, 0.0], [size.0, size.1, 0.0]],
            [[0.0, 0.0, size.2], [size.0, 0.0, size.2]],
            [[0.0, size.1, size.2], [size.0, size.1, size.2]],
            //y-axis_direction
            [[0.0, 0.0, 0.0], [0.0, size.1, 0.0]],
            [[size.0, 0.0, 0.0], [size.0, size.1, 0.0]],
            [[0.0, 0.0, size.2], [0.0, size.1, size.2]],
            [[size.0, 0.0, size.2], [size.0, size.1, size.2]],
            //z-axis_direction
            [[0.0, 0.0, 0.0], [0.0, 0.0, size.2]],
            [[size.0, 0.0, 0.0], [size.0, 0.0, size.2]],
            [[0.0, size.1, 0.0], [0.0, size.1, size.2]],
            [[size.0, size.1, 0.0], [size.0, size.1, size.2]],
        ];

        // Build the box edges as thin cylinders so the Wireframe pipeline renders them
        let edge_radius = 0.01_f32;
        for &edge in &lines {
            let start = Vec3::new(edge[0][0], edge[0][1], edge[0][2]);
            let end = Vec3::new(edge[1][0], edge[1][1], edge[1][2]);
            self.add_cylinder(scene, start, end, edge_radius, self.color);
        }
    }

    fn gpu_pipeline(&self) -> moleucle_3dview_rs::GpuPipeline {
        moleucle_3dview_rs::GpuPipeline::Wireframe
    }
}
