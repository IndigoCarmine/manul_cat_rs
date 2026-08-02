//! The COMPONENTS model: a named *partition* of the molecule's atoms.
//!
//! Before this existed, a component was literally a residue name and the whole
//! model was `BTreeMap<String, bool>`, so `SOL` could only ever mean "every
//! water in the system". Here a component is a name plus an explicit atom set,
//! which is what makes splitting and merging possible.
//!
//! Two properties do the heavy lifting:
//!
//! * **Every atom belongs to exactly one component.** [`ComponentState::owner`]
//!   is the authority and the atom lists are derived from it, so visibility is
//!   never ambiguous and no NDX-style overlap resolution
//!   (`KuromameApp::resolve_ndx_overlaps`) is needed.
//! * **Assignment moves atoms rather than defining a view.** `DOM1 = PROT and
//!   resid 1-100` takes those atoms *out* of PROT, and `PROT = PROA or PROB`
//!   empties both sources, which then disappear. Split and merge are the same
//!   operation seen from two directions.
//!
//! Atom indices here are *original* indices — the same space as
//! `KuromameApp::molecule` and `SelectionState::selected_atom_indices`, before
//! `VisibilityState::to_view` projects into the viewport.

use moleucle_3dview_rs::Molecule;

/// One display group.
#[derive(Clone, Debug)]
pub struct Component {
    pub name: String,
    /// Sorted, deduplicated original atom indices. Derived from
    /// [`ComponentState::owner`]; never written directly from outside.
    pub atoms: Vec<u32>,
    pub visible: bool,
    /// The expression that produced this component, kept for `list` and so a
    /// reload of the same system can re-derive it. `None` for the ones built
    /// automatically from residue names.
    pub source: Option<String>,
}

#[derive(Debug)]
pub enum ComponentError {
    NotFound {
        name: String,
    },
    NameTaken {
        name: String,
    },
    /// Shrinking a component whose name is also the residue name of the atoms
    /// it would drop. Those atoms fall back to the component named after their
    /// residue — which is the one rejecting them — so there is nowhere to put
    /// them and the assignment would silently not shrink.
    NoFallbackHome {
        name: String,
        atoms: usize,
    },
}

impl std::fmt::Display for ComponentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentError::NotFound { name } => {
                write!(f, "no component named '{name}' (run 'list' to see them)")
            }
            ComponentError::NameTaken { name } => {
                write!(f, "a component named '{name}' already exists")
            }
            ComponentError::NoFallbackHome { name, atoms } => write!(
                f,
                "'{name}' cannot be shrunk in place: the {atoms} atoms it would drop belong to \
                 residue '{name}', so they would fall straight back into it\n  \
                 hint: give the subset its own name instead, or 'rename {name} ...' first"
            ),
        }
    }
}

/// What an assignment did, for the status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignOutcome {
    /// Atoms the target holds afterwards.
    pub claimed: usize,
    /// Components that ended up empty and were removed — i.e. the ones that got
    /// merged away.
    pub absorbed: Vec<String>,
    /// Whether the target did not exist before.
    pub created: bool,
}

#[derive(Default)]
pub struct ComponentState {
    components: Vec<Component>,
    /// Original atom index -> index into `components`. Same length as the
    /// molecule's atom list whenever the state is live.
    owner: Vec<u32>,
}

impl ComponentState {
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// How many atoms the partition currently covers.
    pub fn atom_count(&self) -> usize {
        self.owner.len()
    }

    /// Whether this state still describes a molecule of `n` atoms. A trajectory
    /// swap or a reload of a different system invalidates every stored index.
    pub fn matches_atom_count(&self, n: usize) -> bool {
        self.owner.len() == n
    }

    /// True while the partition is still purely residue-derived — nothing has
    /// been split or merged by hand. Lets the app safely re-derive it (e.g.
    /// after the user edits residue names) without discarding anyone's work.
    pub fn is_default_partition(&self) -> bool {
        self.components.iter().all(|c| c.source.is_none())
    }

    /// Is the atom at this *original* index drawn? Unknown atoms default to
    /// visible, matching the old `res_visible.get(..).unwrap_or(true)`.
    pub fn is_atom_visible(&self, orig: usize) -> bool {
        match self.owner.get(orig) {
            Some(&slot) => self.components[slot as usize].visible,
            None => true,
        }
    }

