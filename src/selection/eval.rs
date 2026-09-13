//! Evaluation of a parsed [`Expr`] against a molecule.
//!
//! Everything here works in *original* atom-index space — the same space as
//! `KuromameApp::molecule` and `SelectionState::selected_atom_indices`, before
//! `VisibilityState::to_view` projects into the viewport. Sets are carried as
//! `Vec<bool>` of length `n` so `and`/`or`/`not` are plain elementwise loops.

use moleucle_3dview_rs::Molecule;

use super::ast::{CountSpec, Expr, Hybrid, NumRange};

/// Per-atom lookup tables, built once per molecule.
///
/// This exists because the pre-existing `KuromameApp::atom_name_at` reaches for
/// `gro.atoms().nth(i)` / `pdb.atoms().nth(i)`, which is O(n) per atom and makes
/// any whole-molecule scan O(n²). Every field below is a direct index instead.
///
/// The three name columns are interned: a `Vec<String>` per column cost ~92
/// bytes and three allocations per atom (17.5 MB on a 200k-atom system) to hold
/// a few dozen distinct strings. They are now `u32` ids into `names`, which
/// also makes every predicate an integer compare rather than a string compare.
pub struct AtomTable {
    pub n: usize,
    /// Every distinct uppercased, trimmed string the three columns below refer
    /// to, stored once. Shared across the columns, so the same id means the
    /// same text whichever column it came from.
    names: Vec<Box<str>>,
    /// Residue name of each atom, as an index into `names`. Atoms with no
    /// residue name point at the interned empty string.
    res_name: Vec<u32>,
    /// Atom name of each atom, falling back to the element symbol the way
    /// `atom_name_at` does.
    name: Vec<u32>,
    /// Element symbol of each atom.
    element: Vec<u32>,
    /// Residue sequence number, or [`NO_RES_SEQ`] where the atom has none.
    res_seq: Vec<i32>,
    /// The ids each column actually uses, sorted. Lets `has_res_name` and its
    /// siblings answer from a handful of entries instead of scanning every atom.
    res_name_used: Vec<u32>,
    name_used: Vec<u32>,
    element_used: Vec<u32>,
    /// Neighbour lists in CSR form: atom `i`'s neighbours are
    /// `adj[adj_start[i]..adj_start[i + 1]]`.
    adj_start: Vec<u32>,
    adj: Vec<u32>,
    /// Whether the molecule carried any bonds at all. A bare `.gro` has none,
    /// and connectivity predicates must say so rather than quietly match zero.
    pub has_bonds: bool,
}

/// Stored in [`AtomTable::res_seq`] for an atom whose source gave no residue
/// number. No coordinate format can write `i32::MIN` in a residue field.
const NO_RES_SEQ: i32 = i32::MIN;

/// Interns uppercased, trimmed strings while the table is being built.
#[derive(Default)]
struct Interner {
    names: Vec<Box<str>>,
    index: std::collections::HashMap<Box<str>, u32>,
}

impl Interner {
    fn intern(&mut self, text: &str) -> u32 {
        if let Some(&id) = self.index.get(text) {
            return id;
        }
        let id = self.names.len() as u32;
        let boxed: Box<str> = text.into();
        self.names.push(boxed.clone());
        self.index.insert(boxed, id);
        id
    }
}

/// The distinct ids in `column`, sorted, for the `has_*` checks.
fn used_ids(column: &[u32]) -> Vec<u32> {
    let mut ids = column.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
}

