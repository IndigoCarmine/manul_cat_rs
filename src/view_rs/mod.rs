use lin_alg::f32::Vec3;
use moleucle_3dview_rs::molecule::Atom;
use moleucle_3dview_rs::{Element, Molecule};

// Re-exported so the file parsers build atom metadata through a single path.
pub use moleucle_3dview_rs::molecule::AtomMeta;

pub trait To3dViewMolecule {
    fn to_molecule(&self) -> Molecule;
}

/// Build a viewer [`Atom`] from raw parts.
///
/// The viewer stores the element inline as an [`Element`] and keeps the optional
/// PDB-style attributes in a boxed [`AtomMeta`]. Centralizing the construction
/// here keeps that representation in one place so each file parser
/// (PDB/GRO/MOL2) does not have to repeat the `Element::new` / `Box` dance.
pub fn view_atom(position: Vec3, element: &str, id: usize, meta: Option<AtomMeta>) -> Atom {
    Atom {
        position,
        element: Element::new(element),
        id,
        meta: meta.map(Box::new),
    }
}