    pub fn any_hidden(&self) -> bool {
        self.components.iter().any(|c| !c.visible)
    }

    /// Case-insensitive lookup, since users type `prot` for `PROT`.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.components
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Name/atoms pairs for the expression evaluator's `EvalCtx`.
    pub fn name_atom_pairs(&self) -> Vec<(String, Vec<u32>)> {
        self.components
            .iter()
            .map(|c| (c.name.clone(), c.atoms.clone()))
            .collect()
    }

    /// Build the default one-component-per-residue-name partition, in the same
    /// alphabetical order the panel has always shown.
    ///
    /// Show/hide choices survive by name, which is what made a reload keep the
    /// user's hidden `SOL` before this model existed.
    pub fn rebuild_from_molecule(&mut self, mol: &Molecule) {
        let previous: Vec<(String, bool)> = self
            .components
            .iter()
            .map(|c| (c.name.clone(), c.visible))
            .collect();
        let was_visible = |name: &str| {
            previous
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| *v)
        };

        let mut names: Vec<String> = mol.atoms.iter().map(residue_key).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();

        self.components = sorted
            .iter()
            .map(|name| Component {
                name: name.clone(),
                atoms: Vec::new(),
                visible: was_visible(name).unwrap_or(true),
                source: None,
            })
            .collect();

        self.owner = names
            .drain(..)
            .map(|name| sorted.binary_search(&name).unwrap_or(0) as u32)
            .collect();

