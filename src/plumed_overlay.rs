//! The 3D overlay for the PLUMED viewer: highlight spheres on the atoms a
//! collective variable reads, markers on the virtual atoms it derives, and
//! arrows for the vectors it measures.
//!
//! This lives in the app rather than in `moleucle_3dview_rs` because the viewer
//! crate has no arrow primitive — `AtomPairRender` draws a plain cylinder and
//! only between *atom indices*, which a `CENTER` has none of. It also has no
//! spare state slot: `AtomGroupState` is already the NDX groups' and
//! `SphereSetState` the inactive layers', and sharing either would make PLUMED
//! and NDX highlights mutually exclusive. One small overlay of our own costs
//! less than that.

use lin_alg::f32::{Quaternion, Vec3};
use moleucle_3dview_rs::frame_state::RenderFrameState;
use moleucle_3dview_rs::scene_types::{Entity, Mesh, Scene, Vertex};
use moleucle_3dview_rs::{AdditionalRender, GpuPipeline, vdw_radius, with_state_by_type};

/// Radial segments in an arrowhead cone. Arrowheads are small on screen, so a
/// low count reads the same as a high one and keeps the mesh cheap.
const CONE_SIDES: usize = 12;

/// Shaft radius of a PLUMED vector, in nm.
pub const ARROW_RADIUS: f32 = 0.035;
/// Arrowhead radius, in nm.
pub const HEAD_RADIUS: f32 = 0.09;
/// Longest an arrowhead may be, in nm. Short vectors get a proportionally
/// shorter head so the cone never swallows the shaft.
pub const HEAD_MAX_LEN: f32 = 0.18;
/// Radius of a virtual-atom (`CENTER` / `COM`) marker, in nm.
pub const MARKER_RADIUS: f32 = 0.16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlumedArrow {
    pub start: Vec3,
    pub end: Vec3,
    pub radius: f32,
    pub color: (f32, f32, f32, f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlumedMarker {
    pub position: Vec3,
    pub radius: f32,
    pub color: (f32, f32, f32, f32),
}

/// Atoms highlighted in one colour. A line with `GROUPA`/`GROUPB` contributes
/// two of these so the two sides stay tellable apart.
#[derive(Debug, Clone, PartialEq)]
pub struct PlumedAtomGroup {
    pub atom_indices: Vec<usize>,
    pub color: (f32, f32, f32, f32),
}

/// Everything the PLUMED overlay draws, rebuilt by the app whenever the
/// selected line, the visibility toggles or the atom positions change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlumedOverlayState {
    pub atom_groups: Vec<PlumedAtomGroup>,
    pub markers: Vec<PlumedMarker>,
    pub arrows: Vec<PlumedArrow>,
    pub visible: bool,
}

impl PlumedOverlayState {
    pub fn is_empty(&self) -> bool {
        self.atom_groups.is_empty() && self.markers.is_empty() && self.arrows.is_empty()
    }
}

pub struct PlumedOverlayRender;

impl PlumedOverlayRender {
    pub fn new() -> Self {
        Self
    }

    /// Shaft plus a cone head, pointing `start` → `end`.
    ///
    /// The head is a cone rather than the ball tip [`AxesRender`] uses: these
    /// are vectors, and a reader has to be able to tell which end is the tip at
    /// a glance when several share an origin.
    fn add_arrow(&self, scene: &mut Scene, cone_mesh: usize, arrow: &PlumedArrow) {
        let direction = arrow.end - arrow.start;
        let length = direction.magnitude();
        if length <= f32::EPSILON {
            return;
        }
        let dir = direction / length;

        // Never let the head eat more than a quarter of the vector, so a short
        // one still reads as an arrow rather than a blob.
        let head_len = HEAD_MAX_LEN.min(length * 0.25);
        let head_radius = HEAD_RADIUS.min(head_len * 0.9);
        let shaft_end = arrow.end - dir * head_len;

        self.add_cylinder(scene, arrow.start, shaft_end, arrow.radius, arrow.color);

        let orientation = Quaternion::from_unit_vecs(Vec3::new(0.0, 1.0, 0.0), dir);
        let mut head = Entity::new(
            cone_mesh,
            // The unit cone is centred on its own axis, like the unit cylinder.
            arrow.end - dir * (head_len * 0.5),
            orientation,
            1.0,
            arrow.color,
            0.2,
        );
        head.scale_partial = Some(Vec3::new(head_radius, head_len, head_radius));
        scene.entities.push(head);
    }
}

impl Default for PlumedOverlayRender {
    fn default() -> Self {
        Self::new()
    }
}

impl AdditionalRender for PlumedOverlayRender {
    /// Without this every arrow, cone and highlight sphere is re-derived on the
    /// CPU on each repaint, including the ones that only moved the camera.
    fn revision(&self, frame: &RenderFrameState<'_>) -> Option<u64> {
        frame.overlay_revision::<PlumedOverlayState>()
    }

    fn gpu_pipeline(&self) -> GpuPipeline {
        // Triangles for the shafts and cones. The highlight spheres still come
        // out as impostors in `Circles` style — a batch uploads its triangles
        // and its impostors both — so nothing is lost by not asking for the
        // impostor pipeline here.
        GpuPipeline::Triangles
    }

    fn update_scene(&self, scene: &mut Scene, frame: &RenderFrameState<'_>) {
        let Some(states) = frame.shared_states else {
            return;
        };

        // Everything is drawn inside this one closure: the state map's lock is
        // held for its duration and is not reentrant, so a second
        // `with_state_by_type` in here would deadlock. The `add_*` helpers only
        // touch `scene`, so they are safe.
        with_state_by_type::<PlumedOverlayState, ()>(states, |state| {
            if !state.visible || state.is_empty() {
                return;
            }

            if let Some(molecule) = frame.molecule {
                for group in &state.atom_groups {
                    for &index in &group.atom_indices {
                        if !frame.is_atom_visible(index) {
                            continue;
                        }
                        let Some(atom) = molecule.atoms.get(index) else {
                            continue;
                        };
                        // Comfortably larger than the ball-and-stick atom so the
                        // highlight reads as a halo around it rather than a
                        // recolouring of it.
                        let radius = vdw_radius(atom.element.as_str()) * 0.55;
                        self.add_sphere(scene, frame, atom.position, radius, group.color);
                    }
                }
            }

            for marker in &state.markers {
                self.add_sphere(scene, frame, marker.position, marker.radius, marker.color);
            }

            if !state.arrows.is_empty() {
                // One cone mesh for every arrow in the scene, pushed once here
                // rather than per arrow.
                let cone_mesh = scene.meshes.len();
                scene.meshes.push(unit_cone_mesh(CONE_SIDES));
                for arrow in &state.arrows {
                    self.add_arrow(scene, cone_mesh, arrow);
                }
            }
        });
    }
}

/// A cone of height 1 and base radius 1, centred on the origin and pointing
/// along `+Y` — the same axis and centring convention as the viewer's unit
/// cylinder, so `scale_partial` stretches it the same way.
fn unit_cone_mesh(sides: usize) -> Mesh {
    let sides = sides.max(3);
    let mut vertices = Vec::with_capacity(sides * 3 + 1);
    let mut indices = Vec::with_capacity(sides * 6);
    let (apex_y, base_y) = (0.5, -0.5);

    // Side: one apex vertex per segment, each carrying that segment's normal,
    // so the cone shades as a cone instead of smearing toward the tip.
    for i in 0..sides {
        let next = (i + 1) % sides;
        let (x0, z0) = ring_point(i, sides);
        let (x1, z1) = ring_point(next, sides);

        // For a unit cone the outward normal at angle t is (cos t, 1, sin t)
        // normalised: base radius and height are both 1, so the slope is 45°.
        let n0 = Vec3::new(x0, 1.0, z0).to_normalized();
        let n1 = Vec3::new(x1, 1.0, z1).to_normalized();
        let apex_normal = ((n0 + n1) * 0.5).to_normalized();

        let base = vertices.len();
        vertices.push(Vertex::new([0.0, apex_y, 0.0], apex_normal));
        vertices.push(Vertex::new([x0, base_y, z0], n0));
        vertices.push(Vertex::new([x1, base_y, z1], n1));
        // Same winding as the unit cylinder's side quads with the top ring
        // collapsed onto the apex.
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    // Base cap, so the cone is not see-through from behind.
    let center = vertices.len();
    vertices.push(Vertex::new(
        [0.0, base_y, 0.0],
        Vec3::new(0.0, -1.0, 0.0),
    ));
    let ring_start = vertices.len();
    for i in 0..sides {
        let (x, z) = ring_point(i, sides);
        vertices.push(Vertex::new([x, base_y, z], Vec3::new(0.0, -1.0, 0.0)));
    }
    for i in 0..sides {
        let next = (i + 1) % sides;
        indices.extend_from_slice(&[center, ring_start + i, ring_start + next]);
    }

    Mesh { vertices, indices }
}

fn ring_point(i: usize, sides: usize) -> (f32, f32) {
    let angle = (i as f32 / sides as f32) * std::f32::consts::TAU;
    (angle.cos(), angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_cone_is_closed_and_unit_sized() {
        let mesh = unit_cone_mesh(CONE_SIDES);
        // One triangle per side plus one per base-cap segment.
        assert_eq!(mesh.indices.len(), CONE_SIDES * 3 * 2);
        assert!(mesh.indices.iter().all(|&i| i < mesh.vertices.len()));

        for vertex in &mesh.vertices {
            let [x, y, z] = vertex.position;
            assert!((-0.5..=0.5).contains(&y), "y out of range: {y}");
            assert!(x.hypot(z) <= 1.0 + 1e-5);
        }
    }

    #[test]
    fn a_degenerate_arrow_draws_nothing() {
        let mut scene = Scene::default();
        let cone = scene.meshes.len();
        scene.meshes.push(unit_cone_mesh(CONE_SIDES));
        let point = Vec3::new(1.0, 1.0, 1.0);

        PlumedOverlayRender::new().add_arrow(
            &mut scene,
            cone,
            &PlumedArrow {
                start: point,
                end: point,
                radius: ARROW_RADIUS,
                color: (1.0, 1.0, 1.0, 1.0),
            },
        );

        assert!(scene.entities.is_empty());
    }

    #[test]
    fn an_arrow_adds_a_shaft_and_a_head() {
        let mut scene = Scene::default();
        let cone = scene.meshes.len();
        scene.meshes.push(unit_cone_mesh(CONE_SIDES));

        PlumedOverlayRender::new().add_arrow(
            &mut scene,
            cone,
            &PlumedArrow {
                start: Vec3::new(0.0, 0.0, 0.0),
                end: Vec3::new(0.0, 2.0, 0.0),
                radius: ARROW_RADIUS,
                color: (1.0, 0.0, 0.0, 1.0),
            },
        );

        assert_eq!(scene.entities.len(), 2);
        // The head sits at the tip, set back by half its own length.
        let head = scene.entities.last().unwrap();
        assert_eq!(head.mesh, cone);
        assert!((head.position.y - (2.0 - HEAD_MAX_LEN * 0.5)).abs() < 1e-5);
    }
}