impl AtomTable {
    pub fn from_molecule(mol: &Molecule) -> Self {
        let n = mol.atoms.len();
        let mut interner = Interner::default();
        let mut res_name = Vec::with_capacity(n);
        let mut res_seq = Vec::with_capacity(n);
        let mut name = Vec::with_capacity(n);
        let mut element = Vec::with_capacity(n);

        // Scratch reused across atoms so uppercasing does not allocate a fresh
        // `String` per atom on the way into the interner.
        let mut scratch = String::new();
        let mut intern_upper = |interner: &mut Interner, text: &str| -> u32 {
            scratch.clear();
            scratch.extend(text.trim().chars().map(|c| c.to_ascii_uppercase()));
            interner.intern(&scratch)
        };

        for atom in &mol.atoms {
            let elem = intern_upper(&mut interner, atom.element.as_str());
            res_name.push(intern_upper(
                &mut interner,
                mol.res_name_of(atom).unwrap_or(""),
            ));
            res_seq.push(atom.res_seq().unwrap_or(NO_RES_SEQ));
            let atom_name = mol.name_of(atom).map(str::trim).unwrap_or("");
            name.push(if atom_name.is_empty() {
                elem
            } else {
                intern_upper(&mut interner, atom_name)
            });
            element.push(elem);
        }

        // CSR build: count degrees, prefix-sum, then scatter. Bond endpoints are
        // range-checked because a topology can describe more atoms than the
        // coordinate file (the same mismatch `rebuild_viewport` guards against).
        let mut degree = vec![0u32; n];
        let mut usable = 0usize;
        for bond in &mol.bonds {
            let (atom_a, atom_b) = bond.endpoints();
            if atom_a < n && atom_b < n && atom_a != atom_b {
                degree[atom_a] += 1;
                degree[atom_b] += 1;
                usable += 1;
            }
        }
        let mut adj_start = Vec::with_capacity(n + 1);
        let mut running = 0u32;
        adj_start.push(0);
        for d in &degree {
            running += d;
            adj_start.push(running);
        }
        let mut cursor = adj_start.clone();
        let mut adj = vec![0u32; running as usize];
        for bond in &mol.bonds {
            let (atom_a, atom_b) = bond.endpoints();
            if atom_a < n && atom_b < n && atom_a != atom_b {
                adj[cursor[atom_a] as usize] = atom_b as u32;
                cursor[atom_a] += 1;
                adj[cursor[atom_b] as usize] = atom_a as u32;
                cursor[atom_b] += 1;
            }
        }

        Self {
            n,
            res_name_used: used_ids(&res_name),
            name_used: used_ids(&name),
            element_used: used_ids(&element),
            names: interner.names,
            res_name,
            res_seq,
            name,
            element,
            adj_start,
            adj,
            has_bonds: usable > 0,
        }
    }

    pub fn neighbours(&self, atom: usize) -> &[u32] {
        let lo = self.adj_start[atom] as usize;
        let hi = self.adj_start[atom + 1] as usize;
        &self.adj[lo..hi]
    }

    pub fn degree(&self, atom: usize) -> usize {
        (self.adj_start[atom + 1] - self.adj_start[atom]) as usize
    }

    /// Residue-name id of every atom, in atom order.
    pub fn res_name_ids(&self) -> &[u32] {
        &self.res_name
    }

    /// Atom-name id of every atom, in atom order.
    pub fn name_ids(&self) -> &[u32] {
        &self.name
    }

    /// Element id of every atom, in atom order.
    pub fn element_ids(&self) -> &[u32] {
        &self.element
    }

    /// Uppercased element symbol of one atom.
    pub fn element(&self, atom: usize) -> &str {
        self.text(self.element[atom])
    }

    /// Residue sequence number of one atom, where its source gave one.
    pub fn res_seq(&self, atom: usize) -> Option<i32> {
        self.res_seq
            .get(atom)
            .copied()
            .filter(|seq| *seq != NO_RES_SEQ)
    }

    fn text(&self, id: u32) -> &str {
        self.names.get(id as usize).map(|s| &**s).unwrap_or("")
    }

    /// The id `text` (already uppercased and trimmed) is interned under, or
    /// `None` when this molecule uses no such string at all.
    pub fn id_of(&self, text: &str) -> Option<u32> {
        self.names
            .iter()
            .position(|candidate| &**candidate == text)
            .map(|index| index as u32)
    }