        self.recompute();
    }

    pub fn set_visible(&mut self, name: &str, visible: bool) -> Result<bool, ComponentError> {
        let slot = self.find(name).ok_or_else(|| ComponentError::NotFound {
            name: name.to_string(),
        })?;
        let changed = self.components[slot].visible != visible;
        self.components[slot].visible = visible;
        Ok(changed)
    }

    pub fn set_all_visible(&mut self, visible: bool) -> bool {
        let mut changed = false;
        for c in &mut self.components {
            if c.visible != visible {
                c.visible = visible;
                changed = true;
            }
        }
        changed
    }

    /// Show one component and hide every other.
    pub fn only(&mut self, name: &str) -> Result<(), ComponentError> {
        let slot = self.find(name).ok_or_else(|| ComponentError::NotFound {
            name: name.to_string(),
        })?;
        for (i, c) in self.components.iter_mut().enumerate() {
            c.visible = i == slot;
        }
        Ok(())
    }

    /// `NAME = expr`: make `name` hold exactly `atoms`.
    ///
    /// Atoms move in from wherever they were; atoms the target held but that are
    /// no longer matched fall back to their residue-name component, so the
    /// operation is idempotent — running the same assignment twice is a no-op
    /// rather than a slow drift.
    pub fn assign(
        &mut self,
        name: &str,
        atoms: &[u32],
        source: Option<String>,
        mol: &Molecule,
    ) -> Result<AssignOutcome, ComponentError> {
        let before: Vec<String> = self.components.iter().map(|c| c.name.clone()).collect();
        let existing = self.find(name);

        // What the target would stop holding. Computed before anything moves so
        // an impossible eviction can be refused without half-applying.
        let keep: std::collections::HashSet<u32> = atoms.iter().copied().collect();
        let stale: Vec<u32> = match existing {
            Some(slot) => self.components[slot]
                .atoms
                .iter()
                .copied()
                .filter(|a| !keep.contains(a))
                .collect(),
            None => Vec::new(),
        };
        let homeless = stale
            .iter()
            .filter(|&&a| {
                mol.atoms
                    .get(a as usize)
                    .map(residue_key)
                    .is_some_and(|key| key.eq_ignore_ascii_case(name.trim()))
            })
            .count();
        if homeless > 0 {
            return Err(ComponentError::NoFallbackHome {
                name: name.trim().to_string(),
                atoms: homeless,
            });
        }

        let created = existing.is_none();
        let target = match existing {
            Some(slot) => slot,
            None => {
                self.components.push(Component {
                    name: name.trim().to_string(),
                    atoms: Vec::new(),
                    visible: true,
                    source: source.clone(),
                });
                self.components.len() - 1
            }
        };
        if !created {
            self.components[target].source = source;
        }

        for atom in stale {
            let home = self.residue_home(atom, mol);
            if let Some(slot) = self.owner.get_mut(atom as usize) {
                *slot = home;
            }
        }

        // Claim the matched atoms. `target` is still valid: `residue_home` only
        // ever appends, and appending cannot move an earlier index.
        for &atom in atoms {
            if let Some(slot) = self.owner.get_mut(atom as usize) {
                *slot = target as u32;
            }
        }

        self.recompute();

        let after: std::collections::HashSet<&str> =
            self.components.iter().map(|c| c.name.as_str()).collect();
        let absorbed = before
            .into_iter()
            .filter(|n| !after.contains(n.as_str()))
            .collect();

        Ok(AssignOutcome {
            claimed: self
                .find(name)
                .map(|s| self.components[s].atoms.len())
                .unwrap_or(0),
            absorbed,
            created,
        })
    }

    /// `del NAME`: dissolve the component, returning its atoms to the components
    /// named after their residues. Returns how many atoms moved.
    pub fn dissolve(&mut self, name: &str, mol: &Molecule) -> Result<usize, ComponentError> {
        let slot = self.find(name).ok_or_else(|| ComponentError::NotFound {
            name: name.to_string(),
        })?;
        let atoms = self.components[slot].atoms.clone();
        for &atom in &atoms {
            let home = self.residue_home(atom, mol);
            if home == slot as u32 {
                continue; // already named after its own residue; nothing to do
            }
            if let Some(owner) = self.owner.get_mut(atom as usize) {
                *owner = home;
            }
        }
        self.recompute();
        Ok(atoms.len())
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), ComponentError> {
        let slot = self.find(from).ok_or_else(|| ComponentError::NotFound {
            name: from.to_string(),
        })?;
        if let Some(clash) = self.find(to)
            && clash != slot {
                return Err(ComponentError::NameTaken {
                    name: to.to_string(),
                });
            }
        self.components[slot].name = to.trim().to_string();
        Ok(())
    }

    /// The slot an atom falls back to when it is evicted: the component named
    /// after its residue, created (hidden-preserving default: visible) if the
    /// last atom of that residue had previously been carved away.
    fn residue_home(&mut self, atom: u32, mol: &Molecule) -> u32 {
        let key = mol
            .atoms
            .get(atom as usize)
            .map(residue_key)
            .unwrap_or_default();
        if let Some(slot) = self.find(&key) {
            return slot as u32;
        }
        self.components.push(Component {
            name: key,
            atoms: Vec::new(),
            visible: true,
            source: None,
        });
        (self.components.len() - 1) as u32
    }

    /// Re-derive every atom list from `owner` and drop the components that came
    /// out empty, remapping `owner` onto the compacted slots.
    ///
    /// Walking `owner` in atom order means each list comes out sorted and
    /// duplicate-free for free, which is the invariant the rest of the app
    /// (and `NdxSelectionState`) relies on.
    fn recompute(&mut self) {
        for c in &mut self.components {
            c.atoms.clear();
        }
        for (atom, &slot) in self.owner.iter().enumerate() {
            if let Some(c) = self.components.get_mut(slot as usize) {
                c.atoms.push(atom as u32);
            }
        }

        if self.components.iter().all(|c| !c.atoms.is_empty()) {
            return;
        }

        let mut remap = vec![0u32; self.components.len()];
        let mut next = 0u32;
        for (i, c) in self.components.iter().enumerate() {
            if !c.atoms.is_empty() {
                remap[i] = next;
                next += 1;
            }
        }
        self.components.retain(|c| !c.atoms.is_empty());
        for slot in &mut self.owner {
            *slot = remap[*slot as usize];
        }
    }

    /// Debug assertion helper, also exercised by the tests: does every atom
    /// belong to exactly one component, and do the lists agree with `owner`?
    #[cfg(test)]
    fn assert_partition(&self) {
        let mut seen = vec![false; self.owner.len()];
        for (slot, c) in self.components.iter().enumerate() {
            assert!(!c.atoms.is_empty(), "component '{}' is empty", c.name);
            let mut sorted = c.atoms.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted, c.atoms, "'{}' is not sorted/deduped", c.name);
            for &a in &c.atoms {
                assert!(!seen[a as usize], "atom {a} is in two components");
                seen[a as usize] = true;
                assert_eq!(self.owner[a as usize], slot as u32, "owner disagrees");
            }
        }
        assert!(seen.iter().all(|&s| s), "some atom belongs to no component");
    }
}

