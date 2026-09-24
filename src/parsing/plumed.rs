//! PLUMED input (`plumed.dat`) parsing.
//!
//! Syntax only: this module turns the file into labelled actions with their
//! keywords, and says nothing about whether the atoms those keywords name
//! actually exist. Resolving serials against a loaded structure needs the
//! molecule and lives in [`crate::plumed_view`].
//!
//! The grammar covered here is PLUMED's general one, not a per-action schema,
//! so an action this viewer has never heard of still parses and still lists its
//! keywords:
//!
//! ```text
//! # a comment
//! UNITS LENGTH=nm TIME=ps                    # bare action
//! sLo1: CENTER ATOMS=3858-3873,3876          # label: ACTION KEY=VALUE
//! UPPER_WALLS ARG=rho AT=0.5 LABEL=w_rho     # label through LABEL=
//! long: DISTANCE \                           # backslash continuation
//!       ATOMS=1,2
//! METAD ...                                  # `...` block
//!   ARG=theta
//!   CALC_RCT                                 # a bare word is a flag
//! ... METAD
//! ```

use std::fmt;

/// One `KEY=VALUE` pair. `line` is the physical line it was written on, which
/// is not the action's own line inside a `...` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlumedKeyword {
    pub key: String,
    /// The value with one layer of `{...}` stripped, so `FUNC={a b}` reads as
    /// `a b` — which is what PLUMED itself sees.
    pub value: String,
    pub line: usize,
}

/// One PLUMED statement: an action, its label, and everything it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlumedAction {
    /// `sLo1` in `sLo1: CENTER ...`, or the value of a `LABEL=` keyword.
    pub label: Option<String>,
    /// The action name, upper-cased as PLUMED writes it (`CENTER`, `METAD`).
    pub name: String,
    pub keywords: Vec<PlumedKeyword>,
    /// Bare words that are not `KEY=VALUE`, e.g. `CALC_RCT`, `NOPBC`.
    pub flags: Vec<String>,
    /// 1-based physical line the statement starts on.
    pub line: usize,
    /// 1-based physical line it ends on. Equal to `line` unless a `...` block
    /// or a `\` continuation stretched it.
    pub end_line: usize,
}

impl PlumedAction {
    /// The value of `key`, if it was given. Case-insensitive, as PLUMED is.
    pub fn keyword(&self, key: &str) -> Option<&str> {
        self.keywords
            .iter()
            .find(|kw| kw.key.eq_ignore_ascii_case(key))
            .map(|kw| kw.value.as_str())
    }

    /// Whether `flag` was given as a bare word.
    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
    }

    /// Every keyword that holds a list of atoms.
    ///
    /// PLUMED spells this several ways — `ATOMS`, the numbered `ATOMS1`,
    /// `ATOMS2`, … a multicolvar takes, and the `GROUP` / `GROUPA` / `GROUPB`
    /// of the pairwise actions — and the viewer wants to highlight all of them
    /// the same way, so they are recognised by prefix rather than enumerated.
    pub fn atom_keywords(&self) -> impl Iterator<Item = &PlumedKeyword> {
        self.keywords.iter().filter(|kw| {
            let key = kw.key.to_ascii_uppercase();
            let stem = key.trim_end_matches(|c: char| c.is_ascii_digit());
            matches!(stem, "ATOMS" | "GROUP" | "GROUPA" | "GROUPB" | "ENTITY")
        })
    }
}

/// A parsed `plumed.dat`.
///
/// `lines` is the file verbatim so the UI can show the source it came from, and
/// every action carries the line span it occupies, which is what keeps the text
/// view and the 3D highlight pointing at the same thing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlumedFile {
    pub lines: Vec<String>,
    pub actions: Vec<PlumedAction>,
}