    /// Ids for each of `words`, skipping the ones this molecule never uses.
    /// Predicates compare ids, so the string work happens once per query rather
    /// than once per atom.
    pub fn ids_of(&self, words: &[String]) -> Vec<u32> {
        words.iter().filter_map(|w| self.id_of(w)).collect()
    }

    /// Distinct residue names present, uppercased.
    pub fn residue_names(&self) -> Vec<String> {
        self.res_name_used
            .iter()
            .map(|&id| self.text(id).to_string())
            .collect()
    }

    fn has_res_name(&self, id: u32) -> bool {
        self.res_name_used.binary_search(&id).is_ok()
    }

    fn has_element(&self, id: u32) -> bool {
        self.element_used.binary_search(&id).is_ok()
    }

    fn has_atom_name(&self, id: u32) -> bool {
        self.name_used.binary_search(&id).is_ok()
    }
}

/// What a bare word in an expression turned out to mean. Reported back so the
/// command log can say `H -> element (12043 atoms)` instead of leaving the user
/// guessing which of four namespaces won.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentKind {
    Component,
    ResName,
    Element,
    AtomName,
}

impl IdentKind {
    pub fn label(self) -> &'static str {
        match self {
            IdentKind::Component => "component",
            IdentKind::ResName => "residue name",
            IdentKind::Element => "element",
            IdentKind::AtomName => "atom name",
        }
    }
}

#[derive(Clone, Debug)]
pub enum EvalError {
    /// A connectivity predicate on a structure with no bonds.
    NoBonds { predicate: &'static str },
    UnknownIdent {
        name: String,
        suggestion: Option<String>,
    },
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::NoBonds { predicate } => write!(
                f,
                "'{predicate}' needs bonded topology, but the current structure has no bonds\n  \
                 hint: load a .top (Ctrl+T / Ctrl+Shift+O) or use resid / index / resname instead"
            ),
            EvalError::UnknownIdent { name, suggestion } => match suggestion {
                Some(s) => write!(f, "unknown name '{name}'\n  hint: did you mean '{s}'?"),
                None => write!(
                    f,
                    "unknown name '{name}'\n  hint: run 'list' to see components, or use an \
                     explicit keyword such as 'resname {name}' / 'element {name}'"
                ),
            },
        }
    }
}

/// Everything an expression can refer to besides the molecule itself.
pub struct EvalCtx<'a> {
    pub table: &'a AtomTable,
    /// Components in panel order, as `(name, sorted original indices)`.
    pub components: &'a [(String, Vec<u32>)],
    /// Current atom pick, in original indices.
    pub selected: &'a [usize],
}

