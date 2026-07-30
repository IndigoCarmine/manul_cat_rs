//! The component command language: `NAME = expr` plus a handful of verbs.
//!
//! Split into a tokenizer, a recursive-descent parser and an evaluator, none of
//! which know about `egui` or `KuromameApp`, so the whole language is testable
//! with `cargo test` and no window.
//!
//! ```text
//! DOM1 = PROT and resid 1-100                  // split
//! PROT = PROA or PROB                          // merge
//! CH2  = element C and sp3 and with 2 H        // methylene carbons
//! only DOM1
//! ```

pub mod ast;
pub mod eval;
pub mod lexer;
pub mod parser;

pub use ast::{CountSpec, Expr, Hybrid, NumRange, ParseError, Span, Statement, Targets};
pub use eval::{AtomTable, EvalCtx, EvalError, IdentKind, evaluate, to_indices};
pub use parser::parse_statement;

/// The text behind the `help` command and the README syntax table.
pub const HELP_TEXT: &str = "\
components — split and merge the COMPONENTS list (display only; files are never changed)

  NAME = <selection>    move every matched atom into NAME (creates it if new)
  <selection>           just report how many atoms match

  show NAME...          hide NAME...        show/hide all
  only NAME             show NAME, hide everything else
  del NAME...           dissolve: atoms fall back to their residue-name component
  rename OLD NEW        list                reset (back to one component per residue)

selections
  resname SOL NA        residue names            resid 1-100 205    residue numbers
  index 1-4000          atom numbers (1-based)   name CA C1'        atom names
  element C O           element symbols          selected           the current pick
  all | none
  sp | sp2 | sp3        hybridisation, inferred from the number of bonds
  numbonds 4            bond count; also >=N <=N >N <N and N-M
  with 2 H              exactly 2 bonded hydrogens; takes the same counts
  and / or / not        also spelled & | ! ; precedence is not > and > or
  ( )                   grouping             \"NAME\"            quote a name that
                                             collides with a keyword

examples
  DOM1 = PROT and resid 1-100        split a domain out of PROT
  PROT = PROA or PROB or PROC        merge three chains into one
  CH2  = element C and sp3 and with 2 H
  QUAT = element C and numbonds 4 and with 0 H

notes
  A bare word resolves in this order: component, residue name, element, atom name.
  Every resolution is echoed back, and the explicit keyword form always wins ties.
  sp/sp2/sp3 are inferred from bond counts, so an amide nitrogen reads as sp3.
  sp*, numbonds and with need a topology: a bare .gro has no bonds.
";
