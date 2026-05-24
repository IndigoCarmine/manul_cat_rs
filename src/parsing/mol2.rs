use crate::view_rs::To3dViewMolecule;
use lin_alg::f32::Vec3;
use moleucle_3dview_rs::{
    molecule::{Atom, Bond},
    Molecule,
};
use std::collections::HashMap;
use std::fmt::Write;

const ANGSTROM_TO_NM: f32 = 0.1;

// --- Mol2 Structures ---

#[derive(Debug, Clone)]
pub struct Mol2AtomRecord {
    pub atom_id: usize,
    pub atom_name: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub atom_type: String,
    pub subst_id: i32,
    pub subst_name: String,
    pub charge: f32,
    pub status_bit: String,
}

impl Mol2AtomRecord {
    pub fn from_line(line: &str) -> Option<Self> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            return None;
        }

        Some(Mol2AtomRecord {
            atom_id: parts[0].parse().ok()?,
            atom_name: parts[1].to_string(),
            x: parts[2].parse().ok()?,
            y: parts[3].parse().ok()?,
            z: parts[4].parse().ok()?,
            atom_type: parts[5].to_string(),
            subst_id: parts[6].parse().ok()?,
            subst_name: parts[7].to_string(),
            charge: parts[8].parse().ok()?,
            status_bit: parts.get(9).unwrap_or(&"").to_string(),
        })
    }

    pub fn to_line(&self) -> String {
        format!(
            "{:>7} {:<8}{:>10.4}{:>10.4}{:>10.4} {:<5}{:>6}     {:<4}{:>10.4}",
            self.atom_id,
            self.atom_name,
            self.x,
            self.y,
            self.z,
            self.atom_type,
            self.subst_id,
            self.subst_name,
            self.charge
        )
    }
}

#[derive(Debug, Clone)]
pub struct Mol2BondRecord {
    pub bond_id: usize,
    pub origin_atom_id: usize,
    pub target_atom_id: usize,
    pub bond_type: String,
    pub status_bit: String,
}

impl Mol2BondRecord {
    pub fn from_line(line: &str) -> Option<Self> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            return None;
        }

        Some(Mol2BondRecord {
            bond_id: parts[0].parse().ok()?,
            origin_atom_id: parts[1].parse().ok()?,
            target_atom_id: parts[2].parse().ok()?,
            bond_type: parts[3].to_string(),
            status_bit: parts.get(4).unwrap_or(&"").to_string(),
        })
    }

    pub fn to_line(&self) -> String {
        format!(
            "{:>6}{:>6}{:>6}{:>6}",
            self.bond_id, self.origin_atom_id, self.target_atom_id, self.bond_type
        )
    }
}

#[derive(Debug, Clone)]
pub enum Mol2Line {
    Atom(Mol2AtomRecord),
    Bond(Mol2BondRecord),
    SectionHeader(String),
    MoleculeLine(String),
    Other(String),
    Empty,
}

#[derive(Debug, Clone, Default)]
pub struct Mol2File {
    pub lines: Vec<Mol2Line>,
}

impl Mol2File {
    pub fn load(content: &str) -> Self {
        let mut lines = Vec::new();
        let mut current_section = "";

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                lines.push(Mol2Line::Empty);
                continue;
            }

            if line.starts_with("@<TRIPOS>") {
                current_section = &line[9..];
                lines.push(Mol2Line::SectionHeader(line.to_string()));
                continue;
            }

            match current_section {
                "MOLECULE" => lines.push(Mol2Line::MoleculeLine(line.to_string())),
                "ATOM" => {
                    if let Some(atom) = Mol2AtomRecord::from_line(line) {
                        lines.push(Mol2Line::Atom(atom));
                    } else {
                        lines.push(Mol2Line::Other(line.to_string()));
                    }
                }
                "BOND" => {
                    if let Some(bond) = Mol2BondRecord::from_line(line) {
                        lines.push(Mol2Line::Bond(bond));
                    } else {
                        lines.push(Mol2Line::Other(line.to_string()));
                    }
                }
                _ => lines.push(Mol2Line::Other(line.to_string())),
            }
        }
        Self { lines }
    }

    pub fn dump(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match line {
                Mol2Line::Atom(a) => writeln!(out, "{}", a.to_line()).unwrap(),
                Mol2Line::Bond(b) => writeln!(out, "{}", b.to_line()).unwrap(),
                Mol2Line::SectionHeader(s) | Mol2Line::MoleculeLine(s) | Mol2Line::Other(s) => {
                    writeln!(out, "{}", s).unwrap()
                }
                Mol2Line::Empty => writeln!(out).unwrap(),
            }
        }
        out
    }

    pub fn atoms(&self) -> impl Iterator<Item = &Mol2AtomRecord> {
        self.lines.iter().filter_map(|l| match l {
            Mol2Line::Atom(a) => Some(a),
            _ => None,
        })
    }

    pub fn bonds(&self) -> impl Iterator<Item = &Mol2BondRecord> {
        self.lines.iter().filter_map(|l| match l {
            Mol2Line::Bond(b) => Some(b),
            _ => None,
        })
    }
}

impl To3dViewMolecule for Mol2File {
    fn to_molecule(&self) -> Molecule {
        let mut atoms = Vec::new();
        let mut bonds = Vec::new();

        let mut id_to_index = HashMap::new();

        for (i, record) in self.atoms().enumerate() {
            id_to_index.insert(record.atom_id, i);
            atoms.push(Atom {
                position: Vec3::new(
                    record.x * ANGSTROM_TO_NM,
                    record.y * ANGSTROM_TO_NM,
                    record.z * ANGSTROM_TO_NM,
                ),
                element: record
                    .atom_type
                    .split('.')
                    .next()
                    .unwrap_or("?")
                    .to_uppercase(),
                id: i,
                name: None,
                res_name: None,
                chain_id: None,
                res_seq: None,
                occupancy: None,
                temp_factor: None,
                charge: None,
            });
        }

        for record in self.bonds() {
            if let (Some(&idx_a), Some(&idx_b)) = (
                id_to_index.get(&record.origin_atom_id),
                id_to_index.get(&record.target_atom_id),
            ) {
                let order = match record.bond_type.as_str() {
                    "2" => 2,
                    "3" => 3,
                    _ => 1,
                };
                bonds.push(Bond {
                    atom_a: idx_a,
                    atom_b: idx_b,
                    order,
                });
            }
        }

        Molecule { atoms, bonds }
    }
}