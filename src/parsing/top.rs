use crate::parsing::GroFile;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

// Gromacs TOP bonded length parameters are represented in nanometers.
#[allow(dead_code)]
pub const GROMACS_LENGTH_UNIT: &str = "nm";

// `[ molecules ]` counts are read verbatim from the file, so the expanded atom
// and bond totals are attacker-controlled. These caps sit far above any system
// a desktop viewer can usefully render; their only job is to turn an absurd
// count into a reported error instead of a multi-gigabyte allocation that
// aborts the process (or a billions-iteration loop that hangs it).
const MAX_EXPANDED_ATOMS: usize = 20_000_000;
const MAX_EXPANDED_BONDS: usize = 40_000_000;

#[derive(Debug, Clone)]
pub struct TopAtomRecord {
    pub nr: usize,
    pub atom_type: String,
    pub resi: i32,
    pub res: String,
    pub atom: String,
    pub cgnr: i32,
    pub charge: f32,
    pub mass: f32,
    pub comment: Option<String>,
}

impl TopAtomRecord {
    fn split_comment(line: &str) -> (&str, Option<String>) {
        if let Some((head, tail)) = line.split_once(';') {
            (head.trim_end(), Some(tail.trim().to_string()))
        } else {
            (line.trim_end(), None)
        }
    }

    pub fn from_line(line: &str) -> Option<Self> {
        let (data, comment) = Self::split_comment(line);
        let parts: Vec<&str> = data.split_whitespace().collect();
        if parts.len() < 8 {
            return None;
        }

        Some(Self {
            nr: parts[0].parse().ok()?,
            atom_type: parts[1].to_string(),
            resi: parts[2].parse().ok()?,
            res: parts[3].to_string(),
            atom: parts[4].to_string(),
            cgnr: parts[5].parse().ok()?,
            charge: parts[6].parse().ok()?,
            mass: parts[7].parse().ok()?,
            comment,
        })
    }

    pub fn to_line(&self) -> String {
        let mut line = format!(
            "{:>6} {:<8}{:>6} {:<5} {:<5}{:>6} {:>12.6} {:>11.5}",
            self.nr,
            self.atom_type,
            self.resi,
            self.res,
            self.atom,
            self.cgnr,
            self.charge,
            self.mass
        );
        if let Some(comment) = &self.comment
            && !comment.is_empty() {
                line.push_str(" ; ");
                line.push_str(comment);
            }
        line
    }

    pub fn set_res_name(&mut self, value: &str) {
        self.res = value.trim().to_string();
    }
}

#[derive(Debug, Clone)]
pub struct TopBondRecord {
    pub ai: usize,
    pub aj: usize,
    pub funct: i32,
    // Bond length in Gromacs TOP is nm when present.
    pub r: Option<f32>,
    pub k: Option<f32>,
    pub comment: Option<String>,
}

impl TopBondRecord {
    fn split_comment(line: &str) -> (&str, Option<String>) {
        if let Some((head, tail)) = line.split_once(';') {
            (head.trim_end(), Some(tail.trim().to_string()))
        } else {
            (line.trim_end(), None)
        }
    }

    pub fn from_line(line: &str) -> Option<Self> {
        let (data, comment) = Self::split_comment(line);
        let parts: Vec<&str> = data.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }

        Some(Self {
            ai: parts[0].parse().ok()?,
            aj: parts[1].parse().ok()?,
            funct: parts[2].parse().ok()?,
            r: parts.get(3).and_then(|value| value.parse().ok()),
            k: parts.get(4).and_then(|value| value.parse().ok()),
            comment,
        })
    }

    pub fn to_line(&self) -> String {
        let mut line = format!("{:>6}{:>7}{:>6}", self.ai, self.aj, self.funct);
        if let Some(r) = self.r {
            line.push_str(&format!("{:>13.4e}", r));
        }
        if let Some(k) = self.k {
            line.push_str(&format!("{:>13.4e}", k));
        }
        if let Some(comment) = &self.comment
            && !comment.is_empty() {
                line.push_str(" ; ");
                line.push_str(comment);
            }
        line
    }
}

#[derive(Debug, Clone)]
pub struct TopMolRecord {
    name: String,
    nmols: usize,
}

impl TopMolRecord {
    pub fn from_line(line: &str) -> Option<Self> {
        let data = line.split(';').next()?.trim();
        let parts: Vec<&str> = data.split_whitespace().collect();
        if parts.len() < 2 {
            return None;
        }

        Some(Self {
            name: parts[0].to_string(),
            nmols: parts[1].parse().ok()?,
        })
    }
    pub fn to_line(&self) -> String {
        format!("{:>6} {:>6} ; {}", self.name, self.nmols, "molecule count")
    }
}

