//! Martini coarse-grained force-field registry.
//!
//! The Martini master `.itp` (e.g. `martini_v3.0.0.itp`) does not describe a
//! molecule — it defines the *bead types*: an `[ atomtypes ]` list plus the
//! `[ nonbond_params ]` Lennard-Jones table. We use it purely as a lookup:
//!
//! * `[ nonbond_params ]` self-pairs (`type type`) give each bead's LJ sigma in
//!   nanometers, which sets the rendered bead radius (`sigma / 2`).
//! * The bead-type name encodes chemistry (`P` polar, `N` intermediate, `C`
//!   apolar, `Q` charged, `X`/`D`/`W`/`U` special) after an optional size prefix
//!   (`S` small, `T` tiny), which sets the bead colour.
//!
//! A structure/topology tells us *which* bead type each particle is (see the
//! app's bead-type resolution); this registry turns that type into a
//! `(radius, colour)`, applied as per-atom overrides on the main molecule.

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Fewest declared bead types a file must have to be considered a Martini force
/// field. Every Martini master `.itp` declares dozens; a handful of
/// `[ atomtypes ]` lines is far more likely a small hand-written topology.
const MIN_BEAD_TYPES: usize = 8;

/// Fraction of declared types that must fit the Martini bead-name grammar.
/// Not 1.0: real Martini files carry a few off-grammar extras (Martini 2's
/// `AC1`/`AC2` ring beads and the `BP4` antifreeze bead, for instance).
const MIN_BEAD_NAME_RATIO: f32 = 0.8;

/// Parsed Martini bead-type registry. Empty when the loaded `.itp` is not a
/// Martini force field (no `[ atomtypes ]` / `[ nonbond_params ]`).
#[derive(Debug, Clone, Default)]
pub struct MartiniForceField {
    /// Bead type name -> LJ sigma (nm), taken from the `[ nonbond_params ]`
    /// self-interaction (`type type`) line.
    sigma_nm: HashMap<String, f32>,
    /// Every bead type declared in `[ atomtypes ]` (superset of `sigma_nm`).
    types: HashMap<String, ()>,
}

impl MartiniForceField {
    pub fn load_from_path(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::parse(&content))
    }

    /// Parse the `[ atomtypes ]` and `[ nonbond_params ]` sections. Other
    /// sections (defaults, bondtypes, molecule topologies, …) are ignored.
    pub fn parse(content: &str) -> Self {
        let mut sigma_nm = HashMap::new();
        let mut types = HashMap::new();
        let mut section = Section::Other;

        for line in content.lines() {
            let line = match line.split(';').next() {
                Some(head) => head.trim(),
                None => continue,
            };
            if line.is_empty() {
                continue;
            }

            if let Some(name) = line.strip_prefix('[').and_then(|s| s.split(']').next()) {
                section = match name.trim().to_ascii_lowercase().as_str() {
                    "atomtypes" => Section::AtomTypes,
                    "nonbond_params" => Section::NonbondParams,
                    _ => Section::Other,
                };
                continue;
            }

            let mut parts = line.split_whitespace();
            match section {
                Section::AtomTypes => {
                    // `name mass charge ptype V W`
                    if let Some(name) = parts.next() {
                        types.insert(name.to_string(), ());
                    }
                }
                Section::NonbondParams => {
                    // `typeI typeJ funct sigma epsilon` (sigma-epsilon form).
                    // Keep only the self-interaction, which fixes the bead size.
                    let (Some(i), Some(j)) = (parts.next(), parts.next()) else {
                        continue;
                    };
                    if i != j {
                        continue;
                    }
                    // Skip funct, read sigma.
                    if let Some(sigma) = parts.nth(1).and_then(|s| s.parse::<f32>().ok())
                        && sigma > 0.0 {
                            sigma_nm.insert(i.to_string(), sigma);
                        }
                }
                Section::Other => {}
            }
        }

        Self { sigma_nm, types }
    }

    /// True when this looks like a *Martini* force field.
    ///
    /// Having an `[ atomtypes ]` section is not enough: every atomistic GROMACS
    /// force field (Amber, CHARMM, OPLS, GROMOS) declares one too, so treating
    /// any file with atom types as Martini made ordinary `.top` files register
    /// as coarse-grained. What actually distinguishes Martini is its bead-name
    /// grammar (`W`, `P4`, `SN3a`, `TC5`, `SQ4p`), which atomistic type names
    /// (`HC`, `OW`, `CT1`, `opls_001`) do not fit — so we require a decent
    /// number of declared types and a large majority of them to be bead names.
    pub fn is_forcefield(&self) -> bool {
        let declared: HashSet<&str> = self
            .types
            .keys()
            .chain(self.sigma_nm.keys())
            .map(String::as_str)
            .collect();
        if declared.len() < MIN_BEAD_TYPES {
            return false;
        }
        let bead_like = declared
            .iter()
            .filter(|name| looks_like_bead_type(name))
            .count();
        bead_like as f32 >= declared.len() as f32 * MIN_BEAD_NAME_RATIO
    }

    /// Number of distinct bead types declared.
    pub fn bead_type_count(&self) -> usize {
        self.types.len().max(self.sigma_nm.len())
    }

    /// Whether `bead_type` is a bead type declared by this force field.
    pub fn is_known(&self, bead_type: &str) -> bool {
        let key = bead_type.trim();
        self.types.contains_key(key) || self.sigma_nm.contains_key(key)
    }

    /// Rendered sphere radius (nm) for a bead type: half the LJ sigma when known,
    /// otherwise a size-class estimate from the `S`/`T` prefix. `None` if the
    /// type is not a recognised bead.
    pub fn radius_nm(&self, bead_type: &str) -> Option<f32> {
        let key = bead_type.trim();
        if let Some(&sigma) = self.sigma_nm.get(key) {
            return Some(sigma * 0.5);
        }
        if !self.is_known(key) {
            return None;
        }
        // Known type but no self-pair sigma: fall back to nominal Martini 3 sizes.
        Some(match size_prefix(key) {
            Some('T') => 0.34 * 0.5, // tiny
            Some('S') => 0.41 * 0.5, // small
            _ => 0.47 * 0.5,         // regular
        })
    }

    /// Colour for a bead type, keyed on its chemical class. Static: needs no
    /// force-field data, so it also colours beads whose type is only known by
    /// name (fallback path).
    pub fn color(bead_type: &str) -> (f32, f32, f32) {
        class_color(chem_class(bead_type))
    }
}