/// Evaluate `expr`, appending one note per resolved bare word to `notes`.
pub fn evaluate(
    expr: &Expr,
    ctx: &EvalCtx<'_>,
    notes: &mut Vec<String>,
) -> Result<Vec<bool>, EvalError> {
    let n = ctx.table.n;
    let mut set = vec![false; n];

    match expr {
        Expr::All => set.fill(true),
        Expr::None => {}
        Expr::Selected => {
            for &i in ctx.selected {
                if i < n {
                    set[i] = true;
                }
            }
        }
        Expr::ResName(names) => {
            let wanted = ctx.table.ids_of(&upper(names));
            for (slot, id) in set.iter_mut().zip(ctx.table.res_name_ids()).take(n) {
                *slot = wanted.contains(id);
            }
        }
        Expr::Name(names) => {
            let wanted = ctx.table.ids_of(&upper(names));
            for (slot, id) in set.iter_mut().zip(ctx.table.name_ids()).take(n) {
                *slot = wanted.contains(id);
            }
        }
        Expr::Element(names) => {
            let wanted = ctx.table.ids_of(&upper(names));
            for (slot, id) in set.iter_mut().zip(ctx.table.element_ids()).take(n) {
                *slot = wanted.contains(id);
            }
        }
        Expr::ResId(ranges) => {
            for (i, slot) in set.iter_mut().enumerate().take(n) {
                *slot = ctx
                    .table
                    .res_seq(i)
                    .is_some_and(|seq| in_ranges(ranges, seq as i64));
            }
        }
        Expr::Index(ranges) => {
            // 1-based on the way in, GROMACS style, matching the NDX files these
            // structures come with.
            for (i, slot) in set.iter_mut().enumerate().take(n) {
                *slot = in_ranges(ranges, i as i64 + 1);
            }
        }
        Expr::Hybrid(h) => {
            if !ctx.table.has_bonds {
                return Err(EvalError::NoBonds {
                    predicate: match h {
                        Hybrid::Sp => "sp",
                        Hybrid::Sp2 => "sp2",
                        Hybrid::Sp3 => "sp3",
                    },
                });
            }
            for (i, slot) in set.iter_mut().enumerate().take(n) {
                *slot = hybridisation(ctx.table.element(i), ctx.table.degree(i)) == Some(*h);
            }
        }
        Expr::NumBonds(spec) => {
            if !ctx.table.has_bonds {
                return Err(EvalError::NoBonds {
                    predicate: "numbonds",
                });
            }
            for (i, slot) in set.iter_mut().enumerate().take(n) {
                *slot = spec.matches(ctx.table.degree(i) as i64);
            }
        }
        Expr::With { count, of } => {
            if !ctx.table.has_bonds {
                return Err(EvalError::NoBonds { predicate: "with" });
            }
            let inner = evaluate(of, ctx, notes)?;
            for (i, slot) in set.iter_mut().enumerate().take(n) {
                let hits = ctx
                    .table
                    .neighbours(i)
                    .iter()
                    .filter(|&&nb| inner[nb as usize])
                    .count();
                *slot = count.matches(hits as i64);
            }
        }
        Expr::Ident(word) => {
            let (kind, resolved) = resolve_ident(word, ctx)?;
            let hits = resolved.iter().filter(|&&b| b).count();
            notes.push(format!(
                "{word} -> {} ({hits} atoms)",
                kind.label()
            ));
            set = resolved;
        }
        Expr::And(a, b) => {
            let lhs = evaluate(a, ctx, notes)?;
            let rhs = evaluate(b, ctx, notes)?;
            for i in 0..n {
                set[i] = lhs[i] && rhs[i];
            }
        }
        Expr::Or(a, b) => {
            let lhs = evaluate(a, ctx, notes)?;
            let rhs = evaluate(b, ctx, notes)?;
            for i in 0..n {
                set[i] = lhs[i] || rhs[i];
            }
        }
        Expr::Not(a) => {
            let inner = evaluate(a, ctx, notes)?;
            for i in 0..n {
                set[i] = !inner[i];
            }
        }
    }

    Ok(set)
}

/// Resolve a bare word, in this fixed order:
///
/// 1. an existing component, 2. a residue name, 3. an element symbol,
/// 4. an atom name.
///
/// Components first because they are the names the user just created and is
/// most likely to mean. Residue names before elements is what makes the DNA
/// residue `C` (cytosine) win over carbon — the collision is real, so every
/// resolution is reported back to the user and `element C` always says exactly
/// what it means.
fn resolve_ident(word: &str, ctx: &EvalCtx<'_>) -> Result<(IdentKind, Vec<bool>), EvalError> {
    let n = ctx.table.n;
    let upper = word.trim().to_ascii_uppercase();

    if let Some((_, atoms)) = ctx
        .components
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(word))
    {
        let mut set = vec![false; n];
        for &a in atoms {
            if (a as usize) < n {
                set[a as usize] = true;
            }
        }
        return Ok((IdentKind::Component, set));
    }

    // A word this molecule never uses in any column resolves to nothing, so the
    // three membership checks below are skipped along with it.
    if let Some(id) = ctx.table.id_of(&upper) {
        if ctx.table.has_res_name(id) {
            let set = ctx.table.res_name_ids()[..n]
                .iter()
                .map(|v| *v == id)
                .collect();
            return Ok((IdentKind::ResName, set));
        }

        if ctx.table.has_element(id) {
            let set = ctx.table.element_ids()[..n]
                .iter()
                .map(|v| *v == id)
                .collect();
            return Ok((IdentKind::Element, set));
        }

        if ctx.table.has_atom_name(id) {
            let set = ctx.table.name_ids()[..n]
                .iter()
                .map(|v| *v == id)
                .collect();
            return Ok((IdentKind::AtomName, set));
        }
    }

    Err(EvalError::UnknownIdent {
        name: word.to_string(),
        suggestion: nearest(&upper, ctx),
    })
}

