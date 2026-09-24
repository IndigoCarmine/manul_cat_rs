//! Resolving a parsed `plumed.dat` against a loaded structure.
//!
//! [`crate::parsing::plumed`] answers "what does this line say"; this module
//! answers "what does it point at" — which atoms, where the virtual atoms it
//! derives actually sit, and which vectors the collective variable measures.
//! That needs the molecule, so it lives here rather than with the parser.
//!
//! PLUMED serials are 1-based, as in a `.ndx` file. Unlike the NDX path
//! ([`crate::app`]'s `normalized_ndx_indices`), which quietly drops entries past
//! the end of the structure, a serial that does not exist is reported as an
//! error on its line: a PLUMED script written against the wrong structure is
//! exactly the mistake this viewer exists to catch.

use std::collections::HashMap;

use lin_alg::f32::Vec3;
use moleucle_3dview_rs::Molecule;

use crate::parsing::{AtomToken, PlumedAction, PlumedFile, parse_atom_list};

/// A vector one line measures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlumedVector {
    pub start: Vec3,
    pub end: Vec3,
    /// True for the central bond of a `TORSION` — the axis the dihedral is
    /// measured around, which is worth telling apart from the two arms.
    pub axis: bool,
}

/// What one PLUMED action contributes to the 3D view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlumedEntry {
    /// The atoms this line reads, kept split by the keyword that named them so
    /// a `GROUPA`/`GROUPB` pair can be drawn in two colours.
    pub atom_groups: Vec<Vec<usize>>,
    /// Virtual atoms (`CENTER`, `COM`) to mark with a sphere.
    pub markers: Vec<Vec3>,
    pub arrows: Vec<PlumedVector>,
    /// True when this geometry was inherited by following `ARG=` rather than
    /// named on the line itself, so the view can draw it more faintly.
    pub indirect: bool,
    pub error: Option<String>,
    /// One plain-language sentence for the detail box.
    pub summary: String,
}

impl PlumedEntry {
    pub fn atom_count(&self) -> usize {
        self.atom_groups.iter().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.atom_groups.iter().all(Vec::is_empty)
            && self.markers.is_empty()
            && self.arrows.is_empty()
    }
}

/// One entry of an `ATOMS=` list once resolved: a real atom, or the virtual
/// site a label stands for.
#[derive(Debug, Clone)]
struct Point {
    position: Vec3,
    /// The real atoms behind it — one for a serial, the whole group for a
    /// virtual site.
    members: Vec<usize>,
    is_virtual: bool,
}

/// How many `ARG=` hops to follow before giving up. Deep enough for the chains
/// real scripts build (`METAD` → `CUSTOM` → `CUSTOM` → `TORSION`), shallow
/// enough that a pathological file cannot stall the UI.
const MAX_ARG_DEPTH: usize = 8;

/// Resolve every action in `file` against `molecule`.
///
/// `masses` is the per-atom mass in molecule order, when a topology supplied
/// one; without it a mass-weighted centre falls back to the geometric one and
/// the summary says so.
pub fn resolve(file: &PlumedFile, molecule: &Molecule, masses: Option<&[f32]>) -> Vec<PlumedEntry> {
    let mut resolver = Resolver {
        file,
        molecule,
        masses,
        by_label: label_index(file),
        sites: vec![None; file.actions.len()],
    };

    let mut entries: Vec<PlumedEntry> = Vec::with_capacity(file.actions.len());
    for (idx, action) in file.actions.iter().enumerate() {
        let entry = resolver.entry(idx, action, &entries);
        entries.push(entry);
    }
    entries
}

/// A file with no structure loaded still lists, it just cannot point anywhere.
pub fn describe_only(file: &PlumedFile) -> Vec<PlumedEntry> {
    file.actions
        .iter()
        .map(|action| PlumedEntry {
            summary: headline(action),
            ..Default::default()
        })
        .collect()
}

fn label_index(file: &PlumedFile) -> HashMap<&str, usize> {
    let mut map = HashMap::new();
    for (idx, action) in file.actions.iter().enumerate() {
        if let Some(label) = action.label.as_deref() {
            // PLUMED takes the first definition of a duplicated label, so so do we.
            map.entry(label).or_insert(idx);
        }
    }
    map
}

