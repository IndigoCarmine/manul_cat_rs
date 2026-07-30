//! Tokenizer for the component command language.
//!
//! Words are cut at a small delimiter set and then *classified*: a run made
//! only of digits and `-` becomes [`Tok::Num`], everything else stays a
//! [`Tok::Word`]. Classifying after cutting (rather than lexing digits eagerly)
//! is what lets PDB atom names that start with a digit — `1HB`, `2HG1` — survive
//! as single words while `1-100` still reads as one range token.

use super::ast::{ParseError, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// Bare word: a keyword, a component name, or a residue/atom/element name.
    Word(String),
    /// A quoted word. Never treated as a keyword, so a residue literally named
    /// `AND` can be written `resname "AND"`.
    Quoted(String),
    /// A run of digits and `-` that reads as one number or one inclusive range
    /// (`5`, `-3`, `1-100`, `-5--1`). Kept raw; the parser splits it.
    Num(String),
    Eq,
    LParen,
    RParen,
    /// `&`, the symbolic spelling of `and`.
    Amp,
    /// `|`, the symbolic spelling of `or`.
    Pipe,
    /// `!`, the symbolic spelling of `not`.
    Bang,
    Ge,
    Le,
    Gt,
    Lt,
}

impl Tok {
    /// How the token is quoted back in an error message.
    pub fn describe(&self) -> String {
        match self {
            Tok::Word(w) => format!("'{w}'"),
            Tok::Quoted(w) => format!("'\"{w}\"'"),
            Tok::Num(n) => format!("'{n}'"),
            Tok::Eq => "'='".into(),
            Tok::LParen => "'('".into(),
            Tok::RParen => "')'".into(),
            Tok::Amp => "'&'".into(),
            Tok::Pipe => "'|'".into(),
            Tok::Bang => "'!'".into(),
            Tok::Ge => "'>='".into(),
            Tok::Le => "'<='".into(),
            Tok::Gt => "'>'".into(),
            Tok::Lt => "'<'".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spanned {
    pub tok: Tok,
    pub span: Span,
}

/// Words the grammar claims. A word list (`resname SOL NA`) stops when it hits
/// one of these, which is what keeps `resname SOL and resid 1-10` from
/// swallowing `and` as a third residue name.
///
/// `within` and `of` are reserved but not yet implemented — reserving them now
/// means adding distance selections later cannot break an existing command that
/// used them as a component name.
pub const RESERVED: &[&str] = &[
    "and", "or", "not", "with", "within", "of", "resname", "resid", "index", "name", "element",
    "selected", "all", "none", "sp", "sp2", "sp3", "numbonds", "show", "hide", "only", "del",
    "rename", "list", "reset", "help",
];

pub fn is_reserved(word: &str) -> bool {
    RESERVED.iter().any(|k| k.eq_ignore_ascii_case(word))
}

/// Characters that end a word run. `-`, `*` and `'` are deliberately absent so
/// `1-100`, `C1'` and wildcard-ish atom names stay in one piece.
fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | '=' | '&' | '|' | '!' | '<' | '>' | '"' | '#')
}

/// Does this word run read as a number or an inclusive range?
///
/// Accepts `12`, `-3`, `1-100` and `-5--1`; rejects `1HB`, `1-`, `-` and `1-2-3`.
fn is_num_run(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    let first_digit = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == first_digit {
        return false; // no digits at all
    }
    if i == b.len() {
        return true; // a plain number
    }
    if b[i] != b'-' {
        return false; // letters or junk trailing the digits
    }
    i += 1;
    if i < b.len() && b[i] == b'-' {
        i += 1; // negative upper bound, as in "-5--1"
    }
    let second_digit = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    i > second_digit && i == b.len()
}

pub fn tokenize(input: &str) -> Result<Vec<Spanned>, ParseError> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let c = input[i..].chars().next().unwrap();

        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }

        // A comment runs to the end of the command.
        if c == '#' {
            break;
        }

        let start = i;
        let single = |tok: Tok, len: usize| Spanned {
            tok,
            span: Span::new(start, len),
        };

        match c {
            '(' => {
                out.push(single(Tok::LParen, 1));
                i += 1;
            }
            ')' => {
                out.push(single(Tok::RParen, 1));
                i += 1;
            }
            '=' => {
                out.push(single(Tok::Eq, 1));
                i += 1;
            }
            '&' => {
                out.push(single(Tok::Amp, 1));
                i += 1;
            }
            '|' => {
                out.push(single(Tok::Pipe, 1));
                i += 1;
            }
            '!' => {
                out.push(single(Tok::Bang, 1));
                i += 1;
            }
            '>' | '<' => {
                let eq = bytes.get(i + 1) == Some(&b'=');
                let tok = match (c, eq) {
                    ('>', true) => Tok::Ge,
                    ('>', false) => Tok::Gt,
                    ('<', true) => Tok::Le,
                    (_, _) => Tok::Lt,
                };
                let len = if eq { 2 } else { 1 };
                out.push(single(tok, len));
                i += len;
            }
            '"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += 1;
                }
                if j >= bytes.len() {
                    return Err(ParseError::new(
                        "unterminated quoted name",
                        Span::new(start, input.len() - start),
                    )
                    .with_hint("close it with a matching '\"'"));
                }
                out.push(Spanned {
                    tok: Tok::Quoted(input[i + 1..j].to_string()),
                    span: Span::new(start, j + 1 - start),
                });
                i = j + 1;
            }
            _ => {
                let mut j = i;
                while j < bytes.len() {
                    let ch = input[j..].chars().next().unwrap();
                    if is_delimiter(ch) {
                        break;
                    }
                    j += ch.len_utf8();
                }
                let word = &input[i..j];
                let tok = if is_num_run(word) {
                    Tok::Num(word.to_string())
                } else {
                    Tok::Word(word.to_string())
                };
                out.push(Spanned {
                    tok,
                    span: Span::new(start, j - start),
                });
                i = j;
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(input: &str) -> Vec<Tok> {
        tokenize(input).unwrap().into_iter().map(|s| s.tok).collect()
    }

    #[test]
    fn digit_leading_atom_names_stay_whole() {
        // PDB hydrogens are routinely named 1HB / 2HG1; splitting them at the
        // leading digit would make `name 1HB` unwritable.
        assert_eq!(
            toks("name 1HB 2HG1"),
            vec![
                Tok::Word("name".into()),
                Tok::Word("1HB".into()),
                Tok::Word("2HG1".into())
            ]
        );
    }

    #[test]
    fn ranges_and_negative_numbers() {
        assert_eq!(toks("1-100"), vec![Tok::Num("1-100".into())]);
        assert_eq!(toks("-5"), vec![Tok::Num("-5".into())]);
        assert_eq!(toks("-5--1"), vec![Tok::Num("-5--1".into())]);
        // Incomplete ranges stay words so the parser can report them precisely.
        assert_eq!(toks("1-"), vec![Tok::Word("1-".into())]);
    }

    #[test]
    fn comparison_operators_beat_bare_angle_brackets() {
        assert_eq!(
            toks(">=2 <3 >1 <=4"),
            vec![
                Tok::Ge,
                Tok::Num("2".into()),
                Tok::Lt,
                Tok::Num("3".into()),
                Tok::Gt,
                Tok::Num("1".into()),
                Tok::Le,
                Tok::Num("4".into()),
            ]
        );
    }

    #[test]
    fn quoted_names_escape_the_keyword_list() {
        assert_eq!(
            toks("resname \"AND\""),
            vec![Tok::Word("resname".into()), Tok::Quoted("AND".into())]
        );
        assert!(tokenize("resname \"AND").is_err());
    }

    #[test]
    fn comments_and_tight_punctuation() {
        assert_eq!(
            toks("A=B # trailing note"),
            vec![Tok::Word("A".into()), Tok::Eq, Tok::Word("B".into())]
        );
        assert_eq!(
            toks("(a|b)"),
            vec![
                Tok::LParen,
                Tok::Word("a".into()),
                Tok::Pipe,
                Tok::Word("b".into()),
                Tok::RParen
            ]
        );
    }

    #[test]
    fn spans_point_at_the_token() {
        let spanned = tokenize("DOM1 = PROT").unwrap();
        assert_eq!(spanned[0].span, Span::new(0, 4));
        assert_eq!(spanned[1].span, Span::new(5, 1));
        assert_eq!(spanned[2].span, Span::new(7, 4));
    }

    #[test]
    fn primed_and_starred_atom_names_survive() {
        assert_eq!(toks("C1'"), vec![Tok::Word("C1'".into())]);
    }
}