enum Section {
    AtomTypes,
    NonbondParams,
    Other,
}

/// The leading size prefix of a Martini bead type, if any (`S` small, `T` tiny).
/// Only treated as a prefix when a chemical-class letter follows it, so a
/// regular type never loses its first letter.
fn size_prefix(bead: &str) -> Option<char> {
    let b = bead.trim().as_bytes();
    if b.len() >= 2 && (b[0] == b'S' || b[0] == b'T') && is_class_byte(b[1]) {
        Some(b[0] as char)
    } else {
        None
    }
}

/// Chemical-class letter of a bead type, ignoring any size prefix.
fn chem_class(bead: &str) -> char {
    let b = bead.trim().as_bytes();
    if b.is_empty() {
        return '?';
    }
    if size_prefix(bead).is_some() {
        b[1] as char
    } else {
        b[0] as char
    }
}

/// Whether `name` fits the Martini bead-type naming grammar: an optional size
/// prefix (`S` small, `T` tiny), a chemical-class letter, then an optional
/// polarity level and lowercase sub-type label — `W`, `P4`, `SP1`, `TC5`,
/// `N4a`, `SQ4p`. Deliberately case-strict, so atomistic type names such as
/// `CT1`, `NH1` or `HW` are rejected.
fn looks_like_bead_type(name: &str) -> bool {
    let b = name.trim().as_bytes();
    let mut i = if size_prefix(name).is_some() { 1 } else { 0 };
    if b.get(i).copied().is_none_or(|c| !is_class_byte(c)) {
        return false;
    }
    i += 1;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_lowercase() {
        i += 1;
    }
    i == b.len()
}

fn is_class_byte(c: u8) -> bool {
    matches!(c, b'P' | b'N' | b'C' | b'X' | b'Q' | b'W' | b'D' | b'U')
}