/// One entry of an `ATOMS=` list.
///
/// PLUMED mixes serial numbers with references to virtual atoms defined earlier
/// in the same file (`ATOMS=sLo1,cenLo,cenUp,sUp1`), so the two cannot be
/// flattened to numbers here — the labels only resolve once the whole file is
/// read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtomToken {
    /// A serial, or a `lo-hi` / `lo-hi:step` range. `lo == hi` for a single
    /// serial and `step` is 1 unless written.
    Range { lo: u32, hi: u32, step: u32 },
    /// A label defined elsewhere in the file, or a PLUMED special like
    /// `@mdatoms`.
    Label(String),
}

impl AtomToken {
    /// Append the serials this token stands for. A [`AtomToken::Label`] appends
    /// nothing — the caller resolves those.
    pub fn extend_serials(&self, out: &mut Vec<u32>) {
        if let Self::Range { lo, hi, step } = self {
            let step = (*step).max(1);
            let mut n = *lo;
            while n <= *hi {
                out.push(n);
                // `hi` can be u32::MAX in a malformed file, which would wrap
                // back to the start and spin forever.
                match n.checked_add(step) {
                    Some(next) => n = next,
                    None => break,
                }
            }
        }
    }
}

impl PlumedFile {
    pub fn parse(input: &str) -> Result<Self, ParsePlumedError> {
        let lines: Vec<String> = input.lines().map(str::to_string).collect();
        let segments = logical_segments(&lines)?;
        let actions = group_statements(segments)?;
        Ok(Self { lines, actions })
    }
}

/// A comment-stripped, continuation-joined line, and the physical lines it
/// came from.
#[derive(Debug)]
struct Segment {
    tokens: Vec<String>,
    start: usize,
    end: usize,
}

/// Strip `#` comments, join `\` continuations, drop blank lines, and tokenize.
fn logical_segments(lines: &[String]) -> Result<Vec<Segment>, ParsePlumedError> {
    let mut segments = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let start = i + 1;
        let mut text = strip_comment(&lines[i]).trim().to_string();
        let mut end = start;

        // A trailing backslash continues onto the next physical line. The
        // comment on each line is stripped first, so `A=1 \  # note` still
        // continues.
        while text.ends_with('\\') {
            text.pop();
            i += 1;
            let Some(next) = lines.get(i) else {
                // A continuation with nothing after it: treat the statement as
                // ending here rather than erroring, which is what PLUMED does.
                break;
            };
            text.push(' ');
            text.push_str(strip_comment(next).trim());
            end = i + 1;
        }

        i += 1;

        if text.trim().is_empty() {
            continue;
        }

        segments.push(Segment {
            tokens: tokenize(&text, start)?,
            start,
            end,
        });
    }

    Ok(segments)
}

/// Everything from the first `#` outside braces is a comment.
fn strip_comment(line: &str) -> &str {
    let mut depth = 0i32;
    for (idx, ch) in line.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            '#' if depth <= 0 => return &line[..idx],
            _ => {}
        }
    }
    line
}

