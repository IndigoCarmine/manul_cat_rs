[Japanese README 日本語あります](README_ja.md)

# Overview

This application is a GUI-based tool for visualizing and validating GROMACS
structures, topologies, index files, and trajectories.
Its original purpose is to facilitate visual inspection of the intermolecular
interaction settings defined in `.top` files — enabling users to identify
configuration errors and inconsistencies more efficiently — and it has grown
into a general viewer for MD structures, coarse-grained systems, surfaces and
trajectories.

Supported file formats:

* `.gro` : coordinate / structure files
* `.pdb` : coordinate / structure files (including `gmx sasa` dot surfaces)
* `.mol2` : coordinate / structure files

and

* `.top` / `.itp` : topology files (GROMACS `#include` expansion supported)
* `.ndx` : index files
* `.xtc` : trajectory files

---

# Main Features

* **3D molecular viewer** — ball + stick / ball / wireframe / circles render
  styles, with drag-and-drop file loading.
* **GROMACS topology** (`.top` / `.itp`) loading and **visualization of
  intermolecular interactions**.
* **Index (`.ndx`) files** — inspect groups, toggle group visibility, and
  generate selection strings for the `make_ndx` command.
* **Cross-validation** with `.gro` / `.pdb` structure files.
* **XTC trajectory playback** — play / step / seek, adjustable FPS, and
  frame interpolation (smoothing) between recorded frames.
* **Surface (dot) mesh view** from `gmx sasa` PDBs, plus multiple **overlay
  surfaces** loaded from separate files, each with its own colour.
* **Document layers** — load several structures at once to compare them; each
  layer has its own visibility, identity colour and opacity.
* **Martini coarse-grained bead view** — beads sized from the force field's LJ
  sigma and coloured by bead type, drawn through the main molecule pipeline.
* **Per-residue show/hide** and **per-molecule / per-layer opacity**.
* **Atom selection** — pick atoms in the view, selector expressions (e.g.
  `aC1 | aC2`), shortest-path "select between", residue-name editing, and
  H-bond-aware selection; export the edited structure.
* **Loaded-files overview** so every file currently loaded is visible at a
  glance.

---

# Intended Use Cases

This application is designed for the following purposes:

* Verification of intermolecular interaction definitions
* Detection of topology configuration errors
* Validation of index group consistency
* Pre-simulation inspection of GROMACS input files
* Visual comparison of structures, surfaces and coarse-grained systems
* Inspection of MD trajectories

---

# Screenshots

(Add screenshots here)

---

# License

GNU Affero General Public License v3.0 or later (AGPL-3.0-or-later).
See [LICENSE](LICENSE) for the full text.

---

# Author

Yuhei Yamada (Indigo Carmine)
ORCID: 0009-0003-9780-4135
[https://orcid.org/0009-0003-9780-4135](https://orcid.org/0009-0003-9780-4135)
