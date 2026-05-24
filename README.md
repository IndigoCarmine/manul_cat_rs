[Japanese README 日本語あります](README_ja.md)

# Overview

This application is a GUI-based tool for visualizing and validating GROMACS topology and index files.
Its primary purpose is to facilitate visual inspection of intermolecular interaction settings defined in `.top` files, enabling users to identify configuration errors and inconsistencies more efficiently.

Supported file formats:

* `.gro` : coordinate/structure files
* `.pdb` : coordinate/structure files

and

* `.top` / `.itp` : topology files
* `.ndx` : index files

---

# Main Features

* Loading GROMACS topology (`.top` / `.itp`) files
* Visualization of intermolecular interactions
* Loading index (`.ndx`) files
* Inspection of index groups
* Cross-validation with `.gro` structure files
* Generation of selection strings for the `make_ndx` command

---

# Intended Use Cases

This application is designed for the following purposes:

* Verification of intermolecular interaction definitions
* Detection of topology configuration errors
* Validation of index group consistency
* Pre-simulation inspection of GROMACS input files

---

# Screenshots

(Add screenshots here)

---

# License

MIT License

---

# Author

Yuhei Yamada (Indigo Carmine)
ORCID: 0009-0003-9780-4135
[https://orcid.org/0009-0003-9780-4135](https://orcid.org/0009-0003-9780-4135)