#[derive(Debug, Clone)]
pub enum TopLine {
    SectionHeader(String),
    Atom(TopAtomRecord),
    Bond(TopBondRecord),
    Molecule(TopMolRecord),
    IntermolecularInteraction(TopBondRecord),
    Comment(String),
    Other(String),
    Empty,
}

#[derive(Debug, Clone, Default)]
pub struct TopFile {
    pub lines: Vec<TopLine>,
}

#[derive(Debug, Clone)]
struct MoleculeTemplate {
    name: String,
    atoms: Vec<TopAtomRecord>,
    bonds: Vec<TopBondRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopGroComparison {
    pub atom_count_match: bool,
    pub atom_order_match: bool,
    pub bond_count_match: bool,
    pub bond_connectivity_match: bool,
}

impl TopGroComparison {
    pub fn matches(&self) -> bool {
        self.atom_count_match
            && self.atom_order_match
            && self.bond_count_match
            && self.bond_connectivity_match
    }
}

impl TopFile {
    pub fn load(content: &str) -> Self {
        Self::parse(content)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let expanded = Self::expand_includes(path.as_ref())?;
        Ok(Self::parse(&expanded))
    }

    /// Read a TOP/ITP file and expand its `#include`s into a single string — the
    /// same preprocessing `load_from_path` applies before parsing. Exposed so
    /// callers can scan force-field sections (e.g. Martini `[ atomtypes ]`) that
    /// often live in an included file rather than the top-level one.
    pub fn expand_includes(path: &Path) -> Result<String, String> {
        let content = fs::read_to_string(path)
            .map_err(|err| format!("Failed to read TOP file {}: {}", path.display(), err))?;
        TopPreprocessor::default().expand(&content, Some(path))
    }

    fn parse(content: &str) -> Self {
        let mut lines = Vec::new();
        let mut current_section = String::new();
        let mut in_intermolecular_interactions = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                lines.push(TopLine::Empty);
                continue;
            }

            if trimmed.starts_with(';') {
                lines.push(TopLine::Comment(line.to_string()));
                continue;
            }

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                current_section = trimmed
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim()
                    .to_ascii_lowercase();
                if current_section == "intermolecular_interactions" {
                    in_intermolecular_interactions = true;
                }
                lines.push(TopLine::SectionHeader(line.to_string()));
                continue;
            }