struct Resolver<'a> {
    file: &'a PlumedFile,
    molecule: &'a Molecule,
    masses: Option<&'a [f32]>,
    by_label: HashMap<&'a str, usize>,
    /// Memoised virtual site per action index: `None` = not tried yet.
    sites: Vec<Option<Result<Point, String>>>,
}

impl<'a> Resolver<'a> {
    fn entry(
        &mut self,
        idx: usize,
        action: &'a PlumedAction,
        done: &[PlumedEntry],
    ) -> PlumedEntry {
        let name = action.name.as_str();
        let summary = headline(action);

        let geometry = match name {
            "CENTER" | "COM" => self.center_entry(action),
            "DISTANCE" => self.chain_entry(action, 2, ChainKind::Distance),
            "ANGLE" => self.chain_entry(action, 0, ChainKind::Angle),
            "TORSION" | "DIHEDRAL" => self.chain_entry(action, 4, ChainKind::Torsion),
            "POSITION" => self.chain_entry(action, 1, ChainKind::Position),
            "GYRATION" => self.group_entry(action, true),
            _ => {
                if action.atom_keywords().next().is_some() {
                    self.group_entry(action, false)
                } else if action.keyword("ARG").is_some() {
                    return self.inherited_entry(idx, action, done, summary);
                } else {
                    Ok(PlumedEntry::default())
                }
            }
        };

        match geometry {
            Ok(mut entry) => {
                entry.summary = format!("{summary}{}", detail_suffix(action, &entry));
                entry
            }
            Err(error) => PlumedEntry {
                summary,
                error: Some(error),
                ..Default::default()
            },
        }
    }

    /// `CENTER` / `COM`: highlight the members, mark the derived point.
    fn center_entry(&mut self, action: &'a PlumedAction) -> Result<PlumedEntry, String> {
        let point = self.site_of(action, &mut Vec::new())?;
        Ok(PlumedEntry {
            atom_groups: vec![distinct(point.members.iter().copied())],
            markers: vec![point.position],
            ..Default::default()
        })
    }

    /// An action whose `ATOMS=` is an ordered list of points that a vector runs
    /// between. `expected` of 0 means "any count" (`ANGLE` takes 3 or 4).
    fn chain_entry(
        &mut self,
        action: &'a PlumedAction,
        expected: usize,
        kind: ChainKind,
    ) -> Result<PlumedEntry, String> {
        let value = action
            .keyword("ATOMS")
            .ok_or_else(|| format!("{} has no ATOMS", action.name))?;
        let points = self.points(value, action.line, &mut Vec::new())?;

        let ok = match kind {
            ChainKind::Angle => matches!(points.len(), 3 | 4),
            _ => points.len() == expected,
        };
        if !ok {
            return Err(format!(
                "{} takes {} atoms, but {} were given",
                action.name,
                match kind {
                    ChainKind::Angle => "3 or 4".to_string(),
                    _ => expected.to_string(),
                },
                points.len()
            ));
        }

        let arrows = match kind {
            ChainKind::Position => Vec::new(),
            ChainKind::Distance => vec![vector(&points[0], &points[1], false)],
            ChainKind::Angle if points.len() == 3 => {
                // The angle between (a1 - a2) and (a3 - a2): both arms leave the
                // vertex, which is the atom in the middle.
                vec![
                    vector(&points[1], &points[0], false),
                    vector(&points[1], &points[2], false),
                ]
            }
            ChainKind::Angle => {
                // Four atoms: the angle between the vectors a1→a2 and a3→a4.
                vec![
                    vector(&points[0], &points[1], false),
                    vector(&points[2], &points[3], false),
                ]
            }
            ChainKind::Torsion => vec![
                vector(&points[0], &points[1], false),
                // The central bond is the axis the dihedral turns around.
                vector(&points[1], &points[2], true),
                vector(&points[2], &points[3], false),
            ],
        };

        Ok(PlumedEntry {
            atom_groups: vec![distinct(points.iter().flat_map(|p| p.members.iter().copied()))],
            markers: points
                .iter()
                .filter(|p| p.is_virtual)
                .map(|p| p.position)
                .collect(),
            arrows,
            ..Default::default()
        })
    }

