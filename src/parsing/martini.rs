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
//! `(radius, colour)` for [`crate::martini_bead_render`].

use std::collections::HashMap;
use std::path::Path;

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
                    if let Some(sigma) = parts.nth(1).and_then(|s| s.parse::<f32>().ok()) {
                        if sigma > 0.0 {
                            sigma_nm.insert(i.to_string(), sigma);
                        }
                    }
                }
                Section::Other => {}
            }
        }

        Self { sigma_nm, types }
    }

    /// True when this looks like a Martini force field (has bead-type defs).
    pub fn is_forcefield(&self) -> bool {
        !self.types.is_empty() || !self.sigma_nm.is_empty()
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

[ nonbond_params ]
    P4    P4  1 4.700000e-01    4.250000e+00
   SP4   SP4  1 4.100000e-01    3.550000e+00
   TC1   TC1  1 3.400000e-01    3.000000e+00
    P4   SP4  1 4.400000e-01    3.900000e+00
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
}
