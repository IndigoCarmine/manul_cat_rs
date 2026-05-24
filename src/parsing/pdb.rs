use crate::view_rs::To3dViewMolecule;
use lin_alg::f32::Vec3;
use moleucle_3dview_rs::{
    molecule::{Atom, Bond},
    Molecule,
};
use std::collections::HashMap;
use std::fmt::Write;

use super::AtomRecord;

const ANGSTROM_TO_NM: f32 = 0.1;
const NM_TO_ANGSTROM: f32 = 10.0;

// --- PDB Structures ---

#[derive(Debug, Clone)]
pub struct ConectRecord {
    pub serial: usize,
    pub bonded: Vec<usize>,
}

impl ConectRecord {
    pub fn from_line(line: &str) -> Option<Self> {
        let serial = line.get(6..11)?.trim().parse().ok()?;
        let mut bonded = Vec::new();

        for range in [(11, 16), (16, 21), (21, 26), (26, 31)] {
            if let Some(s) = line.get(range.0..range.1) {
                if let Ok(v) = s.trim().parse::<usize>() {
                    bonded.push(v);
                }
            }
        }

        Some(ConectRecord { serial, bonded })
    }

    pub fn to_line(&self) -> String {
        let mut s = format!("CONECT{:5}", self.serial);
        for b in &self.bonded {
            write!(s, "{:5}", b).unwrap();
        }
        s
    }
}

#[derive(Debug, Clone)]
pub enum PdbLine {
    Atom(AtomRecord),
    Conect(ConectRecord),
    Other(String),
}

#[derive(Debug, Clone, Default)]
pub struct PdbFile {
    pub lines: Vec<PdbLine>,
}

impl PdbFile {
    pub fn load(content: &str) -> Self {
        let mut lines = Vec::new();
        for line in content.lines() {
            if line.starts_with("ATOM") || line.starts_with("HETATM") {
                if let Some(atom) = AtomRecord::from_line(line) {
                    lines.push(PdbLine::Atom(atom));
                    continue;
                }
            } else if line.starts_with("CONECT") {
                if let Some(conect) = ConectRecord::from_line(line) {
                    lines.push(PdbLine::Conect(conect));
                    continue;
                }
            }
            lines.push(PdbLine::Other(line.to_string()));
        }
        Self { lines }
    }

    pub fn dump(&mut self) -> String {
        self.update_resseq();
        let mut out = String::new();
        for line in &self.lines {
            match line {
                PdbLine::Atom(a) => writeln!(out, "{}", a.to_line()).unwrap(),
                PdbLine::Conect(c) => writeln!(out, "{}", c.to_line()).unwrap(),
                PdbLine::Other(s) => writeln!(out, "{}", s).unwrap(),
            }
        }
        out
    }

    pub fn atoms(&self) -> impl Iterator<Item = &AtomRecord> {
        self.lines.iter().filter_map(|l| match l {
            PdbLine::Atom(a) => Some(a),
            _ => None,
        })
    }

    pub fn atoms_mut(&mut self) -> impl Iterator<Item = &mut AtomRecord> {
        self.lines.iter_mut().filter_map(|l| match l {
            PdbLine::Atom(a) => Some(a),
            _ => None,
        })
    }

    pub fn update_resseq(&mut self) {
        let mut resnames: Vec<String> = self
            .atoms()
            .map(|a| a.res_name.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        resnames.sort();

        let resseq_map: HashMap<String, i32> = resnames
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, (i + 1) as i32))
            .collect();