    /// Anything else that names atoms: one highlight group per atom keyword, so
    /// `GROUPA`/`GROUPB` stay tellable apart.
    fn group_entry(
        &mut self,
        action: &'a PlumedAction,
        mark_center: bool,
    ) -> Result<PlumedEntry, String> {
        let keywords: Vec<(usize, String)> = action
            .atom_keywords()
            .map(|kw| (kw.line, kw.value.clone()))
            .collect();

        let mut atom_groups = Vec::new();
        let mut markers = Vec::new();
        for (line, value) in keywords {
            let points = self.points(&value, line, &mut Vec::new())?;
            let members = distinct(points.iter().flat_map(|p| p.members.iter().copied()));
            markers.extend(points.iter().filter(|p| p.is_virtual).map(|p| p.position));
            if mark_center && !members.is_empty() {
                markers.push(self.centroid(&members, false));
            }
            atom_groups.push(members);
        }

        Ok(PlumedEntry {
            atom_groups,
            markers,
            ..Default::default()
        })
    }

    /// A bias or function: it names no atoms of its own, so show the geometry of
    /// the CVs it reads through `ARG=`.
    fn inherited_entry(
        &mut self,
        idx: usize,
        action: &'a PlumedAction,
        done: &[PlumedEntry],
        summary: String,
    ) -> PlumedEntry {
        let mut merged = PlumedEntry {
            summary,
            indirect: true,
            ..Default::default()
        };

        let mut sources: Vec<usize> = Vec::new();
        self.collect_arg_sources(idx, MAX_ARG_DEPTH, &mut sources);

        // The CVs a bias reads overlap heavily -- six per-spoke torsions all
        // contain the same two rosette centres -- so the union is taken rather
        // than the concatenation. Without this a `METAD` over six torsions
        // claims six copies of every shared atom, marker and axis vector.
        let mut atoms: Vec<usize> = Vec::new();
        for source in sources {
            // Only backwards references are resolved: PLUMED requires a label to
            // be defined before it is used, and `done` holds exactly those.
            let Some(entry) = done.get(source) else {
                continue;
            };
            atoms.extend(entry.atom_groups.iter().flatten().copied());
            for marker in &entry.markers {
                push_unique_point(&mut merged.markers, *marker);
            }
            for arrow in &entry.arrows {
                push_unique_vector(&mut merged.arrows, *arrow);
            }
        }
        let atoms = distinct(atoms.into_iter());
        if !atoms.is_empty() {
            merged.atom_groups.push(atoms);
        }

        let names = arg_names(action);
        merged.summary = format!(
            "{} · follows {} to {} atom{}",
            merged.summary,
            if names.len() == 1 {
                format!("`{}`", names[0])
            } else {
                format!("{} arguments", names.len())
            },
            merged.atom_count(),
            if merged.atom_count() == 1 { "" } else { "s" },
        );
        merged
    }

    /// Walk `ARG=` transitively, collecting the action indices that actually
    /// carry geometry.
    fn collect_arg_sources(&self, idx: usize, depth: usize, out: &mut Vec<usize>) {
        if depth == 0 {
            return;
        }
        let Some(action) = self.file.actions.get(idx) else {
            return;
        };
        for name in arg_names(action) {
            // `mtd.bias` names a component of the `mtd` action.
            let base = name.split('.').next().unwrap_or(&name);
            let Some(&target) = self.by_label.get(base) else {
                continue;
            };
            if target == idx || out.contains(&target) {
                continue;
            }
            let Some(target_action) = self.file.actions.get(target) else {
                continue;
            };
            if target_action.atom_keywords().next().is_some() {
                out.push(target);
            } else {
                self.collect_arg_sources(target, depth - 1, out);
            }
        }
    }

    /// The virtual site an action stands for when it is named in an `ATOMS=`
    /// list, memoised. `visiting` is the label chain currently being resolved,
    /// so `a: CENTER ATOMS=b` / `b: CENTER ATOMS=a` reports a cycle instead of
    /// recursing until the stack runs out.
    fn site(&mut self, idx: usize, visiting: &mut Vec<usize>) -> Result<Point, String> {
        if let Some(cached) = &self.sites[idx] {
            return cached.clone();
        }
        // `file` outlives `self`, so copying the reference out frees the
        // `&mut self` calls below from borrowing it.
        let file = self.file;
        if visiting.contains(&idx) {
            let label = file.actions[idx].label.as_deref().unwrap_or("?");
            return Err(format!("`{label}` is defined in terms of itself"));
        }

        visiting.push(idx);
        let result = self.site_of(&file.actions[idx], visiting);
        visiting.pop();

        self.sites[idx] = Some(result.clone());
        result
    }