            match current_section.as_str() {
                "molecules" => {
                    if let Some(mol) = TopMolRecord::from_line(line) {
                        lines.push(TopLine::Molecule(mol));
                    } else {
                        lines.push(TopLine::Other(line.to_string()));
                    }
                }
                "atoms" => {
                    if let Some(atom) = TopAtomRecord::from_line(line) {
                        lines.push(TopLine::Atom(atom));
                    } else {
                        lines.push(TopLine::Other(line.to_string()));
                    }
                }
                "bonds" if in_intermolecular_interactions => {
                    println!("Parsing intermolecular interaction line: {}", line);
                    if let Some(bond) = TopBondRecord::from_line(line) {
                        lines.push(TopLine::IntermolecularInteraction(bond));
                    } else {
                        lines.push(TopLine::Other(line.to_string()));
                    }
                }
                "bonds" => {
                    if let Some(bond) = TopBondRecord::from_line(line) {
                        lines.push(TopLine::Bond(bond));
                    } else {
                        lines.push(TopLine::Other(line.to_string()));
                    }
                }
                _ => lines.push(TopLine::Other(line.to_string())),
            }
        }

        Self { lines }
    }

    pub fn dump(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match line {
                TopLine::SectionHeader(text) | TopLine::Other(text) | TopLine::Comment(text) => {
                    out.push_str(text);
                    out.push('\n');
                }
                TopLine::Atom(atom) => {
                    out.push_str(&atom.to_line());
                    out.push('\n');
                }
                TopLine::Bond(bond) => {
                    out.push_str(&bond.to_line());
                    out.push('\n');
                }
                TopLine::Molecule(mol) => {
                    out.push_str(&mol.to_line());
                    out.push('\n');
                }
                TopLine::IntermolecularInteraction(bond) => {
                    out.push_str(&bond.to_line());
                    out.push('\n');
                }
                TopLine::Empty => out.push('\n'),
            }
        }
        out
    }

    pub fn atoms(&self) -> impl Iterator<Item = &TopAtomRecord> {
        self.lines.iter().filter_map(|line| match line {
            TopLine::Atom(atom) => Some(atom),
            _ => None,
        })
    }

    fn parse_section_name(text: &str) -> String {
        text.trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim()
            .to_ascii_lowercase()
    }

    fn parse_layout(&self) -> (Vec<MoleculeTemplate>, Vec<TopMolRecord>) {
        let mut templates = Vec::new();
        let mut instances = Vec::new();
        let mut current_section = String::new();
        let mut current_template: Option<MoleculeTemplate> = None;

        for line in &self.lines {
            match line {
                TopLine::SectionHeader(text) => {
                    let new_section = Self::parse_section_name(text);
                    // If we're starting a new moleculetype, finalize the previous template.
                    if new_section == "moleculetype" {
                        if let Some(template) = current_template.take() {
                            templates.push(template);
                        }
                        current_template = Some(MoleculeTemplate {
                            name: String::new(),
                            atoms: Vec::new(),
                            bonds: Vec::new(),
                        });
                    }
                    // Update current section for parsing subsequent lines (atoms/bonds/etc.)
                    current_section = new_section;
                }
                TopLine::Atom(atom) if current_section == "atoms" => {
                    if let Some(template) = current_template.as_mut() {
                        template.atoms.push(atom.clone());
                    }
                }
                TopLine::Bond(bond) if current_section == "bonds" => {
                    if let Some(template) = current_template.as_mut() {
                        template.bonds.push(bond.clone());
                    }
                }
                TopLine::Molecule(mol) => {
                    instances.push(mol.clone());
                }
                TopLine::Other(text) if current_section == "moleculetype" => {
                    if let Some(template) = current_template.as_mut() {
                        template.name = text
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string();
                    }
                }

                _ => {}
            }
        }

        if let Some(template) = current_template {
            templates.push(template);
        }

        (templates, instances)
    }

    pub fn generate_molecule_with_gro(
        &self,
        gro: &GroFile,
    ) -> Result<(moleucle_3dview_rs::molecule::Molecule, Vec<(usize, usize)>), String> {
        let (templates, instances) = self.parse_layout();

        // Size the expansion with checked arithmetic and reject anything past the
        // caps BEFORE allocating or entering the push loop below: `nmols` is
        // untrusted, so `bonds.len() * nmols` can overflow (a debug panic, a
        // release wrap into a bogus capacity) and the `for _ in 0..nmols` loops
        // can allocate/iterate until the process dies.
        let mut natoms_total: usize = 0;
        let mut nbond: usize = 0;
        for instance in &instances {
            let template = templates
                .iter()
                .find(|t| t.name == instance.name)
                .ok_or_else(|| {
                    format!(
                        "No molecule template found for instance '{}'",
                        instance.name
                    )
                })?;
            natoms_total = template
                .atoms
                .len()
                .checked_mul(instance.nmols)
                .and_then(|a| natoms_total.checked_add(a))
                .filter(|n| *n <= MAX_EXPANDED_ATOMS)
                .ok_or_else(|| {
                    format!(
                        "topology expands to more than {MAX_EXPANDED_ATOMS} atoms \
                         (check the [ molecules ] counts)"
                    )
                })?;
            nbond = template
                .bonds
                .len()
                .checked_mul(instance.nmols)
                .and_then(|b| nbond.checked_add(b))
                .filter(|n| *n <= MAX_EXPANDED_BONDS)
                .ok_or_else(|| {
                    format!(
                        "topology expands to more than {MAX_EXPANDED_BONDS} bonds \
                         (check the [ molecules ] counts)"
                    )
                })?;
        }

        let mut bonds = Vec::with_capacity(nbond);
        let mut offset = 0;
        for instance in &instances {
            let template = templates
                .iter()
                .find(|t| t.name == instance.name)
                .ok_or_else(|| {
                    format!(
                        "No molecule template found for instance '{}'",
                        instance.name
                    )
                })?;
            // An empty moleculetype (no atoms, no bonds) passes both caps above
            // because len()*nmols == 0, so a huge `nmols` would spin this loop up
            // to usize::MAX times doing nothing (offset += 0, nothing pushed) and
            // hang the app. The body is a no-op when both are empty, so skipping
            // is behaviour-preserving and bounds the loop (mirrors the guard in
            // expanded_atom_types).
            if template.atoms.is_empty() && template.bonds.is_empty() {
                continue;
            }
            for _ in 0..instance.nmols {
                for bond in &template.bonds {
                    // `bond.ai`/`bond.aj` are parsed verbatim from the file and can
                    // be up to usize::MAX; the caps above bound the atom/bond count
                    // but not these values. Saturate instead of `+` so a near-MAX
                    // index cannot overflow (a debug-build panic) — a saturated
                    // index is out of range and gets dropped by the downstream
                    // atoms.len() bounds-filter in to_molecule_with_metadata.
                    bonds.push(TopBondRecord {
                        ai: bond.ai.saturating_add(offset),
                        aj: bond.aj.saturating_add(offset),
                        funct: bond.funct,
                        r: bond.r,
                        k: bond.k,
                        comment: bond.comment.clone(),
                    });
                }
                offset += template.atoms.len();
            }
        }

        let molecule = gro.to_molecule_with_metadata(
            true,
            Some(&bonds.into_iter().map(|b| (b.ai, b.aj)).collect::<Vec<_>>()),
        );

        println!(
            "Intermolecular interactions found: {}",
            self.lines
                .iter()
                .filter(|line| matches!(line, TopLine::IntermolecularInteraction(_)))
                .count()
        );
        // GROMACS numbers atoms from 1; convert here so nothing downstream has
        // to remember which convention a pair is in. A serial of 0 is malformed
        // and is dropped rather than wrapping to usize::MAX.
        let intermolecular_pairs: Vec<(usize, usize)> = self
            .lines
            .iter()
            .filter_map(|line| match line {
                TopLine::IntermolecularInteraction(bond) => {
                    Some((bond.ai.checked_sub(1)?, bond.aj.checked_sub(1)?))
                }
                _ => None,
            })
            .collect();
        Ok((molecule, intermolecular_pairs))
    }

    pub fn atoms_mut(&mut self) -> impl Iterator<Item = &mut TopAtomRecord> {
        self.lines.iter_mut().filter_map(|line| match line {
            TopLine::Atom(atom) => Some(atom),
            _ => None,
        })
    }

    /// The `atom_type` of every atom, expanded across `[ molecules ]` instances
    /// in the same order the coordinates appear in the matching GRO — i.e. the
    /// order `generate_molecule_with_gro` produces atoms in. For Martini
    /// topologies this yields the bead type of each particle.
    ///
    /// Returns an empty vector when the file has no molecule templates/instances
    /// (e.g. a force-field-only `.itp`), in which case there is nothing to align.
    pub fn expanded_atom_types(&self) -> Vec<String> {
        let (templates, instances) = self.parse_layout();
        let mut out = Vec::new();
        for instance in &instances {
            let Some(template) = templates.iter().find(|t| t.name == instance.name) else {
                continue;
            };
            // Skip empty templates so a huge `nmols` cannot spin the inner loop
            // billions of times with nothing to append, and stop once the cap is
            // reached so an absurd count cannot grow `out` until the allocator
            // aborts. `nmols` is untrusted (read verbatim from `[ molecules ]`).
            if template.atoms.is_empty() {
                continue;
            }
            for _ in 0..instance.nmols {
                if out.len().saturating_add(template.atoms.len()) > MAX_EXPANDED_ATOMS {
                    return out;
                }
                out.extend(template.atoms.iter().map(|a| a.atom_type.clone()));
            }
        }
        out
    }
}

