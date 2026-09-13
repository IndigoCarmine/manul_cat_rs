use lin_alg::f32::Vec3;
use moleucle_3dview_rs::molecule::Atom;
use moleucle_3dview_rs::{Element, Molecule, MoleculeBuilder};

// Re-exported so the file parsers build atom metadata through a single path.
pub use moleucle_3dview_rs::molecule::AtomMeta;

pub trait To3dViewMolecule {
    fn to_molecule(&self) -> Molecule;
}

/// Build a viewer [`Atom`] that carries only a position and an element.
///
/// Names cannot be attached here: the viewer interns them into the owning
/// molecule's symbol table, so an atom with a name has to be pushed through a
/// [`MoleculeBuilder`] (see [`push_named_atom`]). This is the path for the
/// sources that have no names to keep — MOL2, and the placeholder atoms a
/// trajectory without a structure file is drawn with.
pub fn view_atom(position: Vec3, element: &str) -> Atom {
    Atom::new(position, Element::new(element))
}

/// Push one named atom onto `builder`, interning its strings.
///
/// The one place the app turns parsed per-atom fields into a viewer atom, so
/// each file parser (PDB/GRO) does not repeat the `AtomMeta` dance.
pub fn push_named_atom(
    builder: &mut MoleculeBuilder,
    position: Vec3,
    element: &str,
    meta: &AtomMeta<'_>,
) -> usize {
    builder.push(position, element, meta)
}

/// Per-atom Martini bead types, interned.
///
/// A coarse-grained system draws its bead types from a few dozen names ("P4",
/// "Qa", "C1") repeated across every atom, so the old `Vec<String>` spent ~27
/// bytes and one heap allocation per atom — 5 MB on a 200k-atom system — to say
/// so. Names are stored once here and referenced by id.
#[derive(Default)]
pub struct BeadTypes {
    names: moleucle_3dview_rs::SymbolTable,
    per_atom: Vec<moleucle_3dview_rs::SymbolId>,
}

impl BeadTypes {
    /// Bead type of one atom, or `None` past the end.
    pub fn get(&self, atom: usize) -> Option<&str> {
        self.names.resolve(*self.per_atom.get(atom)?)
    }

    pub fn len(&self) -> usize {
        self.per_atom.len()
    }

    pub fn is_empty(&self) -> bool {
        self.per_atom.is_empty()
    }

    pub fn clear(&mut self) {
        self.names = moleucle_3dview_rs::SymbolTable::default();
        self.per_atom.clear();
    }

    /// Append one atom's bead type.
    pub fn push(&mut self, bead_type: &str) {
        let id = self.names.intern(bead_type);
        self.per_atom.push(id);
    }
}