    /// Compute one action's virtual site, without the memo/cycle bookkeeping.
    fn site_of(&mut self, action: &'a PlumedAction, visiting: &mut Vec<usize>) -> Result<Point, String> {
        let value = action
            .keyword("ATOMS")
            .ok_or_else(|| format!("{} has no ATOMS", action.name))?;
        let points = self.points(value, action.line, visiting)?;
        if points.is_empty() {
            return Err(format!("{} names no atoms", action.name));
        }

        let members: Vec<usize> = points.iter().flat_map(|p| p.members.clone()).collect();
        let weighted = wants_mass_weighting(action) && self.masses.is_some();
        Ok(Point {
            position: self.centroid(&members, weighted),
            members,
            is_virtual: true,
        })
    }

    /// Resolve one `ATOMS=` value into ordered points.
    fn points(
        &mut self,
        value: &str,
        line: usize,
        visiting: &mut Vec<usize>,
    ) -> Result<Vec<Point>, String> {
        let tokens = parse_atom_list(value, line).map_err(|err| err.to_string())?;
        let atom_count = self.molecule.atoms.len();
        let mut points = Vec::new();

        for token in tokens {
            match token {
                AtomToken::Range { .. } => {
                    let mut serials = Vec::new();
                    token.extend_serials(&mut serials);
                    for serial in serials {
                        let index = (serial as usize)
                            .checked_sub(1)
                            .filter(|i| *i < atom_count)
                            .ok_or_else(|| {
                                format!(
                                    "atom {serial} does not exist — the structure has {atom_count} atoms"
                                )
                            })?;
                        points.push(Point {
                            position: self.molecule.atoms[index].position,
                            members: vec![index],
                            is_virtual: false,
                        });
                    }
                }
                AtomToken::Label(name) => {
                    // The file this is resolved against is the one the label map
                    // was built from, so a hit is always a valid index.
                    let Some(&idx) = self.by_label.get(name.as_str()) else {
                        return Err(format!("`{name}` is not defined in this file"));
                    };
                    points.push(self.site(idx, visiting)?);
                }
            }
        }

        Ok(points)
    }

    fn centroid(&self, members: &[usize], mass_weighted: bool) -> Vec3 {
        let mut sum = Vec3::new(0.0, 0.0, 0.0);
        let mut total = 0.0f32;

        for &index in members {
            let Some(atom) = self.molecule.atoms.get(index) else {
                continue;
            };
            let weight = if mass_weighted {
                self.masses
                    .and_then(|m| m.get(index).copied())
                    .filter(|m| *m > 0.0)
                    .unwrap_or(1.0)
            } else {
                1.0
            };
            sum += atom.position * weight;
            total += weight;
        }

        if total > 0.0 { sum / total } else { sum }
    }
}

#[derive(Clone, Copy)]
enum ChainKind {
    Distance,
    Angle,
    Torsion,
    Position,
}

/// Two positions closer than this are the same point, for the purpose of not
/// drawing one marker on top of another. Well below any real bond length, and
/// well above the float noise two identical centroid sums can differ by.
const SAME_POINT_TOLERANCE: f32 = 1e-4;

fn same_point(a: Vec3, b: Vec3) -> bool {
    (a - b).magnitude() <= SAME_POINT_TOLERANCE
}

fn push_unique_point(out: &mut Vec<Vec3>, point: Vec3) {
    // Linear, but these lists hold a handful of entries even for a bias over a
    // dozen CVs.
    if !out.iter().any(|existing| same_point(*existing, point)) {
        out.push(point);
    }
}

fn push_unique_vector(out: &mut Vec<PlumedVector>, vector: PlumedVector) {
    let duplicate = out.iter().any(|existing| {
        same_point(existing.start, vector.start) && same_point(existing.end, vector.end)
    });
    if !duplicate {
        out.push(vector);
    }
}

