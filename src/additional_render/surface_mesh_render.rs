use lin_alg::f32::{Quaternion, Vec3};
use moleucle_3dview_rs::scene_types::Vertex;
use moleucle_3dview_rs::{
    AdditionalRender, Entity, GpuPipeline, Mesh, RenderFrameState, Scene, SharedRenderStates,
    render_state::get_state_clone_by_type,
};

/// Renders one or more molecular dot surfaces (e.g. the Connolly / SASA "DOT"
/// surface that `gmx sasa` writes into a PDB). Each dot is drawn as a small
/// 3-axis cross via the wireframe pipeline, so surfaces are always shown as a
/// see-through wireframe regardless of the viewer's current render style.
///
/// Several surfaces can be overlaid at once; each [`SurfaceLayer`] carries its
/// own color so multiple loaded files can be told apart. Positions are supplied
/// through [`SurfaceMeshState`] and are independent of the loaded molecule, so
/// surfaces stay put even while a trajectory animates the atoms.
pub struct SurfaceMeshRender {
    /// Half-length of each cross arm, in nanometers.
    radius: f32,
}

/// A single overlaid dot surface: positions (in the crate's nanometer units)
/// with the color it should be drawn in.
#[derive(Clone)]
pub struct SurfaceLayer {
    pub positions: Vec<Vec3>,
    pub color: (f32, f32, f32),
}

/// State type for the surface overlays stored in SharedRenderStates. Only the
/// layers that should currently be drawn are included.
#[derive(Clone, Default)]
pub struct SurfaceMeshState {
    pub layers: Vec<SurfaceLayer>,
}

impl SurfaceMeshRender {
    pub fn new() -> Self {
        Self { radius: 0.04 }
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

        let total: usize = state.layers.iter().map(|l| l.positions.len()).sum();
        if total == 0 {
            return;
        }

        // All crosses share one unit mesh; per-dot placement/scale/color lives on
        // the entities.
        let mesh_idx = scene.meshes.len();
        scene.meshes.push(Self::unit_cross_mesh());

        scene.entities.reserve(total);
        for layer in &state.layers {
            for &position in &layer.positions {
                scene.entities.push(Entity::new(
                    mesh_idx,
                    position,
                    Quaternion::new_identity(),
                    self.radius,
                    layer.color,
                    0.1,
                ));
            }
        }
    }
}
