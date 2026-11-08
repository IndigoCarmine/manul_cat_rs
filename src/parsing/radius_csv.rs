//! User-supplied particle radii for coarse-grained rendering.
//!
//! A CG model's particles carry whatever names its author chose, so neither
//! the element table nor a Martini force field can size them. This reads a
//! plain two-column table mapping a particle name to its rendered sphere
//! radius in nanometers:
//!
//! ```text
//! name,radius
//! BB,0.25
//! SC1,0.21
//! W,0.235
//! ```
//!
//! - Columns may be separated by a comma, semicolon, tab or spaces.
//! - Blank lines and lines starting with `#` are ignored.
//! - The first line is treated as a header when its radius column is not a
//!   number, so a spreadsheet export with column titles loads as-is.
//! - Names are matched exactly (case-sensitive), against an atom's name first
//!   and its topology atom type second; a later row for the same name wins.

use std::collections::HashMap;

/// Cap on the number of rows, far above any CG model's particle vocabulary.
/// The file is user-chosen, so this only stops a mistaken pick (a huge data
/// CSV) from building an enormous table.
const MAX_ENTRIES: usize = 100_000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParticleRadii {
    radii: HashMap<String, f32>,
}

impl ParticleRadii {
    pub fn parse(input: &str) -> Result<Self, String> {
        let mut radii = HashMap::new();
        let mut seen_data = false;

        for (index, raw) in input.lines().enumerate() {
            let line_no = index + 1;
            // A BOM survives `lines()` on files saved by spreadsheet programs.
            let line = raw.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let mut fields = line
                .split(|c: char| c == ',' || c == ';' || c == '\t' || c.is_whitespace())
                .map(|field| field.trim().trim_matches('"'))
                .filter(|field| !field.is_empty());
            let (Some(name), Some(radius_text)) = (fields.next(), fields.next()) else {
                return Err(format!(
                    "line {line_no}: expected `name,radius`, got `{line}`"
                ));
            };

            let radius = match radius_text.parse::<f32>() {
                Ok(radius) => radius,
                // Only the first meaningful line may be a header.
                Err(_) if !seen_data => {
                    seen_data = true;
                    continue;
                }
                Err(_) => {
                    return Err(format!(
                        "line {line_no}: radius `{radius_text}` is not a number"
                    ));
                }
            };
            seen_data = true;

            if !radius.is_finite() || radius <= 0.0 {
                return Err(format!(
                    "line {line_no}: radius for `{name}` must be a positive number of nm, got {radius_text}"
                ));
            }

            radii.insert(name.to_string(), radius);
            if radii.len() > MAX_ENTRIES {
                return Err(format!("more than {MAX_ENTRIES} particle names"));
            }
        }

        if radii.is_empty() {
            return Err("no `name,radius` rows found".to_string());
        }
        Ok(Self { radii })
    }

    /// Radius (nm) for a particle name, if the table lists it.
    pub fn radius_nm(&self, name: &str) -> Option<f32> {
        self.radii.get(name).copied()
    }

    /// Number of distinct particle names in the table.
    pub fn len(&self) -> usize {
        self.radii.len()
    }

    pub fn is_empty(&self) -> bool {
        self.radii.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rows_and_skips_a_header() {
        let radii = ParticleRadii::parse("name,radius\nBB,0.25\nSC1,0.21\n").unwrap();
        assert_eq!(radii.len(), 2);
        assert_eq!(radii.radius_nm("BB"), Some(0.25));
        assert_eq!(radii.radius_nm("SC1"), Some(0.21));
        assert_eq!(radii.radius_nm("name"), None);
    }

    #[test]
    fn accepts_other_separators_comments_bom_and_quotes() {
        let input = "\u{feff}# CG radii\n\"P1\";0.3\nP2\t0.4\n\nP3   0.5\n";
        let radii = ParticleRadii::parse(input).unwrap();
        assert_eq!(radii.radius_nm("P1"), Some(0.3));
        assert_eq!(radii.radius_nm("P2"), Some(0.4));
        assert_eq!(radii.radius_nm("P3"), Some(0.5));
    }

    #[test]
    fn names_are_case_sensitive_and_the_last_row_wins() {
        let radii = ParticleRadii::parse("A,0.1\na,0.2\nA,0.3\n").unwrap();
        assert_eq!(radii.radius_nm("A"), Some(0.3));
        assert_eq!(radii.radius_nm("a"), Some(0.2));
    }

    #[test]
    fn a_non_numeric_radius_after_the_first_row_is_an_error() {
        let err = ParticleRadii::parse("A,0.1\nB,big\n").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn non_positive_or_missing_radii_are_errors() {
        assert!(ParticleRadii::parse("A,0\n").is_err());
        assert!(ParticleRadii::parse("A,-1\n").is_err());
        assert!(ParticleRadii::parse("A,NaN\n").is_err());
        assert!(ParticleRadii::parse("A\n").is_err());
    }

    #[test]
    fn a_file_with_only_a_header_is_an_error() {
        assert!(ParticleRadii::parse("name,radius\n").is_err());
        assert!(ParticleRadii::parse("").is_err());
    }
}
