use lin_alg::f32::{Quaternion, Vec3};
use moleucle_3dview_rs::scene_types::Vertex;
use moleucle_3dview_rs::{
    AdditionalRender, Entity, GpuPipeline, Mesh, RenderFrameState, Scene, SharedRenderStates,
    render_state::get_state_clone_by_type,
};

/// Renders a molecular dot surface (e.g. the Connolly / SASA "DOT" surface that
/// `gmx sasa` writes into a PDB). Each dot is drawn as a small 3-axis cross via
/// the wireframe pipeline, so the surface is always shown as a see-through
/// wireframe regardless of the viewer's current render style. The dot positions
/// are supplied through [`SurfaceMeshState`] and are independent of the loaded
/// molecule, so the surface stays put even while a trajectory animates the atoms.
pub struct SurfaceMeshRender {
    color: (f32, f32, f32),
    /// Half-length of each cross arm, in nanometers.
    radius: f32,
}

/// State type for the surface dot cloud stored in SharedRenderStates.
///
/// `positions` are in the crate's nanometer units.
#[derive(Clone, Default)]
pub struct SurfaceMeshState {
    pub positions: Vec<Vec3>,
    pub visible: bool,
}

impl SurfaceMeshRender {
    pub fn new() -> Self {
        Self {
            color: (0.35, 0.72, 0.95),
            radius: 0.04,
        }
    }

    pub fn set_color(&mut self, color: (f32, f32, f32)) {
        self.color = color;
    }

    pub fn set_radius(&mut self, radius: f32) {
        self.radius = radius;
    }

    /// Unit cross mesh: three axis-aligned segments through the origin.
    ///
    /// The wireframe pipeline draws its vertex stream as a `LineList` (vertex
    /// pairs), and [`Scene`] geometry reaches it as triangles emitted three
    /// vertices at a time. Laying the six endpoints out in this index order
    /// makes the emitted stream pair up as `(-x,+x)`, `(-y,+y)`, `(-z,+z)` — the
    /// three cross arms — with no stray connecting lines.
    fn unit_cross_mesh() -> Mesh {
        let n = Vec3::new(0.0, 1.0, 0.0);
        Mesh {
            vertices: vec![
                Vertex::new([-1.0, 0.0, 0.0], n),
                Vertex::new([1.0, 0.0, 0.0], n),
                Vertex::new([0.0, -1.0, 0.0], n),
                Vertex::new([0.0, 1.0, 0.0], n),
                Vertex::new([0.0, 0.0, -1.0], n),
                Vertex::new([0.0, 0.0, 1.0], n),
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
        }
    }
}

impl AdditionalRender for SurfaceMeshRender {
    fn gpu_pipeline(&self) -> GpuPipeline {
        GpuPipeline::Wireframe
    }

    fn update_scene(&self, scene: &mut Scene, frame_state: &RenderFrameState<'_>) {
        let states: &SharedRenderStates = if let Some(states) = frame_state.shared_states {
            states
        } else {
            return;
        };

        let Some(state) = get_state_clone_by_type::<SurfaceMeshState>(states) else {
            return;
        };

        if !state.visible || state.positions.is_empty() {
            return;
        }

        let mesh_idx = scene.meshes.len();
        scene.meshes.push(Self::unit_cross_mesh());

        scene.entities.reserve(state.positions.len());
        for &position in &state.positions {
            scene.entities.push(Entity::new(
                mesh_idx,
                position,
                Quaternion::new_identity(),
                self.radius,
                self.color,
                0.1,
            ));
        }
    }
}