/// The name a residue contributes to the default partition: trimmed, kept in
/// the file's own case. An atom with no residue metadata lands in `""`, which
/// the panel renders as "(no residue)" exactly as it did before.
fn residue_key(atom: &moleucle_3dview_rs::Atom) -> String {
    atom.res_name().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_rs::{view_atom};
    use lin_alg::f32::Vec3;
    use moleucle_3dview_rs::molecule::AtomMeta;

    /// 3 PROT atoms (res_seq 1..3), 2 SOL, 1 NA — six atoms, three residues.
    fn system() -> Molecule {
        let spec = [
            ("PROT", 1),
            ("PROT", 2),
            ("PROT", 3),
            ("SOL", 4),
            ("SOL", 5),
            ("NA", 6),
        ];
        let atoms = spec
            .iter()
            .enumerate()
            .map(|(i, (res, seq))| {
                view_atom(
                    Vec3::new(i as f32, 0.0, 0.0),
                    "C",
                    i,
                    Some(AtomMeta {
                        name: Some(format!("A{i}")),
                        res_name: Some(res.to_string()),
                        chain_id: Some('A'),
                        res_seq: Some(*seq),
                        occupancy: None,
                        temp_factor: None,
                        charge: None,
                    }),
                )
            })
            .collect();
        Molecule::from_atoms_bonds(atoms, Vec::new())
    }

    fn state() -> (ComponentState, Molecule) {
        let mol = system();
        let mut st = ComponentState::default();
        st.rebuild_from_molecule(&mol);
        (st, mol)
    }

    fn names(st: &ComponentState) -> Vec<&str> {
        st.components.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn default_partition_is_one_component_per_residue_alphabetically() {
        let (st, _) = state();
        assert_eq!(names(&st), vec!["NA", "PROT", "SOL"]);
        assert_eq!(st.components[1].atoms, vec![0, 1, 2]);
        assert_eq!(st.components[2].atoms, vec![3, 4]);
        st.assert_partition();
    }

    #[test]
    fn split_carves_atoms_out_of_the_source() {
        let (mut st, mol) = state();
        let out = st
            .assign("DOM1", &[0, 1], Some("PROT and resid 1-2".into()), &mol)
            .unwrap();
        assert!(out.created);
        assert_eq!(out.claimed, 2);
        assert!(out.absorbed.is_empty());

        assert_eq!(st.components[st.find("PROT").unwrap()].atoms, vec![2]);
        assert_eq!(st.components[st.find("DOM1").unwrap()].atoms, vec![0, 1]);
        st.assert_partition();
    }

    #[test]
    fn merge_empties_the_sources_and_removes_them() {
        let (mut st, mol) = state();
        // Everything that was PROT or SOL becomes one component.
        let out = st.assign("BIG", &[0, 1, 2, 3, 4], None, &mol).unwrap();
        assert_eq!(out.claimed, 5);
        let mut absorbed = out.absorbed.clone();
        absorbed.sort();
        assert_eq!(absorbed, vec!["PROT", "SOL"]);
        assert_eq!(names(&st), vec!["NA", "BIG"]);
        st.assert_partition();
    }

    #[test]
    fn assignment_is_idempotent() {
        let (mut st, mol) = state();
        st.assign("DOM1", &[0, 1], None, &mol).unwrap();
        let snapshot: Vec<(String, Vec<u32>)> = st.name_atom_pairs();
        st.assign("DOM1", &[0, 1], None, &mol).unwrap();
        assert_eq!(st.name_atom_pairs(), snapshot);
        st.assert_partition();
    }

    #[test]
    fn reassigning_a_smaller_set_evicts_the_rest_to_their_residue() {
        let (mut st, mol) = state();
        st.assign("DOM1", &[0, 1, 2], None, &mol).unwrap();
        assert!(st.find("PROT").is_none(), "PROT was emptied and dropped");

        st.assign("DOM1", &[0], None, &mol).unwrap();
        assert_eq!(st.components[st.find("DOM1").unwrap()].atoms, vec![0]);
        assert_eq!(
            st.components[st.find("PROT").unwrap()].atoms,
            vec![1, 2],
            "the evicted atoms came back under their residue name"
        );
        st.assert_partition();
    }

    #[test]
    fn dissolve_returns_atoms_to_their_residue_components() {
        let (mut st, mol) = state();
        st.assign("MIX", &[0, 3], None, &mol).unwrap(); // one PROT atom and one SOL atom
        let moved = st.dissolve("MIX", &mol).unwrap();
        assert_eq!(moved, 2);
        assert!(st.find("MIX").is_none());
        assert_eq!(st.components[st.find("PROT").unwrap()].atoms, vec![0, 1, 2]);
        assert_eq!(st.components[st.find("SOL").unwrap()].atoms, vec![3, 4]);
        st.assert_partition();
    }

    #[test]
    fn visibility_survives_a_rebuild_by_name() {
        let (mut st, mol) = state();
        st.set_visible("SOL", false).unwrap();
        st.rebuild_from_molecule(&mol);
        assert!(!st.components[st.find("SOL").unwrap()].visible);
        assert!(st.components[st.find("PROT").unwrap()].visible);
    }

    #[test]
    fn only_shows_one_and_hides_the_rest() {
        let (mut st, _) = state();
        st.only("PROT").unwrap();
        assert!(st.components[st.find("PROT").unwrap()].visible);
        assert!(!st.components[st.find("SOL").unwrap()].visible);
        assert!(!st.components[st.find("NA").unwrap()].visible);
    }

    #[test]
    fn is_atom_visible_follows_the_owning_component() {
        let (mut st, _) = state();
        st.set_visible("SOL", false).unwrap();
        assert!(st.is_atom_visible(0)); // PROT
        assert!(!st.is_atom_visible(3)); // SOL
        assert!(st.is_atom_visible(99), "unknown atoms default to visible");
    }

    #[test]
    fn rename_rejects_a_collision_but_allows_a_no_op() {
        let (mut st, _) = state();
        assert!(matches!(
            st.rename("PROT", "SOL"),
            Err(ComponentError::NameTaken { .. })
        ));
        st.rename("PROT", "prot").unwrap(); // same slot, just a case change
        assert!(st.find("PROT").is_some());
        assert!(matches!(
            st.rename("NOPE", "X"),
            Err(ComponentError::NotFound { .. })
        ));
    }

    #[test]
    fn lookups_are_case_insensitive() {
        let (st, _) = state();
        assert_eq!(st.find("prot"), st.find("PROT"));
        assert_eq!(st.find(" PrOt "), st.find("PROT"));
    }

    #[test]
    fn atom_count_tracks_the_molecule() {
        let (st, mol) = state();
        assert!(st.matches_atom_count(mol.atoms.len()));
        assert!(!st.matches_atom_count(mol.atoms.len() + 1));
    }

    #[test]
    fn default_partition_starts_out_underived() {
        let (mut st, mol) = state();
        assert!(st.is_default_partition());
        st.assign("DOM1", &[0], Some("index 1".into()), &mol).unwrap();
        assert!(!st.is_default_partition());
    }

    #[test]
    fn a_residue_component_cannot_be_shrunk_into_itself() {
        // `SOL = <one of the two waters>` would drop the other water, whose
        // fallback home is the component named after its residue — SOL, the one
        // rejecting it. Refusing beats silently not shrinking.
        let (mut st, mol) = state();
        let err = st.assign("SOL", &[3], None, &mol).unwrap_err();
        match err {
            ComponentError::NoFallbackHome { name, atoms } => {
                assert_eq!(name, "SOL");
                assert_eq!(atoms, 1);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            st.components[st.find("SOL").unwrap()].atoms,
            vec![3, 4],
            "the refused assignment left SOL untouched"
        );
        st.assert_partition();

        // The same subset under its own name is fine.
        st.assign("WAT1", &[3], None, &mol).unwrap();
        assert_eq!(st.components[st.find("SOL").unwrap()].atoms, vec![4]);
        st.assert_partition();
    }
}

/// End-to-end tests over the exact path `KuromameApp::run_command` takes:
/// parse the line, evaluate it against the molecule, then apply it to the
/// partition. The app itself needs a GPU viewport and cannot be built here, but
/// every step between the typed text and the regrouped components can.
#[cfg(test)]
mod command_flow {
    use super::*;
    use crate::selection::{AtomTable, EvalCtx, Statement, evaluate, parse_statement, to_indices};
    use crate::view_rs::{view_atom};
    use lin_alg::f32::Vec3;
    use moleucle_3dview_rs::molecule::{AtomMeta, Bond};

    /// Two propane molecules in residue `PRP` (res_seq 1 and 2) plus three
    /// waters in `SOL` (res_seq 3..5). 22 propane atoms + 9 water atoms.
    fn system() -> Molecule {
        let mut atoms = Vec::new();
        let mut bonds = Vec::new();
        let meta = |name: &str, res: &str, seq: i32| {
            Some(AtomMeta {
                name: Some(name.to_string()),
                res_name: Some(res.to_string()),
                chain_id: Some('A'),
                res_seq: Some(seq),
                occupancy: None,
                temp_factor: None,
                charge: None,
            })
        };
        let push = |atoms: &mut Vec<_>, el: &str, name: &str, res: &str, seq: i32| {
            let id = atoms.len();
            atoms.push(view_atom(
                Vec3::new(id as f32, 0.0, 0.0),
                el,
                id,
                meta(name, res, seq),
            ));
            id
        };
        let mut bond = |a: usize, b: usize| {
            bonds.push(Bond {
                atom_a: a,
                atom_b: b,
                order: 1,
            })
        };

        for copy in 0..2 {
            let seq = copy + 1;
            let c: Vec<usize> = (0..3)
                .map(|i| push(&mut atoms, "C", &format!("C{}", i + 1), "PRP", seq))
                .collect();
            bond(c[0], c[1]);
            bond(c[1], c[2]);
            // 3 H on each end carbon, 2 on the middle one.
            for (ci, count) in [(0usize, 3), (1, 2), (2, 3)] {
                for h in 0..count {
                    let hid = push(&mut atoms, "H", &format!("H{h}"), "PRP", seq);
                    bond(c[ci], hid);
                }
            }
        }
        for w in 0..3 {
            let seq = 3 + w;
            let o = push(&mut atoms, "O", "OW", "SOL", seq);
            for h in 0..2 {
                let hid = push(&mut atoms, "H", &format!("HW{h}"), "SOL", seq);
                bond(o, hid);
            }
        }
        Molecule::from_atoms_bonds(atoms, bonds)
    }

    /// Everything `run_command` does for one line, minus the logging.
    fn run(st: &mut ComponentState, mol: &Molecule, line: &str) -> Result<String, String> {
        let table = AtomTable::from_molecule(mol);
        let stmt = parse_statement(line).map_err(|e| e.to_string())?;
        let pairs = st.name_atom_pairs();
        let selected: Vec<usize> = Vec::new();
        let eval = |expr: &crate::selection::Expr| -> Result<Vec<u32>, String> {
            let ctx = EvalCtx {
                table: &table,
                components: &pairs,
                selected: &selected,
            };
            let mut notes = Vec::new();
            evaluate(expr, &ctx, &mut notes)
                .map(|m| to_indices(&m))
                .map_err(|e| e.to_string())
        };

        match stmt {
            Statement::Assign { name, expr } => {
                let atoms = eval(&expr)?;
                if atoms.is_empty() {
                    return Err("matched 0 atoms".into());
                }
                let out = st
                    .assign(&name, &atoms, Some(line.to_string()), mol)
                    .map_err(|e| e.to_string())?;
                Ok(format!("{name}={}", out.claimed))
            }
            Statement::Count(expr) => Ok(format!("{}", eval(&expr)?.len())),
            Statement::Del(names) => {
                for n in &names {
                    st.dissolve(n, mol).map_err(|e| e.to_string())?;
                }
                Ok("ok".into())
            }
            Statement::Only(name) => st
                .only(&name)
                .map(|_| "ok".to_string())
                .map_err(|e| e.to_string()),
            Statement::Rename { from, to } => st
                .rename(&from, &to)
                .map(|_| "ok".to_string())
                .map_err(|e| e.to_string()),
            Statement::Reset => {
                st.rebuild_from_molecule(mol);
                Ok("ok".into())
            }
            other => Err(format!("unhandled in test harness: {other:?}")),
        }
    }

    fn fresh() -> (ComponentState, Molecule) {
        let mol = system();
        let mut st = ComponentState::default();
        st.rebuild_from_molecule(&mol);
        (st, mol)
    }

    fn atoms_of(st: &ComponentState, name: &str) -> usize {
        st.find(name)
            .map(|i| st.components()[i].atoms.len())
            .unwrap_or(0)
    }

    fn names(st: &ComponentState) -> Vec<&str> {
        st.components().iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn the_readme_split_and_merge_round_trips() {
        let (mut st, mol) = fresh();
        assert_eq!(atoms_of(&st, "PRP"), 22);
        assert_eq!(atoms_of(&st, "SOL"), 9);

        // Split the first propane out by residue number.
        run(&mut st, &mol, "MOL1 = PRP and resid 1").unwrap();
        assert_eq!(atoms_of(&st, "MOL1"), 11);
        assert_eq!(atoms_of(&st, "PRP"), 11);
        st.assert_partition();

        // Merge it back. PRP is absorbed into the new MOL1, so only MOL1 remains.
        run(&mut st, &mol, "MOL1 = MOL1 or PRP").unwrap();
        assert_eq!(atoms_of(&st, "MOL1"), 22);
        assert!(st.find("PRP").is_none());
        st.assert_partition();

        // ...and `reset` puts the residue partition back.
        run(&mut st, &mol, "reset").unwrap();
        assert_eq!(atoms_of(&st, "PRP"), 22);
        assert!(st.find("MOL1").is_none());
        st.assert_partition();
    }

    #[test]
    fn the_methylene_command_selects_both_middle_carbons() {
        let (mut st, mol) = fresh();
        run(&mut st, &mol, "CH2 = element C and sp3 and with 2 H").unwrap();
        assert_eq!(atoms_of(&st, "CH2"), 2, "one middle carbon per propane");
        assert_eq!(atoms_of(&st, "PRP"), 20);
        st.assert_partition();

        run(&mut st, &mol, "CH3 = element C and sp3 and with 3 H").unwrap();
        assert_eq!(atoms_of(&st, "CH3"), 4);
        st.assert_partition();
    }

    #[test]
    fn a_carved_component_can_be_carved_again() {
        let (mut st, mol) = fresh();
        run(&mut st, &mol, "MOL1 = PRP and resid 1").unwrap();
        run(&mut st, &mol, "HEAD = MOL1 and element C").unwrap();
        assert_eq!(atoms_of(&st, "HEAD"), 3);
        assert_eq!(atoms_of(&st, "MOL1"), 8, "the hydrogens stayed behind");
        st.assert_partition();
    }

    #[test]
    fn del_returns_atoms_to_their_residues_across_components() {
        let (mut st, mol) = fresh();
        run(&mut st, &mol, "MIX = resid 1 3").unwrap(); // one propane + one water
        assert_eq!(atoms_of(&st, "MIX"), 14);
        run(&mut st, &mol, "del MIX").unwrap();
        assert_eq!(atoms_of(&st, "PRP"), 22);
        assert_eq!(atoms_of(&st, "SOL"), 9);
        st.assert_partition();
    }

    #[test]
    fn only_leaves_exactly_one_component_visible() {
        let (mut st, mol) = fresh();
        run(&mut st, &mol, "only SOL").unwrap();
        assert!(st.components()[st.find("SOL").unwrap()].visible);
        assert!(!st.components()[st.find("PRP").unwrap()].visible);
        // Every propane atom is now filtered out of the viewport.
        assert!(!st.is_atom_visible(0));
        assert!(st.is_atom_visible(22));
    }

    #[test]
    fn a_selection_matching_nothing_creates_no_component() {
        let (mut st, mol) = fresh();
        assert!(run(&mut st, &mol, "GHOST = resname NOPE").is_err());
        assert!(st.find("GHOST").is_none());
        assert_eq!(st.len(), 2, "PRP and SOL, untouched");
        st.assert_partition();
    }

    #[test]
    fn bare_expressions_only_count() {
        let (mut st, mol) = fresh();
        assert_eq!(run(&mut st, &mol, "element C").unwrap(), "6");
        assert_eq!(run(&mut st, &mol, "resname SOL").unwrap(), "9");
        assert_eq!(st.len(), 2, "counting changed nothing");
    }

    #[test]
    fn bare_word_prefers_a_component_over_a_residue_of_the_same_name() {
        let (mut st, mol) = fresh();
        run(&mut st, &mol, "WAT1 = resname SOL and resid 3").unwrap();
        // Give the carved-out water the name of a residue that still exists.
        run(&mut st, &mol, "rename WAT1 PRP").ok();
        st.rename("WAT1", "PRP").unwrap_err(); // PRP is taken; the rename above no-oped
        // `SOL` still resolves to the (now smaller) SOL component, not the residue.
        assert_eq!(run(&mut st, &mol, "SOL").unwrap(), "6");
        assert_eq!(run(&mut st, &mol, "resname SOL").unwrap(), "9");
    }

    #[test]
    fn shrinking_a_residue_component_in_place_is_refused_with_a_hint() {
        let (mut st, mol) = fresh();
        let err = run(&mut st, &mol, "SOL = resname SOL and resid 3").unwrap_err();
        assert!(err.contains("cannot be shrunk in place"), "{err}");
        assert!(err.contains("its own name"), "{err}");
        assert_eq!(atoms_of(&st, "SOL"), 9, "nothing moved");
        st.assert_partition();
    }

    /// The synthetic molecules above hand-build `AtomMeta`. This one goes
    /// through the real PDB reader, so it catches any mismatch between what the
    /// parser writes into `res_name` (fixed columns, padded) and what
    /// `residue_key` / `AtomTable` expect.
    #[test]
    fn a_real_parsed_pdb_partitions_and_splits() {
        use crate::parsing::PdbFile;
        use crate::view_rs::To3dViewMolecule;

        // Two ALA residues and two HOH. CONECT gives CA its three neighbours,
        // so the connectivity predicates have something to chew on. The records
        // are reciprocal, as in a real file — `to_molecule` only emits a bond
        // from the record whose own serial is the lower index.
        let pdb = "\
ATOM      1  N   ALA A   1      11.104   6.134  -6.504  1.00  0.00           N
ATOM      2  CA  ALA A   1      11.639   6.071  -5.147  1.00  0.00           C
ATOM      3  C   ALA A   1      13.140   6.199  -5.153  1.00  0.00           C
ATOM      4  CB  ALA A   1      11.041   7.184  -4.313  1.00  0.00           C
ATOM      5  N   ALA A   2      13.771   6.312  -4.001  1.00  0.00           N
ATOM      6  CA  ALA A   2      15.229   6.427  -3.912  1.00  0.00           C
ATOM      7  O   HOH A   3      20.000  10.000   0.000  1.00  0.00           O
ATOM      8  O   HOH A   4      21.000  11.000   0.000  1.00  0.00           O
CONECT    1    2
CONECT    2    1    3    4
CONECT    3    2
CONECT    4    2
END
";
        let mol = PdbFile::load(pdb).to_molecule();
        assert_eq!(mol.atoms.len(), 8);

        let mut st = ComponentState::default();
        st.rebuild_from_molecule(&mol);
        assert_eq!(names(&st), vec!["ALA", "HOH"], "one component per residue");
        assert_eq!(atoms_of(&st, "ALA"), 6);
        assert_eq!(atoms_of(&st, "HOH"), 2);
        st.assert_partition();

        // Residue numbers and atom names survive the round trip.
        assert_eq!(run(&mut st, &mol, "resid 1").unwrap(), "4");
        assert_eq!(run(&mut st, &mol, "name CA").unwrap(), "2");
        assert_eq!(run(&mut st, &mol, "element O").unwrap(), "2");

        // ...and so does the CONECT-derived bond graph.
        assert_eq!(run(&mut st, &mol, "numbonds 3").unwrap(), "1");

        run(&mut st, &mol, "RES1 = ALA and resid 1").unwrap();
        assert_eq!(atoms_of(&st, "RES1"), 4);
        assert_eq!(atoms_of(&st, "ALA"), 2);
        st.assert_partition();
    }

    #[test]
    fn hidden_components_survive_a_split() {
        let (mut st, mol) = fresh();
        st.set_visible("SOL", false).unwrap();
        run(&mut st, &mol, "MOL1 = PRP and resid 1").unwrap();
        assert!(
            !st.components()[st.find("SOL").unwrap()].visible,
            "an unrelated split must not disturb other components' visibility"
        );
        assert!(st.components()[st.find("MOL1").unwrap()].visible);
    }
}