fn class_color(class: char) -> (f32, f32, f32) {
    match class {
        'P' => (0.20, 0.45, 0.95), // polar      -> blue
        'N' => (0.20, 0.80, 0.60), // intermediate -> teal
        'C' => (0.55, 0.55, 0.58), // apolar     -> grey
        'X' => (0.65, 0.35, 0.85), // halo/special -> purple
        'Q' => (0.95, 0.35, 0.55), // charged    -> pink/red
        'W' => (0.30, 0.70, 1.00), // water      -> cyan
        'D' => (0.95, 0.85, 0.25), // divalent/dummy -> yellow
        'U' => (0.80, 0.80, 0.80), // unspecified -> light grey
        _ => (0.70, 0.70, 0.70),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[ defaults ]
1 2

[ atomtypes ]
P4 72.0 0.000 A 0.0 0.0
SP4 54.0 0.000 A 0.0 0.0
TC1 36.0 0.000 A 0.0 0.0
W  72.0 0.000 A 0.0 0.0
N4a 72.0 0.000 A 0.0 0.0
SQ4p 54.0 0.000 A 0.0 0.0
C1 72.0 0.000 A 0.0 0.0
X2 72.0 0.000 A 0.0 0.0
D 72.0 0.000 A 0.0 0.0

[ nonbond_params ]
    P4    P4  1 4.700000e-01    4.250000e+00
   SP4   SP4  1 4.100000e-01    3.550000e+00
   TC1   TC1  1 3.400000e-01    3.000000e+00
    P4   SP4  1 4.400000e-01    3.900000e+00
";

    /// An atomistic (Amber-style) topology: has `[ atomtypes ]`, is not Martini.
    const ATOMISTIC: &str = "\
[ defaults ]
1 2 yes 0.5 0.8333

[ atomtypes ]
  C  6 12.01000  0.0000  A  3.39967e-01  3.59824e-01
 CT  6 12.01000  0.0000  A  3.39967e-01  4.57730e-01
 CA  6 12.01000  0.0000  A  3.39967e-01  3.59824e-01
 HC  1  1.00800  0.0000  A  2.64953e-01  6.56888e-02
 HA  1  1.00800  0.0000  A  2.59964e-01  6.27600e-02
  N  7 14.01000  0.0000  A  3.25000e-01  7.11280e-01
 NA  7 14.01000  0.0000  A  3.25000e-01  7.11280e-01
  O  8 16.00000  0.0000  A  2.95992e-01  8.78640e-01
 OH  8 16.00000  0.0000  A  3.06647e-01  8.80314e-01
 OW  8 16.00000  0.0000  A  3.15061e-01  6.36386e-01
 HW  1  1.00800  0.0000  A  0.00000e+00  0.00000e+00
  S 16 32.06000  0.0000  A  3.56359e-01  1.04600e+00
";

    #[test]
    fn parses_types_and_sigma() {
        let ff = MartiniForceField::parse(SAMPLE);
        assert!(ff.is_forcefield());
        assert!(ff.is_known("P4"));
        assert!(ff.is_known("W"));
        assert!(!ff.is_known("ZZ"));
        // radius = sigma / 2
        assert!((ff.radius_nm("P4").unwrap() - 0.235).abs() < 1e-4);
        assert!((ff.radius_nm("SP4").unwrap() - 0.205).abs() < 1e-4);
        assert!((ff.radius_nm("TC1").unwrap() - 0.170).abs() < 1e-4);
        // W declared in atomtypes but no self-pair -> regular-size fallback.
        assert!((ff.radius_nm("W").unwrap() - 0.235).abs() < 1e-4);
        assert!(ff.radius_nm("ZZ").is_none());
    }

    #[test]
    fn chemical_class_ignores_size_prefix() {
        assert_eq!(chem_class("P4"), 'P');
        assert_eq!(chem_class("SP4"), 'P');
        assert_eq!(chem_class("TC1"), 'C');
        assert_eq!(chem_class("SQ1"), 'Q');
        assert_eq!(chem_class("W"), 'W');
        assert_eq!(chem_class("SW"), 'W');
        assert_eq!(chem_class("SC1eq"), 'C');
        // A leading S/T with no class letter after is not a size prefix.
        assert_eq!(size_prefix("S"), None);
    }

    #[test]
    fn atomistic_forcefield_is_not_martini() {
        let ff = MartiniForceField::parse(ATOMISTIC);
        // The atom types parse fine — they just must not pass for Martini.
        assert!(!ff.types.is_empty());
        assert!(!ff.is_forcefield());
    }

    #[test]
    fn a_few_atom_types_are_not_a_forcefield() {
        let ff = MartiniForceField::parse(
            "[ atomtypes ]\nP4 72.0 0.000 A 0.0 0.0\nW 72.0 0.000 A 0.0 0.0\n",
        );
        assert!(!ff.is_forcefield());
    }

    #[test]
    fn bead_name_grammar_rejects_atomistic_names() {
        for bead in ["W", "P4", "SP1", "TC5", "N4a", "SQ4p", "D", "X2", "C1"] {
            assert!(looks_like_bead_type(bead), "{bead} should be a bead type");
        }
        for atom in ["CT", "CA", "HC", "HW", "NA", "OH", "OW", "S", "opls_001"] {
            assert!(!looks_like_bead_type(atom), "{atom} should not be a bead type");
        }
    }
}
