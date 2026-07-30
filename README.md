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
  generate selection strings for the `make_ndx` command. The group colouring
  has its own opacity, separate from the structure's.
* **Cross-validation** with `.gro` / `.pdb` structure files.
* **XTC trajectory playback** — play / step / seek, adjustable FPS, and
  frame interpolation (smoothing) between recorded frames.
* **Surface (dot) mesh view** from `gmx sasa` PDBs, plus multiple **overlay
  surfaces** loaded from separate files, each with its own colour.
* **Document layers** — load several structures at once to compare them; each
  layer has its own visibility, identity colour and opacity.
* **Martini coarse-grained bead view** — beads sized from the force field's LJ
  sigma and coloured by bead type, drawn through the main molecule pipeline.
* **Components** — named groups you can show/hide, split and merge from the
  command bar (see below). They start as one component per residue name.
* **Per-molecule / per-layer opacity**.
* **Atom selection** — pick atoms in the view, selector expressions (e.g.
  `aC1 | aC2`), shortest-path "select between", residue-name editing, and
  H-bond-aware selection; export the edited structure.
* **Loaded-files overview** so every file currently loaded is visible at a
  glance.

---

# Component Commands

Press **Ctrl+P** to focus the command bar at the bottom of the window, then type
`help` for this reference in-app. Components are a *display* grouping: splitting
and merging never touches your PDB / GRO / TOP files.

Every atom belongs to exactly one component, so assignment **moves** atoms —
which is what makes splitting and merging the same operation:

```
DOM1 = PROT and resid 1-100        # split: those atoms leave PROT for DOM1
PROT = PROA or PROB or PROC        # merge: the three sources empty out and vanish
```

| Command | Meaning |
| --- | --- |
| `NAME = <selection>` | move every matched atom into `NAME` (creating it if new) |
| `<selection>` | report how many atoms match, changing nothing |
| `show NAME...` / `hide NAME...` | show or hide components; `show all` / `hide all` for the lot |
| `only NAME` | show `NAME`, hide everything else |
| `del NAME...` | dissolve: the atoms fall back to their residue-name components |
| `rename OLD NEW`, `list`, `reset`, `help` | |

Selections:

| Term | Matches |
| --- | --- |
| `resname SOL NA` | residue names |
| `resid 1-100 205` | residue numbers; several numbers and ranges allowed |
| `index 1-4000` | atom numbers, 1-based, as in a `.ndx` file |
| `name CA C1'` / `element C O` | atom names / element symbols |
| `selected` | the atoms currently picked in the view |
| `all`, `none` | |
| `sp`, `sp2`, `sp3` | hybridisation, inferred from the number of bonds |
| `numbonds 4` | bond count; also `>=N`, `<=N`, `>N`, `<N`, `N-M` |
| `with 2 H` | exactly 2 bonded hydrogens; takes the same count forms |
| `and`, `or`, `not` | also `&`, `\|`, `!`; precedence is `not` > `and` > `or` |
| `( )`, `"NAME"` | grouping; quote a name that collides with a keyword |

```
CH2  = element C and sp3 and with 2 H          # methylene carbons
CH3  = element C and sp3 and with 3 H          # methyl carbons
QUAT = element C and numbonds 4 and with 0 H   # quaternary carbons
SITE = selected                                # freeze the current pick
```

Two things worth knowing:

* A bare word resolves in this order — **component, residue name, element, atom
  name** — and every resolution is echoed in the log. The order matters for real
  collisions: the DNA residue `C` (cytosine) wins over carbon, so write
  `element C` when you mean the element.
* `sp`/`sp2`/`sp3`, `numbonds` and `with` read the bond graph, which only comes
  from a `.top`, a PDB `CONECT` record or a MOL2 file. On a bare `.gro` they
  report that there is no topology rather than silently matching nothing.
  Hybridisation is inferred from coordination number, so an amide nitrogen
  (three neighbours) reads as `sp3`.

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