/// Split on whitespace, except inside `{...}`, which PLUMED uses to give a
/// keyword a value containing spaces.
fn tokenize(text: &str, line: usize) -> Result<Vec<String>, ParsePlumedError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;

    for ch in text.chars() {
        match ch {
            '{' => {
                depth += 1;
                current.push(ch);
            }
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return Err(ParsePlumedError::UnbalancedBraces { line });
                }
                current.push(ch);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if depth != 0 {
        return Err(ParsePlumedError::UnbalancedBraces { line });
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

/// Fold segments into statements, joining `ACTION ...` / `... ACTION` blocks.
fn group_statements(segments: Vec<Segment>) -> Result<Vec<PlumedAction>, ParsePlumedError> {
    let mut actions = Vec::new();
    let mut iter = segments.into_iter().peekable();

    while let Some(head) = iter.next() {
        // A lone `...` outside a block has no statement to attach to.
        if head.tokens.first().is_some_and(|t| t == "...") {
            return Err(ParsePlumedError::StrayBlockEnd { line: head.start });
        }

        let opens_block = head.tokens.last().is_some_and(|t| t == "...");
        if !opens_block {
            actions.push(build_action(&[head])?);
            continue;
        }

        // `ACTION ...`: collect until the `...` that closes it.
        let mut parts = vec![Segment {
            tokens: head.tokens[..head.tokens.len() - 1].to_vec(),
            start: head.start,
            end: head.end,
        }];
        let open_line = head.start;
        let mut closed = false;

        for body in iter.by_ref() {
            if body.tokens.first().is_some_and(|t| t == "...") {
                // The terminator may name the action again — `... METAD` — in
                // which case PLUMED checks it matches, so we do too.
                if let Some(named) = body.tokens.get(1) {
                    let opened = parts[0]
                        .tokens
                        .iter()
                        .find(|t| !t.ends_with(':'))
                        .map(String::as_str)
                        .unwrap_or_default();
                    if !named.eq_ignore_ascii_case(opened) {
                        return Err(ParsePlumedError::BlockEndMismatch {
                            line: body.start,
                            expected: opened.to_string(),
                            found: named.clone(),
                        });
                    }
                }
                parts.push(Segment {
                    tokens: Vec::new(),
                    start: body.start,
                    end: body.end,
                });
                closed = true;
                break;
            }
            parts.push(body);
        }

        if !closed {
            return Err(ParsePlumedError::UnterminatedBlock { line: open_line });
        }

        actions.push(build_action(&parts)?);
    }

    Ok(actions)
}

/// Turn one statement's segments into a [`PlumedAction`].
fn build_action(parts: &[Segment]) -> Result<PlumedAction, ParsePlumedError> {
    let start = parts.first().map(|s| s.start).unwrap_or(1);
    let end = parts.last().map(|s| s.end).unwrap_or(start);

    let mut label: Option<String> = None;
    let mut name: Option<String> = None;
    let mut keywords = Vec::new();
    let mut flags = Vec::new();

    for part in parts {
        for token in &part.tokens {
            // `label:` or `label:ACTION` — the leading label, once.
            if name.is_none()
                && label.is_none()
                && let Some((head, rest)) = token.split_once(':')
                && !head.is_empty()
            {
                label = Some(head.to_string());
                if rest.is_empty() {
                    continue;
                }
                name = Some(rest.to_ascii_uppercase());
                continue;
            }

            if name.is_none() {
                name = Some(token.to_ascii_uppercase());
                continue;
            }

            match token.split_once('=') {
                Some((key, value)) if !key.is_empty() => {
                    let value = unbrace(value).to_string();
                    if key.eq_ignore_ascii_case("LABEL") && label.is_none() {
                        label = Some(value.clone());
                    }
                    keywords.push(PlumedKeyword {
                        key: key.to_string(),
                        value,
                        line: part.start,
                    });
                }
                _ => flags.push(token.clone()),
            }
        }
    }

    let name = name.ok_or(ParsePlumedError::MissingAction { line: start })?;

    Ok(PlumedAction {
        label,
        name,
        keywords,
        flags,
        line: start,
        end_line: end,
    })
}

/// Strip one enclosing `{...}` from a keyword value.
fn unbrace(value: &str) -> &str {
    value
        .strip_prefix('{')
        .and_then(|v| v.strip_suffix('}'))
        .map(str::trim)
        .unwrap_or(value)
}

/// Parse an `ATOMS=` style value into its entries.
///
/// `line` is only carried into the error, so a bad token points at the line the
/// user has to fix.
pub fn parse_atom_list(value: &str, line: usize) -> Result<Vec<AtomToken>, ParsePlumedError> {
    let mut out = Vec::new();

    for raw in value
        .split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        // A leading digit means a serial or a range; anything else is a label
        // (or a PLUMED special like `@mdatoms`).
        if !raw.starts_with(|c: char| c.is_ascii_digit()) {
            out.push(AtomToken::Label(raw.to_string()));
            continue;
        }
        out.push(parse_range(raw, line)?);
    }

    Ok(out)
}

/// `n`, `lo-hi`, or `lo-hi:step`.
fn parse_range(raw: &str, line: usize) -> Result<AtomToken, ParsePlumedError> {
    let bad = || ParsePlumedError::BadAtomToken {
        line,
        token: raw.to_string(),
    };

    let (span, step) = match raw.split_once(':') {
        Some((span, step)) => (span, step.parse::<u32>().map_err(|_| bad())?.max(1)),
        None => (raw, 1),
    };

    // Serials are positive, so the only `-` in a well-formed span is the range
    // separator.
    let Some((lo, hi)) = span.split_once('-') else {
        let n = span.parse::<u32>().map_err(|_| bad())?;
        return Ok(AtomToken::Range {
            lo: n,
            hi: n,
            step: 1,
        });
    };

    let lo = lo.parse::<u32>().map_err(|_| bad())?;
    let hi = hi.parse::<u32>().map_err(|_| bad())?;
    if hi < lo {
        return Err(ParsePlumedError::BackwardsRange {
            line,
            token: raw.to_string(),
        });
    }

    Ok(AtomToken::Range { lo, hi, step })
}

#[derive(Debug)]
pub enum ParsePlumedError {
    UnbalancedBraces { line: usize },

    MissingAction { line: usize },

    UnterminatedBlock { line: usize },

    BlockEndMismatch {
        line: usize,
        expected: String,
        found: String,
    },

    StrayBlockEnd { line: usize },

    BadAtomToken { line: usize, token: String },

    BackwardsRange { line: usize, token: String },
}

impl fmt::Display for ParsePlumedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnbalancedBraces { line } => {
                write!(f, "unbalanced braces at line {line}")
            }
            Self::MissingAction { line } => {
                write!(f, "no action name at line {line}")
            }
            Self::UnterminatedBlock { line } => {
                write!(f, "`...` block opened at line {line} is never closed")
            }
            Self::BlockEndMismatch {
                line,
                expected,
                found,
            } => write!(
                f,
                "line {line} closes the block with `{found}`, but it opened as `{expected}`"
            ),
            Self::StrayBlockEnd { line } => {
                write!(f, "`...` at line {line} closes a block that was never opened")
            }
            Self::BadAtomToken { line, token } => {
                write!(f, "invalid atom entry at line {line}: {token}")
            }
            Self::BackwardsRange { line, token } => write!(
                f,
                "atom range at line {line} counts backwards: {token} (write it low-high)"
            ),
        }
    }
}

