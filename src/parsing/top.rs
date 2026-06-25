use crate::parsing::GroFile;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

// Gromacs TOP bonded length parameters are represented in nanometers.
#[allow(dead_code)]
pub const GROMACS_LENGTH_UNIT: &str = "nm";

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
        if let Some(comment) = &self.comment {
            if !comment.is_empty() {
                line.push_str(" ; ");
                line.push_str(comment);
            }
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
        if let Some(comment) = &self.comment {
            if !comment.is_empty() {
                line.push_str(" ; ");
                line.push_str(comment);
            }
        }
        line
    }
}

#[derive(Debug, Clone)]
struct TopMolRecord {
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
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .map_err(|err| format!("Failed to read TOP file {}: {}", path.display(), err))?;
        let expanded = TopPreprocessor::default().expand(&content, Some(path))?;
        Ok(Self::parse(&expanded))
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
                            .trim()
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

        let mut nbond = 0;
        for instances in &instances {
            let template = templates
                .iter()
                .find(|t| t.name == instances.name)
                .ok_or_else(|| {
                    format!(
                        "No molecule template found for instance '{}'",
                        instances.name
                    )
                })?;
            nbond += template.bonds.len() * instances.nmols;
        }

        let mut bonds = Vec::with_capacity(nbond);
        let mut offset = 0;
        for i in 0..instances.len() {
            let instance = &instances[i];
            let template = templates
                .iter()
                .find(|t| t.name == instance.name)
                .ok_or_else(|| {
                    format!(
                        "No molecule template found for instance '{}'",
                        instance.name
                    )
                })?;
            for _ in 0..instance.nmols {
                for bond in &template.bonds {
                    bonds.push(TopBondRecord {
                        ai: bond.ai + offset,
                        aj: bond.aj + offset,
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
        let intermolecular_pairs: Vec<(usize, usize)> = self
            .lines
            .iter()
            .filter_map(|line| match line {
                TopLine::IntermolecularInteraction(bond) => Some((bond.ai, bond.aj)),
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
}

#[derive(Debug, Clone)]
struct ConditionalFrame {
    parent_active: bool,
    condition_true: bool,
    active: bool,
    else_used: bool,
}

#[derive(Debug, Clone)]
struct TopPreprocessor {
    defines: HashSet<String>,
}

impl Default for TopPreprocessor {
    fn default() -> Self {
        // By default no conditional symbols are defined. Calling code may
        // choose to enable symbols (like INTER) explicitly when desired.
        Self {
            defines: HashSet::new(),
        }
    }
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