/// Closest known name within a small edit distance, for the "did you mean" hint.
fn nearest(needle: &str, ctx: &EvalCtx<'_>) -> Option<String> {
    let mut candidates: Vec<String> = ctx.components.iter().map(|(n, _)| n.clone()).collect();
    candidates.extend(ctx.table.residue_names());

    let budget = if needle.len() <= 3 { 1 } else { 2 };
    candidates
        .into_iter()
        .filter(|c| !c.is_empty())
        .map(|c| {
            let d = edit_distance(needle, &c.to_ascii_uppercase());
            (d, c)
        })
        .filter(|(d, _)| *d <= budget)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Levenshtein distance over a two-row buffer.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Hybridisation from coordination number.
///
/// Bond *orders* would be the honest source, but they only survive the MOL2
/// path — TOP `[bonds]` and PDB `CONECT` carry none — so degree is all there is
/// for the GROMACS files this viewer is built around. The table is therefore a
/// heuristic: an amide or nitro nitrogen has three neighbours and reads as sp3
/// here even though it is really sp2.
pub fn hybridisation(element: &str, degree: usize) -> Option<Hybrid> {
    match (element, degree) {
        ("C" | "SI", 4) => Some(Hybrid::Sp3),
        ("C" | "SI", 3) => Some(Hybrid::Sp2),
        ("C" | "SI", 2) => Some(Hybrid::Sp),
        ("N" | "P", 3 | 4) => Some(Hybrid::Sp3),
        ("N", 2) => Some(Hybrid::Sp2),
        ("N", 1) => Some(Hybrid::Sp),
        ("O" | "S", 2) => Some(Hybrid::Sp3),
        ("O" | "S", 1) => Some(Hybrid::Sp2),
        _ => None,
    }
}

fn upper(words: &[String]) -> Vec<String> {
    words
        .iter()
        .map(|w| w.trim().to_ascii_uppercase())
        .collect()
}

fn in_ranges(ranges: &[NumRange], value: i64) -> bool {
    ranges.iter().any(|r| r.contains(value))
}

/// Fold a boolean mask into sorted original indices.
pub fn to_indices(set: &[bool]) -> Vec<u32> {
    set.iter()
        .enumerate()
        .filter(|&(_, &b)| b)
        .map(|(i, _)| i as u32)
        .collect()
}

/// Only used by `CountSpec`'s tests and the app's status text.
pub fn describe_count(spec: &CountSpec) -> String {
    match *spec {
        CountSpec::Exact(v) => format!("exactly {v}"),
        CountSpec::Range(lo, hi) => format!("{lo}-{hi}"),
        CountSpec::Ge(v) => format!(">= {v}"),
        CountSpec::Le(v) => format!("<= {v}"),
        CountSpec::Gt(v) => format!("> {v}"),
        CountSpec::Lt(v) => format!("< {v}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::parser::parse_statement;
    use crate::selection::ast::Statement;
    use crate::view_rs::push_named_atom;
    use lin_alg::f32::Vec3;
    use moleucle_3dview_rs::molecule::{AtomMeta, Bond};

    /// Propane, C3H8. Atom 0/1/2 are carbons (1 is the central CH2); 3..=10 are
    /// hydrogens: 3,4,5 on C0; 6,7 on C1; 8,9,10 on C2.
    fn propane() -> moleucle_3dview_rs::Molecule {
        fn meta<'a>(name: &'a str, res: &'a str, seq: i32) -> AtomMeta<'a> {
            AtomMeta {
                name: Some(name),
                res_name: Some(res),
                chain_id: Some('A'),
                res_seq: Some(seq),
                ..AtomMeta::default()
            }
        }
        let mut builder = Molecule::builder();
        for (i, nm) in ["C1", "C2", "C3"].iter().enumerate() {
            push_named_atom(
                &mut builder,
                Vec3::new(i as f32, 0.0, 0.0),
                "C",
                &meta(nm, "PRP", 1),
            );
        }
        for i in 0..8 {
            let name = format!("H{i}");
            push_named_atom(
                &mut builder,
                Vec3::new(i as f32, 1.0, 0.0),
                "H",
                &meta(&name, "PRP", 1),
            );
        }
        let bond = |a: usize, b: usize| Bond::new(a, b, 1);
        let bonds = vec![
            bond(0, 1),
            bond(1, 2),
            bond(0, 3),
            bond(0, 4),
            bond(0, 5),
            bond(1, 6),
            bond(1, 7),
            bond(2, 8),
            bond(2, 9),
            bond(2, 10),
        ];
        builder.finish(bonds)
    }

    fn run(mol: &moleucle_3dview_rs::Molecule, src: &str) -> Vec<u32> {
        let table = AtomTable::from_molecule(mol);
        let components: Vec<(String, Vec<u32>)> = vec![];
        let ctx = EvalCtx {
            table: &table,
            components: &components,
            selected: &[],
        };
        let Statement::Count(expr) = parse_statement(src).unwrap() else {
            panic!("expected a bare expression");
        };
        let mut notes = Vec::new();
        to_indices(&evaluate(&expr, &ctx, &mut notes).unwrap())
    }

    #[test]
    fn methylene_carbon_is_the_only_sp3_c_with_two_hydrogens() {
        // The headline example from the syntax design.
        assert_eq!(run(&propane(), "element C and sp3 and with 2 H"), vec![1]);
    }

    #[test]
    fn methyl_carbons_have_three_hydrogens() {
        assert_eq!(run(&propane(), "element C and sp3 and with 3 H"), vec![0, 2]);
    }

    #[test]
    fn every_propane_carbon_is_sp3() {
        assert_eq!(run(&propane(), "sp3"), vec![0, 1, 2]);
        assert_eq!(run(&propane(), "sp2"), Vec::<u32>::new());
    }

    #[test]
    fn numbonds_counts_the_graph_degree() {
        assert_eq!(run(&propane(), "numbonds 4"), vec![0, 1, 2]);
        assert_eq!(run(&propane(), "numbonds >=2"), vec![0, 1, 2]);
        assert_eq!(run(&propane(), "numbonds 1").len(), 8); // the hydrogens
    }

    #[test]
    fn index_is_one_based() {
        assert_eq!(run(&propane(), "index 1-3"), vec![0, 1, 2]);
        assert_eq!(run(&propane(), "index 1 3"), vec![0, 2]);
    }

    #[test]
    fn boolean_algebra_composes() {
        assert_eq!(
            run(&propane(), "not element H"),
            vec![0, 1, 2],
            "not inverts over the whole molecule"
        );
        assert_eq!(run(&propane(), "element C and index 1-2"), vec![0, 1]);
        assert_eq!(run(&propane(), "index 1 or index 11"), vec![0, 10]);
    }

    #[test]
    fn resname_and_resid_read_the_metadata() {
        assert_eq!(run(&propane(), "resname PRP").len(), 11);
        assert_eq!(run(&propane(), "resid 1").len(), 11);
        assert_eq!(run(&propane(), "resid 2"), Vec::<u32>::new());
    }

    #[test]
    fn nested_with_expressions_work() {
        // Carbons bonded to at least one other carbon: all three in propane.
        assert_eq!(run(&propane(), "element C and with >=1 (element C)"), vec![0, 1, 2]);
        // Carbons bonded to exactly two other carbons: only the middle one.
        assert_eq!(run(&propane(), "element C and with 2 (element C)"), vec![1]);
    }

    #[test]
    fn connectivity_predicates_refuse_a_structure_without_bonds() {
        // A bare .gro carries coordinates and no topology at all.
        let bondless = Molecule::from_atoms_bonds(propane().atoms, Vec::new());
        let table = AtomTable::from_molecule(&bondless);
        assert!(!table.has_bonds);
        let components: Vec<(String, Vec<u32>)> = vec![];
        let ctx = EvalCtx {
            table: &table,
            components: &components,
            selected: &[],
        };
        for src in ["with 2 H", "sp3", "numbonds 4"] {
            let Statement::Count(expr) = parse_statement(src).unwrap() else {
                panic!()
            };
            let mut notes = Vec::new();
            let err = evaluate(&expr, &ctx, &mut notes).unwrap_err();
            assert!(
                matches!(err, EvalError::NoBonds { .. }),
                "{src} should refuse, got {err:?}"
            );
        }
    }

    #[test]
    fn bare_words_resolve_component_then_resname_then_element() {
        let mol = propane();
        let table = AtomTable::from_molecule(&mol);
        let components = vec![("TAIL".to_string(), vec![2u32, 8, 9, 10])];
        let ctx = EvalCtx {
            table: &table,
            components: &components,
            selected: &[],
        };
        let eval_one = |src: &str| {
            let Statement::Count(expr) = parse_statement(src).unwrap() else {
                panic!()
            };
            let mut notes = Vec::new();
            let set = evaluate(&expr, &ctx, &mut notes).unwrap();
            (to_indices(&set), notes)
        };

        let (atoms, notes) = eval_one("TAIL");
        assert_eq!(atoms, vec![2, 8, 9, 10]);
        assert!(notes[0].contains("component"), "{:?}", notes);

        let (atoms, notes) = eval_one("PRP");
        assert_eq!(atoms.len(), 11);
        assert!(notes[0].contains("residue name"), "{:?}", notes);

        let (atoms, notes) = eval_one("H");
        assert_eq!(atoms.len(), 8, "H falls through to the element");
        assert!(notes[0].contains("element"), "{:?}", notes);
    }

    #[test]
    fn unknown_words_suggest_a_near_miss() {
        let mol = propane();
        let table = AtomTable::from_molecule(&mol);
        let components = vec![("TAIL".to_string(), vec![2u32])];
        let ctx = EvalCtx {
            table: &table,
            components: &components,
            selected: &[],
        };
        let Statement::Count(expr) = parse_statement("TAOL").unwrap() else {
            panic!()
        };
        let mut notes = Vec::new();
        match evaluate(&expr, &ctx, &mut notes).unwrap_err() {
            EvalError::UnknownIdent { suggestion, .. } => {
                assert_eq!(suggestion.as_deref(), Some("TAIL"))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn selected_reads_the_current_pick() {
        let mol = propane();
        let table = AtomTable::from_molecule(&mol);
        let components: Vec<(String, Vec<u32>)> = vec![];
        let ctx = EvalCtx {
            table: &table,
            components: &components,
            selected: &[4, 7],
        };
        let Statement::Count(expr) = parse_statement("selected").unwrap() else {
            panic!()
        };
        let mut notes = Vec::new();
        assert_eq!(
            to_indices(&evaluate(&expr, &ctx, &mut notes).unwrap()),
            vec![4, 7]
        );
    }

    #[test]
    fn out_of_range_bonds_do_not_panic() {
        // A TOP describing more atoms than the GRO produces exactly this.
        let mol = propane();
        let mut bonds = mol.bonds.clone();
        bonds.push(Bond::new(0, 9999, 1));
        let patched = Molecule::from_atoms_bonds(mol.atoms.clone(), bonds);
        let table = AtomTable::from_molecule(&patched);
        assert_eq!(table.degree(0), 4, "the bogus bond is dropped, not counted");
    }
}