/// Sorted, without repeats. An `ATOMS=` list may name the same atom twice —
/// directly and again through a virtual atom that contains it — and stacking
/// two highlight spheres on one position only makes them z-fight.
fn distinct(indices: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut out: Vec<usize> = indices.collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn vector(from: &Point, to: &Point, axis: bool) -> PlumedVector {
    PlumedVector {
        start: from.position,
        end: to.position,
        axis,
    }
}

/// PLUMED's `CENTER` is the geometric centre unless it is asked for mass
/// weighting; `COM` always is.
fn wants_mass_weighting(action: &PlumedAction) -> bool {
    action.name == "COM" || action.has_flag("MASS") || action.keyword("MASS").is_some()
}

fn arg_names(action: &PlumedAction) -> Vec<String> {
    action
        .keyword("ARG")
        .map(|value| {
            value
                .split([',', ' '])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The first half of a line's description: what the action is, in words.
fn headline(action: &PlumedAction) -> String {
    let what = match action.name.as_str() {
        "UNITS" => "sets the units the rest of the file is written in",
        "CENTER" => "defines a virtual atom at the centre of a group",
        "COM" => "defines a virtual atom at a group's centre of mass",
        "GROUP" => "names a group of atoms for later lines",
        "WHOLEMOLECULES" => "keeps a molecule whole across the periodic boundary",
        "DISTANCE" => "measures the distance between two points",
        "ANGLE" => "measures an angle",
        "TORSION" | "DIHEDRAL" => "measures a dihedral (signed torsion) angle",
        "POSITION" => "reads one atom's position",
        "GYRATION" => "measures a group's radius of gyration",
        "RMSD" => "measures RMSD against a reference structure",
        "COORDINATION" => "counts contacts between two groups",
        "CUSTOM" | "MATHEVAL" => "computes a function of other variables",
        "COMBINE" => "combines other variables linearly",
        "METAD" => "adds a metadynamics bias",
        "UPPER_WALLS" => "pushes back above a threshold",
        "LOWER_WALLS" => "pushes back below a threshold",
        "RESTRAINT" => "applies a harmonic restraint",
        "PRINT" => "writes variables to an output file",
        "FLUSH" => "flushes the output files",
        _ => "",
    };

    let label = action
        .label
        .as_deref()
        .map(|l| format!("`{l}` "))
        .unwrap_or_default();

    if what.is_empty() {
        format!("{label}{} (not specially handled by this viewer)", action.name)
    } else {
        format!("{label}{}", what)
    }
}

/// The second half: what was actually resolved.
fn detail_suffix(action: &PlumedAction, entry: &PlumedEntry) -> String {
    let mut parts: Vec<String> = Vec::new();

    let atoms = entry.atom_count();
    if atoms > 0 {
        parts.push(format!("{atoms} atom{}", if atoms == 1 { "" } else { "s" }));
    }
    if !entry.markers.is_empty() {
        parts.push(format!(
            "{} marker{}",
            entry.markers.len(),
            if entry.markers.len() == 1 { "" } else { "s" }
        ));
    }
    if !entry.arrows.is_empty() {
        parts.push(format!(
            "{} vector{}",
            entry.arrows.len(),
            if entry.arrows.len() == 1 { "" } else { "s" }
        ));
    }
    if matches!(action.name.as_str(), "CENTER" | "COM") {
        parts.push(
            if wants_mass_weighting(action) {
                "mass-weighted"
            } else {
                "geometric centre"
            }
            .to_string(),
        );
    }
    if let Some(reference) = action.keyword("REFERENCE") {
        parts.push(format!("reference {reference}"));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_rs::view_atom;
    use moleucle_3dview_rs::Molecule;

    /// `n` carbons on the x axis at 0, 1, 2, … nm, so a centroid is easy to
    /// check by eye.
    fn line_molecule(n: usize) -> Molecule {
        let atoms = (0..n)
            .map(|i| view_atom(Vec3::new(i as f32, 0.0, 0.0), "C"))
            .collect();
        Molecule::from_atoms_bonds(atoms, Vec::new())
    }

    fn entries(input: &str, atoms: usize) -> Vec<PlumedEntry> {
        let file = PlumedFile::parse(input).expect("should parse");
        resolve(&file, &line_molecule(atoms), None)
    }

    #[test]
    fn a_center_marks_the_midpoint_of_its_members() {
        let got = entries("c: CENTER ATOMS=1-3\n", 4);
        assert_eq!(got[0].error, None);
        assert_eq!(got[0].atom_groups, vec![vec![0, 1, 2]]);
        assert_eq!(got[0].markers, vec![Vec3::new(1.0, 0.0, 0.0)]);
        assert!(got[0].summary.contains("geometric centre"), "{}", got[0].summary);
    }

    #[test]
    fn com_is_mass_weighted_when_masses_are_available() {
        let file = PlumedFile::parse("c: COM ATOMS=1,2\n").unwrap();
        let molecule = line_molecule(2);
        // Atom 2 is three times as heavy, so the centre sits three quarters along.
        let got = resolve(&file, &molecule, Some(&[1.0, 3.0]));
        assert_eq!(got[0].markers, vec![Vec3::new(0.75, 0.0, 0.0)]);
        assert!(got[0].summary.contains("mass-weighted"), "{}", got[0].summary);
    }

    #[test]
    fn a_center_falls_back_to_the_geometric_centre_without_masses() {
        let got = entries("c: COM ATOMS=1,2\n", 2);
        assert_eq!(got[0].markers, vec![Vec3::new(0.5, 0.0, 0.0)]);
    }

    #[test]
    fn a_serial_past_the_structure_is_an_error_and_draws_nothing() {
        let got = entries("c: CENTER ATOMS=1-99\n", 4);
        let err = got[0].error.as_deref().unwrap_or_default();
        assert!(err.contains("atom 5 does not exist"), "{err}");
        assert!(got[0].is_empty());
    }

    #[test]
    fn a_distance_draws_one_arrow_between_its_two_atoms() {
        let got = entries("d: DISTANCE ATOMS=1,3\n", 4);
        assert_eq!(got[0].error, None);
        assert_eq!(got[0].arrows.len(), 1);
        assert_eq!(got[0].arrows[0].start, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(got[0].arrows[0].end, Vec3::new(2.0, 0.0, 0.0));
        assert!(!got[0].arrows[0].axis);
    }

    #[test]
    fn a_three_atom_angle_draws_both_arms_from_the_vertex() {
        let got = entries("a: ANGLE ATOMS=1,2,3\n", 3);
        let arms = &got[0].arrows;
        assert_eq!(arms.len(), 2);
        // Both leave atom 2, the vertex.
        assert_eq!(arms[0].start, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(arms[1].start, Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn a_four_atom_angle_draws_the_two_vectors_it_compares() {
        let got = entries("a: ANGLE ATOMS=1,2,3,4\n", 4);
        let arms = &got[0].arrows;
        assert_eq!(arms.len(), 2);
        assert_eq!(arms[0].start, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(arms[0].end, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(arms[1].start, Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(arms[1].end, Vec3::new(3.0, 0.0, 0.0));
    }

    #[test]
    fn a_torsion_draws_three_bonds_and_flags_the_axis() {
        let got = entries("t: TORSION ATOMS=1,2,3,4\n", 4);
        let arrows = &got[0].arrows;
        assert_eq!(arrows.len(), 3);
        assert_eq!(
            arrows.iter().map(|a| a.axis).collect::<Vec<_>>(),
            vec![false, true, false]
        );
    }

    #[test]
    fn an_action_with_the_wrong_atom_count_says_so() {
        let got = entries("t: TORSION ATOMS=1,2\n", 4);
        let err = got[0].error.as_deref().unwrap_or_default();
        assert!(err.contains("takes 4 atoms, but 2 were given"), "{err}");
    }

    #[test]
    fn a_torsion_resolves_virtual_atoms_named_in_its_atoms_list() {
        let got = entries(
            "lo: CENTER ATOMS=1,2\nup: CENTER ATOMS=3,4\nt: TORSION ATOMS=1,lo,up,4\n",
            4,
        );
        let torsion = &got[2];
        assert_eq!(torsion.error, None);
        // Atom 1, both centres' members, and atom 4 — each atom once.
        assert_eq!(torsion.atom_groups, vec![vec![0, 1, 2, 3]]);
        // Only the two centres are marked, not the two real atoms.
        assert_eq!(
            torsion.markers,
            vec![Vec3::new(0.5, 0.0, 0.0), Vec3::new(2.5, 0.0, 0.0)]
        );
        assert_eq!(torsion.arrows[1].start, Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(torsion.arrows[1].end, Vec3::new(2.5, 0.0, 0.0));
    }

    #[test]
    fn an_undefined_label_is_an_error() {
        let got = entries("d: DISTANCE ATOMS=nope,1\n", 4);
        let err = got[0].error.as_deref().unwrap_or_default();
        assert!(err.contains("`nope` is not defined"), "{err}");
    }

    #[test]
    fn a_self_referential_label_reports_a_cycle_instead_of_hanging() {
        let got = entries("a: CENTER ATOMS=b\nb: CENTER ATOMS=a\nd: DISTANCE ATOMS=a,1\n", 4);
        // `a` cannot resolve, and the line that uses it says why rather than
        // recursing until the stack runs out.
        assert!(got[0].error.is_some());
        let err = got[2].error.as_deref().unwrap_or_default();
        assert!(err.contains("defined in terms of itself"), "{err}");
    }

    #[test]
    fn a_bias_inherits_the_geometry_of_the_cvs_it_reads() {
        let got = entries(
            "d: DISTANCE ATOMS=1,2\nf: CUSTOM ARG=d VAR=a FUNC=a PERIODIC=NO\nMETAD ARG=f PACE=500\n",
            4,
        );
        for bias in &got[1..] {
            assert!(bias.indirect, "{}", bias.summary);
            assert_eq!(bias.arrows.len(), 1, "{}", bias.summary);
            assert_eq!(bias.atom_groups, vec![vec![0, 1]]);
        }
        // METAD reaches the DISTANCE through the CUSTOM in between.
        assert!(got[2].summary.contains("2 atoms"), "{}", got[2].summary);
    }

    #[test]
    fn a_bias_counts_atoms_its_arguments_share_only_once() {
        // Two distances over three atoms, sharing the middle one.
        let got = entries(
            "a: DISTANCE ATOMS=1,2\nb: DISTANCE ATOMS=2,3\nMETAD ARG=a,b PACE=1\n",
            3,
        );
        let metad = got.last().unwrap();
        assert_eq!(metad.atom_groups, vec![vec![0, 1, 2]]);
        assert_eq!(metad.arrows.len(), 2);
    }

    #[test]
    fn a_bias_draws_a_vector_its_arguments_share_only_once() {
        let got = entries(
            "a: DISTANCE ATOMS=1,2\nb: DISTANCE ATOMS=1,2\nMETAD ARG=a,b PACE=1\n",
            3,
        );
        let metad = got.last().unwrap();
        assert_eq!(metad.arrows.len(), 1);
        assert_eq!(metad.atom_groups, vec![vec![0, 1]]);
    }

    #[test]
    fn a_pairwise_action_keeps_its_two_groups_apart() {
        let got = entries("c: COORDINATION GROUPA=1,2 GROUPB=3,4 R_0=0.5\n", 4);
        assert_eq!(got[0].atom_groups, vec![vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn an_unknown_action_still_highlights_the_atoms_it_names() {
        let got = entries("x: SOME_FUTURE_CV ATOMS=2,3 WIDTH=0.1\n", 4);
        assert_eq!(got[0].error, None);
        assert_eq!(got[0].atom_groups, vec![vec![1, 2]]);
        assert!(
            got[0].summary.contains("not specially handled"),
            "{}",
            got[0].summary
        );
    }

    #[test]
    fn an_action_with_no_atoms_and_no_args_is_listed_without_geometry() {
        let got = entries("UNITS LENGTH=nm TIME=ps\n", 4);
        assert_eq!(got[0].error, None);
        assert!(got[0].is_empty());
        assert!(got[0].summary.contains("units"), "{}", got[0].summary);
    }

    #[test]
    fn describing_without_a_structure_lists_every_action() {
        let file = PlumedFile::parse("c: CENTER ATOMS=1-3\nd: DISTANCE ATOMS=c,1\n").unwrap();
        let got = describe_only(&file);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|e| e.is_empty() && e.error.is_none()));
    }
}