#[derive(Debug, Clone)]
/// State of one `#ifdef`/`#ifndef` block.
///
/// Conditional handling is deliberately disabled below: a viewer has no `-D`
/// flags to evaluate the conditions against, so every branch is taken and the
/// file is read as written. This type is the shape that support needs if it is
/// ever turned on, so it outlives the commented-out block that uses it.
#[allow(dead_code)]
struct ConditionalFrame {
    parent_active: bool,
    condition_true: bool,
    active: bool,
    else_used: bool,
}

#[derive(Debug, Clone, Default)]
struct TopPreprocessor {
    /// Symbols from `#define`. Unused while conditional handling is disabled --
    /// see [`ConditionalFrame`].
    #[allow(dead_code)]
    defines: HashSet<String>,
}


impl TopPreprocessor {
    fn expand(&mut self, content: &str, source_path: Option<&Path>) -> Result<String, String> {
        let mut output = String::new();
        let mut include_stack = Vec::new();
        let mut included_files: HashSet<PathBuf> = HashSet::new();
        let mut condition_stack = Vec::new();
        self.expand_into(
            content,
            source_path,
            &mut include_stack,
            &mut included_files,
            &mut condition_stack,
            &mut output,
        )?;
        Ok(output)
    }

    fn expand_into(
        &mut self,
        content: &str,
        source_path: Option<&Path>,
        include_stack: &mut Vec<PathBuf>,
        included_files: &mut HashSet<PathBuf>,
        condition_stack: &mut Vec<ConditionalFrame>,
        output: &mut String,
    ) -> Result<(), String> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                self.handle_directive(
                    trimmed,
                    source_path,
                    include_stack,
                    included_files,
                    condition_stack,
                    output,
                )?;
                continue;
            }

            if Self::is_active(condition_stack) {
                output.push_str(line);
                output.push('\n');
            }
        }

        Ok(())
    }

    fn handle_directive(
        &mut self,
        line: &str,
        source_path: Option<&Path>,
        include_stack: &mut Vec<PathBuf>,
        included_files: &mut HashSet<PathBuf>,
        condition_stack: &mut Vec<ConditionalFrame>,
        output: &mut String,
    ) -> Result<(), String> {
        if let Some(rest) = line.strip_prefix("#include") {
            if !Self::is_active(condition_stack) {
                return Ok(());
            }

            let include_target = rest.trim();
            let include_path = Self::resolve_include_path(include_target, source_path)?;
            let normalized = Self::normalize_path(&include_path);

            // If we've already included this file in this expansion, skip it (include-once).
            if included_files.contains(&normalized) {
                return Ok(());
            }

            if include_stack.contains(&normalized) {
                return Err(format!(
                    "Include cycle detected while expanding {}",
                    normalized.display()
                ));
            }

            let included = fs::read_to_string(&include_path).map_err(|err| {
                format!(
                    "Failed to read included file {}: {}",
                    include_path.display(),
                    err
                )
            })?;

            included_files.insert(normalized.clone());
            include_stack.push(normalized);
            let result = self.expand_into(
                &included,
                Some(&include_path),
                include_stack,
                included_files,
                condition_stack,
                output,
            );
            include_stack.pop();
            return result;
        }

        // if let Some(name) = line.strip_prefix("#ifdef") {
        //     let name = name
        //         .split_whitespace()
        //         .next()
        //         .ok_or_else(|| format!("Malformed #ifdef directive: {}", line))?;
        //     let parent_active = Self::is_active(condition_stack);
        //     let condition_true = self.defines.contains(name);
        //     condition_stack.push(ConditionalFrame {
        //         parent_active,
        //         condition_true,
        //         active: parent_active && condition_true,
        //         else_used: false,
        //     });
        //     return Ok(());
        // }

        // if let Some(name) = line.strip_prefix("#ifndef") {
        //     let name = name
        //         .split_whitespace()
        //         .next()
        //         .ok_or_else(|| format!("Malformed #ifndef directive: {}", line))?;
        //     let parent_active = Self::is_active(condition_stack);
        //     let condition_true = !self.defines.contains(name);
        //     condition_stack.push(ConditionalFrame {
        //         parent_active,
        //         condition_true,
        //         active: parent_active && condition_true,
        //         else_used: false,
        //     });
        //     return Ok(());
        // }

        // if line.starts_with("#else") {
        //     let Some(frame) = condition_stack.last_mut() else {
        //         return Err("#else without matching #if block".to_string());
        //     };
        //     if frame.else_used {
        //         return Err("Duplicate #else in conditional block".to_string());
        //     }
        //     frame.else_used = true;
        //     frame.active = frame.parent_active && !frame.condition_true;
        //     return Ok(());
        // }

        // if line.starts_with("#endif") {
        //     if condition_stack.pop().is_none() {
        //         return Err("#endif without matching #if block".to_string());
        //     }
        //     return Ok(());
        // }

        // if let Some(name) = line.strip_prefix("#define") {
        //     if Self::is_active(condition_stack) {
        //         if let Some(symbol) = name.split_whitespace().next() {
        //             self.defines.insert(symbol.to_string());
        //         }
        //     }
        //     return Ok(());
        // }

        // if let Some(name) = line.strip_prefix("#undef") {
        //     if Self::is_active(condition_stack) {
        //         if let Some(symbol) = name.split_whitespace().next() {
        //             self.defines.remove(symbol);
        //         }
        //     }
        //     return Ok(());
        // }

        Ok(())
    }

    fn resolve_include_path(
        include_target: &str,
        source_path: Option<&Path>,
    ) -> Result<PathBuf, String> {
        let trimmed = include_target.trim();
        let raw_path = if let Some(rest) = trimmed.strip_prefix('"') {
            rest.split_once('"')
                .map(|(path, _)| path)
                .ok_or_else(|| format!("Malformed #include directive: {}", include_target))?
        } else if let Some(rest) = trimmed.strip_prefix('<') {
            rest.split_once('>')
                .map(|(path, _)| path)
                .ok_or_else(|| format!("Malformed #include directive: {}", include_target))?
        } else {
            trimmed
                .split_whitespace()
                .next()
                .ok_or_else(|| format!("Malformed #include directive: {}", include_target))?
        };

        let include_path = Path::new(raw_path);
        if include_path.is_absolute() {
            return Ok(include_path.to_path_buf());
        }

        let Some(source_path) = source_path else {
            return Ok(include_path.to_path_buf());
        };

        let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
        Ok(base_dir.join(include_path))
    }

    fn normalize_path(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn is_active(condition_stack: &[ConditionalFrame]) -> bool {
        condition_stack.iter().all(|frame| frame.active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::GroFile;

    /// One water-like moleculetype (2 atoms, 1 bond) instantiated `nmols` times.
    fn tiny_top(nmols: &str) -> TopFile {
        TopFile::load(&format!(
            "[ moleculetype ]\nSOL 3\n\n\
             [ atoms ]\n\
             1 OW 1 SOL OW 1 0.0 16.0\n\
             2 HW 1 SOL HW1 1 0.0 1.0\n\n\
             [ bonds ]\n1 2 1\n\n\
             [ molecules ]\nSOL {nmols}\n"
        ))
    }

    #[test]
    fn absurd_molecule_count_is_rejected_not_expanded() {
        // A [ molecules ] count this large used to reach Vec::with_capacity and a
        // push loop that allocated until the process aborted. It must now be a
        // reported error instead.
        let top = tiny_top("999999999999");
        let err = top
            .generate_molecule_with_gro(&GroFile::default())
            .expect_err("an absurd molecule count must be rejected");
        assert!(
            err.contains("atoms") || err.contains("bonds"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn overflowing_molecule_count_does_not_panic() {
        // bonds.len() * usize::MAX overflows; checked arithmetic must turn that
        // into an error rather than a debug-build panic.
        let top = tiny_top(&usize::MAX.to_string());
        assert!(
            top.generate_molecule_with_gro(&GroFile::default())
                .is_err()
        );
    }

    #[test]
    fn near_max_bond_index_does_not_overflow_on_second_copy() {
        // A bond index at usize::MAX plus a non-zero offset (any molecule copy
        // after the first) used to hit `bond.ai + offset` and panic with
        // 'attempt to add with overflow' in a debug build. Saturating the add
        // must turn that into a dropped out-of-range bond, not a crash.
        let top = TopFile::load(&format!(
            "[ moleculetype ]\nSOL 3\n\n\
             [ atoms ]\n\
             1 OW 1 SOL OW 1 0.0 16.0\n\n\
             [ bonds ]\n{} 1 1\n\n\
             [ molecules ]\nSOL 2\n",
            usize::MAX
        ));
        // Must not panic; a mismatched GRO simply yields a molecule with the
        // out-of-range bond filtered out downstream.
        assert!(
            top.generate_molecule_with_gro(&GroFile::default())
                .is_ok()
        );
    }

    #[test]
    fn empty_moleculetype_with_huge_count_does_not_hang() {
        // An empty moleculetype (a name line but no atoms and no bonds) makes
        // both expansion caps pass (0 * nmols == 0), so the push loop used to
        // iterate up to usize::MAX times with a no-op body and hang the app. The
        // guard must skip the empty template and return promptly.
        let top = TopFile::load(
            "[ moleculetype ]\nEMPTY 1\n\n[ molecules ]\nEMPTY 999999999999999\n",
        );
        // Must finish (no hang) and not panic; an empty template contributes no
        // atoms/bonds, so this simply builds an empty-ish molecule.
        assert!(
            top.generate_molecule_with_gro(&GroFile::default())
                .is_ok()
        );
    }

    #[test]
    fn well_formed_topology_still_expands() {
        // Two copies of a two-atom molecule expand to four atom types, and the
        // molecule builds without error — the guard leaves normal files untouched.
        let top = tiny_top("2");
        assert!(
            top.generate_molecule_with_gro(&GroFile::default())
                .is_ok()
        );
        assert_eq!(top.expanded_atom_types().len(), 4);
    }
}