impl std::error::Error for ParsePlumedError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> PlumedFile {
        PlumedFile::parse(input).expect("should parse")
    }

    fn serials(action: &PlumedAction, key: &str) -> Vec<u32> {
        let value = action.keyword(key).expect("keyword present");
        let mut out = Vec::new();
        for token in parse_atom_list(value, action.line).unwrap() {
            token.extend_serials(&mut out);
        }
        out
    }

    #[test]
    fn a_labelled_action_keeps_its_label_name_and_keywords() {
        let file = parse("sLo1: CENTER ATOMS=1-4 MASS\n");
        assert_eq!(file.actions.len(), 1);
        let a = &file.actions[0];
        assert_eq!(a.label.as_deref(), Some("sLo1"));
        assert_eq!(a.name, "CENTER");
        assert_eq!(a.keyword("ATOMS"), Some("1-4"));
        assert!(a.has_flag("MASS"));
        assert_eq!((a.line, a.end_line), (1, 1));
    }

    #[test]
    fn a_bare_action_has_no_label() {
        let file = parse("UNITS LENGTH=nm TIME=ps ENERGY=kj/mol\n");
        let a = &file.actions[0];
        assert!(a.label.is_none());
        assert_eq!(a.name, "UNITS");
        assert_eq!(a.keyword("length"), Some("nm"));
    }

    #[test]
    fn the_label_keyword_names_the_action_too() {
        let file = parse("UPPER_WALLS ARG=rho AT=0.5 LABEL=w_rho\n");
        assert_eq!(file.actions[0].label.as_deref(), Some("w_rho"));
        assert_eq!(file.actions[0].name, "UPPER_WALLS");
    }

    #[test]
    fn comments_and_blank_lines_are_dropped() {
        let file = parse("# a comment\n\nUNITS LENGTH=nm  # trailing note\n");
        assert_eq!(file.actions.len(), 1);
        assert_eq!(file.actions[0].keyword("LENGTH"), Some("nm"));
        // The source is kept verbatim for the text view.
        assert_eq!(file.lines.len(), 3);
    }

    #[test]
    fn a_backslash_continues_onto_the_next_line() {
        let file = parse("d: DISTANCE \\\n   ATOMS=1,2\n");
        let a = &file.actions[0];
        assert_eq!(a.name, "DISTANCE");
        assert_eq!(a.keyword("ATOMS"), Some("1,2"));
        assert_eq!((a.line, a.end_line), (1, 2));
    }

    #[test]
    fn a_dots_block_collects_its_keywords_and_spans_its_lines() {
        let file = parse("METAD ...\n  LABEL=mtd\n  ARG=theta\n  CALC_RCT\n... METAD\n");
        let a = &file.actions[0];
        assert_eq!(a.name, "METAD");
        assert_eq!(a.label.as_deref(), Some("mtd"));
        assert_eq!(a.keyword("ARG"), Some("theta"));
        assert!(a.has_flag("CALC_RCT"));
        assert_eq!((a.line, a.end_line), (1, 5));
        // Keywords point at their own line, not the block's.
        let arg = a.keywords.iter().find(|k| k.key == "ARG").unwrap();
        assert_eq!(arg.line, 3);
    }

    #[test]
    fn a_bare_dots_also_closes_a_block() {
        let file = parse("METAD ...\n  PACE=500\n...\n");
        assert_eq!(file.actions[0].keyword("PACE"), Some("500"));
        assert_eq!(file.actions[0].end_line, 3);
    }

    #[test]
    fn a_block_closed_with_the_wrong_name_is_rejected() {
        let err = PlumedFile::parse("METAD ...\n  PACE=500\n... PRINT\n").unwrap_err();
        assert!(
            matches!(err, ParsePlumedError::BlockEndMismatch { line: 3, .. }),
            "{err}"
        );
    }

    #[test]
    fn an_unterminated_block_points_at_where_it_opened() {
        let err = PlumedFile::parse("METAD ...\n  PACE=500\n").unwrap_err();
        assert!(
            matches!(err, ParsePlumedError::UnterminatedBlock { line: 1 }),
            "{err}"
        );
    }

    #[test]
    fn a_stray_block_end_is_rejected() {
        let err = PlumedFile::parse("UNITS LENGTH=nm\n... METAD\n").unwrap_err();
        assert!(
            matches!(err, ParsePlumedError::StrayBlockEnd { line: 2 }),
            "{err}"
        );
    }

    #[test]
    fn a_braced_value_may_contain_spaces() {
        let file = parse("c: CUSTOM ARG=a,b FUNC={a + b} PERIODIC=NO\n");
        let a = &file.actions[0];
        assert_eq!(a.keyword("FUNC"), Some("a + b"));
        assert_eq!(a.keyword("PERIODIC"), Some("NO"));
    }

    #[test]
    fn a_hash_inside_braces_is_not_a_comment() {
        let file = parse("c: CUSTOM FUNC={a # b} PERIODIC=NO\n");
        assert_eq!(file.actions[0].keyword("FUNC"), Some("a # b"));
    }

    #[test]
    fn unbalanced_braces_are_rejected() {
        let err = PlumedFile::parse("c: CUSTOM FUNC={a + b\n").unwrap_err();
        assert!(
            matches!(err, ParsePlumedError::UnbalancedBraces { line: 1 }),
            "{err}"
        );
    }

    #[test]
    fn a_mixed_range_list_expands_to_the_right_serials() {
        let file = parse("s: CENTER ATOMS=10-13,16,18,21-22\n");
        assert_eq!(
            serials(&file.actions[0], "ATOMS"),
            vec![10, 11, 12, 13, 16, 18, 21, 22]
        );
    }

    #[test]
    fn a_range_may_carry_a_stride() {
        let file = parse("s: CENTER ATOMS=1-10:3\n");
        assert_eq!(serials(&file.actions[0], "ATOMS"), vec![1, 4, 7, 10]);
    }

    #[test]
    fn atom_labels_stay_unresolved() {
        let file = parse("d: TORSION ATOMS=sLo1,cenLo,cenUp,sUp1\n");
        let tokens = parse_atom_list(file.actions[0].keyword("ATOMS").unwrap(), 1).unwrap();
        assert_eq!(
            tokens,
            vec![
                AtomToken::Label("sLo1".into()),
                AtomToken::Label("cenLo".into()),
                AtomToken::Label("cenUp".into()),
                AtomToken::Label("sUp1".into()),
            ]
        );
    }

    #[test]
    fn a_backwards_range_is_rejected() {
        let err = parse_atom_list("10-4", 7).unwrap_err();
        assert!(
            matches!(err, ParsePlumedError::BackwardsRange { line: 7, .. }),
            "{err}"
        );
    }

    #[test]
    fn a_malformed_atom_entry_is_rejected() {
        let err = parse_atom_list("3-x", 2).unwrap_err();
        assert!(
            matches!(err, ParsePlumedError::BadAtomToken { line: 2, .. }),
            "{err}"
        );
    }

    #[test]
    fn numbered_and_group_keywords_are_recognised_as_atom_lists() {
        let file = parse("c: COORDINATION GROUPA=1-3 GROUPB=4-6 R_0=0.5\n");
        let keys: Vec<&str> = file.actions[0]
            .atom_keywords()
            .map(|kw| kw.key.as_str())
            .collect();
        assert_eq!(keys, vec!["GROUPA", "GROUPB"]);

        let file = parse("m: DISTANCES ATOMS1=1,2 ATOMS2=3,4\n");
        let keys: Vec<&str> = file.actions[0]
            .atom_keywords()
            .map(|kw| kw.key.as_str())
            .collect();
        assert_eq!(keys, vec!["ATOMS1", "ATOMS2"]);
    }

    /// The shape of the file bundled with the repo, in miniature: every
    /// construct it uses, in one pass.
    #[test]
    fn the_bundled_files_shape_parses_end_to_end() {
        let file = parse(concat!(
            "# PLUMED input\n",
            "UNITS LENGTH=nm TIME=ps ENERGY=kj/mol\n",
            "\n",
            "sLo1: CENTER ATOMS=3858-3873,3876,3878,3881-3884\n",
            "cenLo: CENTER ATOMS=3858-3873,4017-4032\n",
            "cenUp: CENTER ATOMS=4812-4827\n",
            "sUp1: CENTER ATOMS=4812-4827,4830\n",
            "dphi1: TORSION ATOMS=sLo1,cenLo,cenUp,sUp1\n",
            "dLU: DISTANCE ATOMS=cenLo,cenUp\n",
            "theta: CUSTOM ARG=dphi1 VAR=a FUNC=cos(a) PERIODIC=NO\n",
            "UPPER_WALLS ARG=theta AT=0.5 KAPPA=5000.0 LABEL=w\n",
            "METAD ...\n",
            "  LABEL=mtd\n",
            "  ARG=theta\n",
            "  GRID_MIN=-pi\n",
            "  CALC_RCT\n",
            "... METAD\n",
            "PRINT ARG=theta,mtd.bias STRIDE=500 FILE=COLVAR\n",
        ));

        let names: Vec<&str> = file.actions.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "UNITS",
                "CENTER",
                "CENTER",
                "CENTER",
                "CENTER",
                "TORSION",
                "DISTANCE",
                "CUSTOM",
                "UPPER_WALLS",
                "METAD",
                "PRINT",
            ]
        );

        let metad = file.actions.iter().find(|a| a.name == "METAD").unwrap();
        assert_eq!(metad.label.as_deref(), Some("mtd"));
        assert_eq!((metad.line, metad.end_line), (12, 17));
        // A negative-looking value is a value, not a range.
        assert_eq!(metad.keyword("GRID_MIN"), Some("-pi"));
    }
}