        for atom in self.atoms_mut() {
            if let Some(&new_seq) = resseq_map.get(&atom.res_name) {
                atom.res_seq = new_seq;
            }
        }
    }

    pub fn find_connected_hydrogen(&self, atom: &AtomRecord) -> Vec<AtomRecord> {
        let mut connected_serials = std::collections::HashSet::new();
        let target_serial = atom.serial;

        for line in &self.lines {
            if let PdbLine::Conect(conect) = line {
                if conect.serial == target_serial {
                    for serial in &conect.bonded {
                        connected_serials.insert(*serial);
                    }
                } else if conect.bonded.contains(&target_serial) {
                    connected_serials.insert(conect.serial);
                }
            }
        }

        let mut hydrogens = Vec::new();
        for atom_rec in self.atoms() {
            if connected_serials.contains(&atom_rec.serial) && atom_rec.element == "H" {
                hydrogens.push(atom_rec.clone());
            }
        }
        hydrogens
    }

    pub fn from_molecule(molecule: &Molecule) -> Self {
        let mut lines: Vec<PdbLine> = Vec::new();
        lines.reserve(molecule.atoms.len() + molecule.bonds.len());

        for (idx, atom) in molecule.atoms.iter().enumerate() {
            let serial = idx + 1;
            let name = atom
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| atom.element.clone());

            let res_name = atom
                .res_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "LIG".to_string());

            let element = atom
                .element
                .trim()
                .chars()
                .take(2)
                .collect::<String>()
                .to_uppercase();

            lines.push(PdbLine::Atom(AtomRecord {
                serial,
                name,
                alt_loc: ' ',
                res_name,
                chain_id: atom.chain_id.unwrap_or('A'),
                res_seq: atom.res_seq.unwrap_or(1),
                i_code: ' ',
                x: atom.position.x * NM_TO_ANGSTROM,
                y: atom.position.y * NM_TO_ANGSTROM,
                z: atom.position.z * NM_TO_ANGSTROM,
                occupancy: atom.occupancy.unwrap_or(1.0),
                temp_factor: atom.temp_factor.unwrap_or(0.0),
                element,
                charge: atom.charge.clone().unwrap_or_default(),
            }));
        }

        let mut bonded_by_atom: HashMap<usize, Vec<usize>> = HashMap::new();
        for bond in &molecule.bonds {
            let a = bond.atom_a + 1;
            let b = bond.atom_b + 1;
            bonded_by_atom.entry(a).or_default().push(b);
            bonded_by_atom.entry(b).or_default().push(a);
        }

        let mut serials: Vec<usize> = bonded_by_atom.keys().copied().collect();
        serials.sort_unstable();
        for serial in serials {
            if let Some(mut bonded) = bonded_by_atom.remove(&serial) {
                bonded.sort_unstable();
                bonded.dedup();
                lines.push(PdbLine::Conect(ConectRecord { serial, bonded }));
            }
        }

        Self { lines }
    }
}

impl To3dViewMolecule for PdbFile {
    fn to_molecule(&self) -> Molecule {
        let mut atoms = Vec::new();
        let mut bonds = Vec::new();

        let mut serial_to_index = HashMap::new();

        for (i, record) in self.atoms().enumerate() {
            serial_to_index.insert(record.serial, i);
            atoms.push(Atom {
                position: Vec3::new(
                    record.x * ANGSTROM_TO_NM,
                    record.y * ANGSTROM_TO_NM,
                    record.z * ANGSTROM_TO_NM,
                ),
                element: record.element.clone(),
                id: i,
                name: Some(record.name.clone()),
                res_name: Some(record.res_name.clone()),
                chain_id: Some(record.chain_id),
                res_seq: Some(record.res_seq),
                occupancy: Some(record.occupancy),
                temp_factor: Some(record.temp_factor),
                charge: Some(record.charge.clone()),
            });
        }

        for line in &self.lines {
            if let PdbLine::Conect(c) = line {
                if let Some(&idx_a) = serial_to_index.get(&c.serial) {
                    for &bonded_serial in &c.bonded {
                        if let Some(&idx_b) = serial_to_index.get(&bonded_serial) {
                            if idx_a < idx_b {
                                bonds.push(Bond {
                                    atom_a: idx_a,
                                    atom_b: idx_b,
                                    order: 1,
                                });
                            }
                        }
                    }
                }
            }
        }

        Molecule { atoms, bonds }
    }
}