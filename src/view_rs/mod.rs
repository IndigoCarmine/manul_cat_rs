use lin_alg::f32::Vec3;
use moleucle_3dview_rs::molecule::{Atom, Bond};
use moleucle_3dview_rs::{Element, Molecule};

// Re-exported so the file parsers build atom metadata through a single path.
pub use moleucle_3dview_rs::molecule::AtomMeta;

pub trait To3dViewMolecule {
    fn to_molecule(&self) -> Molecule;
}

/// Build a viewer [`Atom`] from raw parts.
///
/// moleucle_3dview_rs 0.6 stores the element inline as a [`Element`] and keeps
/// the optional PDB-style attributes in a boxed [`AtomMeta`]. Centralizing the
/// construction here keeps that representation in one place so each file parser
/// (PDB/GRO/MOL2) does not have to repeat the `Element::new` / `Box` dance.
pub fn view_atom(position: Vec3, element: &str, id: usize, meta: Option<AtomMeta>) -> Atom {
    Atom {
        position,
        element: Element::new(element),
        id,
        meta: meta.map(Box::new),
    }
}

/// Assemble a [`Molecule`] from already-parsed atoms and bonds.
///
/// The 0.6 `Molecule` carries a private `generation` counter, so it can no
/// longer be built with a struct literal from outside the crate; we start from
/// `Default` (generation 0) and fill the public fields.
pub fn molecule_from_parts(atoms: Vec<Atom>, bonds: Vec<Bond>) -> Molecule {
    let mut molecule = Molecule::default();
    molecule.atoms = atoms;
    molecule.bonds = bonds;
    molecule
}
