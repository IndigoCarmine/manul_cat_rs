//! Syntax tree and error type for the component command language.
//!
//! The language has two kinds of statement: an *assignment* (`NAME = expr`),
//! which moves every matched atom into the named component, and a handful of
//! *verbs* (`show`/`hide`/`only`/`del`/`rename`/`list`/`reset`/`help`). A bare
//! expression is also a statement — it just reports how many atoms it matches,
//! which is how you check an expression before committing it.

/// Byte range into the command string a token or error came from.
///
/// Byte offsets (not char offsets) so slicing the input is free; [`ParseError`]
/// converts to a character column only when it renders the caret, which is the
/// one place the distinction is visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub len: usize,
}

impl Span {
    pub fn new(start: usize, len: usize) -> Self {
        Self { start, len }
    }

    /// A zero-width span at the end of `input`, for "unexpected end of input".
    pub fn eof(input: &str) -> Self {
        Self {
            start: input.len(),
            len: 0,
        }
    }
}

/// A parse failure with the span of the offending token.
///
/// Modelled on [`ParseNdxError`](crate::parsing::ndx) — the repo's one existing
/// structured error type — but pointing at a column instead of a line, since a
/// command is always a single line.
#[derive(Clone, Debug)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
    pub hint: Option<String>,
}

impl ParseError {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// A multi-line diagnostic with a caret under the offending span:
    ///
    /// ```text
    ///   DOM1 = PROT and resid 1-
    ///                         ^^ 'resid' needs a number or a range
    ///   hint: ...
    /// ```
    pub fn render(&self, input: &str) -> String {
        format!("  {input}\n{}", self.caret(input))
    }

    /// Just the caret and hint lines, for callers that have already echoed the
    /// command — the command log prefixes every entered line with `> `, which is
    /// the same two-column gutter this indents by, so the caret still lands
    /// under its token without repeating the input.
    pub fn caret(&self, input: &str) -> String {
        let col = input
            .get(..self.span.start.min(input.len()))
            .map(|s| s.chars().count())
            .unwrap_or(0);
        let width = input
            .get(self.span.start..self.span.start + self.span.len)
            .map(|s| s.chars().count().max(1))
            .unwrap_or(1);

        let mut out = format!(
            "  {}{} {}",
            " ".repeat(col),
            "^".repeat(width),
            self.message
        );
        if let Some(hint) = &self.hint {
            out.push_str(&format!("\n  hint: {hint}"));
        }
        out
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (column {})", self.message, self.span.start + 1)
    }
}

/// An inclusive numeric range. A single number parses as `lo == hi`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumRange {
    pub lo: i64,
    pub hi: i64,
}

impl NumRange {
    pub fn contains(&self, value: i64) -> bool {
        value >= self.lo && value <= self.hi
    }
}

/// How many neighbours (or bonds) an atom must have to match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CountSpec {
    Exact(i64),
    /// Inclusive on both ends.
    Range(i64, i64),
    Ge(i64),
    Le(i64),
    Gt(i64),
    Lt(i64),
}

impl CountSpec {
    pub fn matches(&self, n: i64) -> bool {
        match *self {
            CountSpec::Exact(v) => n == v,
            CountSpec::Range(lo, hi) => n >= lo && n <= hi,
            CountSpec::Ge(v) => n >= v,
            CountSpec::Le(v) => n <= v,
            CountSpec::Gt(v) => n > v,
            CountSpec::Lt(v) => n < v,
        }
    }
}

/// Hybridisation, inferred from coordination number (see `eval::hybridisation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hybrid {
    Sp,
    Sp2,
    Sp3,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    All,
    None,
    /// The atoms currently picked in the viewport / by the old `aC1|aC2` selector.
    Selected,
    /// Residue names, matched case-insensitively. Several names union together.
    ResName(Vec<String>),
    /// `AtomMeta::res_seq` ranges.
    ResId(Vec<NumRange>),
    /// 1-based atom index ranges, GROMACS style.
    Index(Vec<NumRange>),
    /// Atom names, matched case-insensitively.
    Name(Vec<String>),
    /// Element symbols, matched case-insensitively.
    Element(Vec<String>),
    Hybrid(Hybrid),
    /// Bond count (graph degree) of the atom itself.
    NumBonds(CountSpec),
    /// Atoms whose bonded neighbours matching `of` number `count`.
    With { count: CountSpec, of: Box<Expr> },
    /// A bare word, resolved against components / residue names / elements /
    /// atom names at evaluation time (see `eval::resolve_ident`).
    Ident(String),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    /// `NAME = expr` — move every matched atom into `name`, creating it if new.
    Assign { name: String, expr: Expr },
    /// A bare expression: report the match count without changing anything.
    Count(Expr),
    Show(Targets),
    Hide(Targets),
    /// Show the named component and hide every other one.
    Only(String),
    /// Dissolve: matched atoms fall back to their residue-name component.
    Del(Vec<String>),
    Rename { from: String, to: String },
    List,
    /// Rebuild the default one-component-per-residue-name partition.
    Reset,
    Help,
}

/// Which components a `show`/`hide` applies to.
#[derive(Clone, Debug, PartialEq)]
pub enum Targets {
    All,
    Named(Vec<String>),
}
