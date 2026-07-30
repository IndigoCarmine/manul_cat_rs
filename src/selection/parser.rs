//! Recursive-descent parser for the component command language.
//!
//! Precedence is the usual `not` > `and` > `or`; `not` is right-recursive.
//! There is no implicit `and` between adjacent terms (VMD allows it) because
//! word lists are greedy — `resname SOL NA` has to mean two residue names, not
//! `resname SOL` intersected with something called `NA`.

use super::ast::{CountSpec, Expr, Hybrid, NumRange, ParseError, Span, Statement, Targets};
use super::lexer::{Spanned, Tok, is_reserved, tokenize};

pub fn parse_statement(input: &str) -> Result<Statement, ParseError> {
    let toks = tokenize(input)?;
    if toks.is_empty() {
        return Err(ParseError::new("empty command", Span::eof(input))
            .with_hint("try 'help', or an assignment like DOM1 = PROT and resid 1-100"));
    }
    let mut p = Parser {
        toks,
        pos: 0,
        eof: Span::eof(input),
    };
    let stmt = p.statement()?;
    if let Some(extra) = p.peek_spanned() {
        return Err(ParseError::new(
            format!("unexpected {} after the end of the command", extra.tok.describe()),
            extra.span,
        ));
    }
    Ok(stmt)
}

struct Parser {
    toks: Vec<Spanned>,
    pos: usize,
    eof: Span,
}

impl Parser {
    fn peek_spanned(&self) -> Option<&Spanned> {
        self.toks.get(self.pos)
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|s| &s.tok)
    }

    fn span_here(&self) -> Span {
        self.toks.get(self.pos).map(|s| s.span).unwrap_or(self.eof)
    }

    fn bump(&mut self) -> Option<Spanned> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// The lower-cased word at the cursor, if the cursor is on a bare word.
    fn peek_keyword(&self) -> Option<String> {
        match self.peek() {
            Some(Tok::Word(w)) => Some(w.to_ascii_lowercase()),
            _ => None,
        }
    }

    // ---------------------------------------------------------------- statements

    fn statement(&mut self) -> Result<Statement, ParseError> {
        // `NAME = expr` is the only form with a `=`, and it is always the second
        // token, so a one-token lookahead settles assignment vs. everything else.
        if matches!(self.peek(), Some(Tok::Word(_) | Tok::Quoted(_)))
            && matches!(self.toks.get(self.pos + 1).map(|s| &s.tok), Some(Tok::Eq))
        {
            let (name, quoted, name_span) = self.name_token("a component name")?;
            self.pos += 1; // '='
            if !quoted && is_reserved(&name) {
                return Err(ParseError::new(
                    format!("'{name}' is a reserved word and cannot name a component"),
                    name_span,
                )
                .with_hint("quote it if you really mean it, as in \"all\" = ..."));
            }
            if self.peek().is_none() {
                return Err(ParseError::new(
                    "expected a selection expression after '='",
                    self.eof,
                )
                .with_hint("for example: PROT and resid 1-100"));
            }
            let expr = self.expr()?;
            return Ok(Statement::Assign { name, expr });
        }

        match self.peek_keyword().as_deref() {
            Some("show") => {
                self.pos += 1;
                Ok(Statement::Show(self.targets("show")?))
            }
            Some("hide") => {
                self.pos += 1;
                Ok(Statement::Hide(self.targets("hide")?))
            }
            Some("only") => {
                self.pos += 1;
                Ok(Statement::Only(self.component_name("a component name")?))
            }
            Some("del") => {
                self.pos += 1;
                Ok(Statement::Del(self.name_args("del")?))
            }
            Some("rename") => {
                self.pos += 1;
                let from = self.component_name("the component to rename")?;
                let (to, quoted, to_span) = self.name_token("the new name")?;
                if !quoted && is_reserved(&to) {
                    return Err(ParseError::new(
                        format!("'{to}' is a reserved word and cannot name a component"),
                        to_span,
                    ));
                }
                Ok(Statement::Rename { from, to })
            }
            Some("list") => {
                self.pos += 1;
                Ok(Statement::List)
            }
            Some("reset") => {
                self.pos += 1;
                Ok(Statement::Reset)
            }
            Some("help") => {
                self.pos += 1;
                Ok(Statement::Help)
            }
            // Anything else is a bare expression, which reports its match count.
            _ => Ok(Statement::Count(self.expr()?)),
        }
    }

    /// `show`/`hide` take either the word `all` or one or more component names.
    fn targets(&mut self, verb: &str) -> Result<Targets, ParseError> {
        if self.peek_keyword().as_deref() == Some("all") && self.toks.len() == self.pos + 1 {
            self.pos += 1;
            return Ok(Targets::All);
        }
        Ok(Targets::Named(self.name_args(verb)?))
    }

    fn name_args(&mut self, verb: &str) -> Result<Vec<String>, ParseError> {
        let mut names = Vec::new();
        while matches!(self.peek(), Some(Tok::Word(_) | Tok::Quoted(_) | Tok::Num(_))) {
            names.push(self.component_name("a component name")?);
        }
        if names.is_empty() {
            return Err(ParseError::new(
                format!("'{verb}' needs at least one component name"),
                self.span_here(),
            )
            .with_hint("run 'list' to see the components you have"));
        }
        Ok(names)
    }

    /// One name token, with whether it was quoted and where it sat.
    ///
    /// Quotedness matters because a quoted name deliberately bypasses the
    /// reserved-word check, so `"all" = ...` can name a component `all`.
    /// Numbers are accepted because a residue can legitimately be called `1`.
    fn name_token(&mut self, what: &str) -> Result<(String, bool, Span), ParseError> {
        let span = self.span_here();
        match self.bump().map(|s| s.tok) {
            Some(Tok::Quoted(w)) => Ok((w, true, span)),
            Some(Tok::Word(w)) | Some(Tok::Num(w)) => Ok((w, false, span)),
            Some(other) => Err(ParseError::new(
                format!("expected {what}, found {}", other.describe()),
                span,
            )),
            None => Err(ParseError::new(format!("expected {what}"), self.eof)),
        }
    }

    fn component_name(&mut self, what: &str) -> Result<String, ParseError> {
        self.name_token(what).map(|(name, _, _)| name)
    }

    // --------------------------------------------------------------- expressions

    fn expr(&mut self) -> Result<Expr, ParseError> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.and_expr()?;
        loop {
            let is_or = matches!(self.peek(), Some(Tok::Pipe)) || self.peek_keyword().as_deref() == Some("or");
            if !is_or {
                break;
            }
            let op_span = self.span_here();
            self.pos += 1;
            if self.peek().is_none() {
                return Err(ParseError::new("expected an expression after 'or'", op_span));
            }
            let rhs = self.and_expr()?;
            lhs = Expr::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.not_expr()?;
        loop {
            let is_and = matches!(self.peek(), Some(Tok::Amp)) || self.peek_keyword().as_deref() == Some("and");
            if !is_and {
                break;
            }
            let op_span = self.span_here();
            self.pos += 1;
            if self.peek().is_none() {
                return Err(ParseError::new("expected an expression after 'and'", op_span));
            }
            let rhs = self.not_expr()?;
            lhs = Expr::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn not_expr(&mut self) -> Result<Expr, ParseError> {
        let is_not = matches!(self.peek(), Some(Tok::Bang)) || self.peek_keyword().as_deref() == Some("not");
        if is_not {
            let op_span = self.span_here();
            self.pos += 1;
            if self.peek().is_none() {
                return Err(ParseError::new("expected an expression after 'not'", op_span));
            }
            return Ok(Expr::Not(Box::new(self.not_expr()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let span = self.span_here();

        if matches!(self.peek(), Some(Tok::LParen)) {
            self.pos += 1;
            let inner = self.expr()?;
            if !matches!(self.peek(), Some(Tok::RParen)) {
                return Err(ParseError::new("expected ')'", self.span_here()));
            }
            self.pos += 1;
            return Ok(inner);
        }

        // A quoted word is always a plain name, never a keyword.
        if let Some(Tok::Quoted(w)) = self.peek().cloned() {
            self.pos += 1;
            return Ok(Expr::Ident(w));
        }

        let Some(kw) = self.peek_keyword() else {
            return match self.peek().cloned() {
                Some(Tok::Num(n)) => Err(ParseError::new(
                    format!("'{n}' is a bare number, which is not a selection on its own"),
                    span,
                )
                .with_hint("say what it means: 'index 1-100' or 'resid 1-100'")),
                Some(other) => Err(ParseError::new(
                    format!("expected a selection, found {}", other.describe()),
                    span,
                )),
                None => Err(ParseError::new("expected a selection", self.eof)),
            };
        };

        match kw.as_str() {
            "all" => {
                self.pos += 1;
                Ok(Expr::All)
            }
            "none" => {
                self.pos += 1;
                Ok(Expr::None)
            }
            "selected" => {
                self.pos += 1;
                Ok(Expr::Selected)
            }
            "sp" => {
                self.pos += 1;
                Ok(Expr::Hybrid(Hybrid::Sp))
            }
            "sp2" => {
                self.pos += 1;
                Ok(Expr::Hybrid(Hybrid::Sp2))
            }
            "sp3" => {
                self.pos += 1;
                Ok(Expr::Hybrid(Hybrid::Sp3))
            }
            "resname" => {
                self.pos += 1;
                Ok(Expr::ResName(self.word_list("resname")?))
            }
            "name" => {
                self.pos += 1;
                Ok(Expr::Name(self.word_list("name")?))
            }
            "element" => {
                self.pos += 1;
                Ok(Expr::Element(self.word_list("element")?))
            }
            "resid" => {
                self.pos += 1;
                Ok(Expr::ResId(self.num_list("resid")?))
            }
            "index" => {
                self.pos += 1;
                Ok(Expr::Index(self.num_list("index")?))
            }
            "numbonds" => {
                self.pos += 1;
                Ok(Expr::NumBonds(self.count_spec("numbonds")?))
            }
            "with" => {
                self.pos += 1;
                let count = self.count_spec("with")?;
                if self.peek().is_none() {
                    return Err(ParseError::new(
                        "expected what to count after 'with'",
                        self.eof,
                    )
                    .with_hint("for example: with 2 H"));
                }
                let of = self.primary()?;
                Ok(Expr::With {
                    count,
                    of: Box::new(of),
                })
            }
            "within" | "of" => {
                self.pos += 1;
                Err(
                    ParseError::new(format!("'{kw}' is reserved but not implemented yet"), span)
                        .with_hint("distance selections are not available; use resid / index / resname"),
                )
            }
            // A reserved verb where a selection belongs is almost always a typo
            // like `DOM1 = show`, so name it rather than falling through to Ident.
            other if is_reserved(other) => Err(ParseError::new(
                format!("'{other}' is a command, not a selection"),
                span,
            )),
            _ => {
                let name = self.component_name("a name")?;
                Ok(Expr::Ident(name))
            }
        }
    }

    /// `WORD+`, stopping at any reserved word or non-word token.
    fn word_list(&mut self, kw: &str) -> Result<Vec<String>, ParseError> {
        let start = self.span_here();
        let mut words = Vec::new();
        loop {
            match self.peek().cloned() {
                Some(Tok::Word(w)) if !is_reserved(&w) => {
                    self.pos += 1;
                    words.push(w);
                }
                Some(Tok::Quoted(w)) | Some(Tok::Num(w)) => {
                    self.pos += 1;
                    words.push(w);
                }
                _ => break,
            }
        }
        if words.is_empty() {
            return Err(ParseError::new(
                format!("'{kw}' needs at least one name"),
                start,
            )
            .with_hint(format!("for example: {kw} SOL")));
        }
        Ok(words)
    }

    /// `num_item+`, where each item is `N` or `LO-HI`.
    fn num_list(&mut self, kw: &str) -> Result<Vec<NumRange>, ParseError> {
        let start = self.span_here();
        let mut ranges = Vec::new();
        while let Some(Tok::Num(raw)) = self.peek().cloned() {
            let span = self.span_here();
            self.pos += 1;
            ranges.push(parse_num_item(&raw, span)?);
        }
        if ranges.is_empty() {
            return Err(ParseError::new(
                format!("'{kw}' needs a number or a range"),
                start,
            )
            .with_hint(format!("for example: {kw} 1-100, or {kw} 1 5 9-12")));
        }
        Ok(ranges)
    }

    fn count_spec(&mut self, kw: &str) -> Result<CountSpec, ParseError> {
        let span = self.span_here();
        let cmp = match self.peek() {
            Some(Tok::Ge) => Some(0),
            Some(Tok::Le) => Some(1),
            Some(Tok::Gt) => Some(2),
            Some(Tok::Lt) => Some(3),
            _ => None,
        };
        if let Some(which) = cmp {
            self.pos += 1;
            let num_span = self.span_here();
            let Some(Tok::Num(raw)) = self.peek().cloned() else {
                return Err(ParseError::new(
                    "expected a number after the comparison",
                    num_span,
                ));
            };
            self.pos += 1;
            let value = raw.parse::<i64>().map_err(|_| {
                ParseError::new(
                    format!("'{raw}' is not a plain number"),
                    num_span,
                )
                .with_hint("a comparison takes a single count, as in 'with >=2 H'")
            })?;
            return Ok(match which {
                0 => CountSpec::Ge(value),
                1 => CountSpec::Le(value),
                2 => CountSpec::Gt(value),
                _ => CountSpec::Lt(value),
            });
        }

        let Some(Tok::Num(raw)) = self.peek().cloned() else {
            return Err(ParseError::new(
                format!("'{kw}' needs a count"),
                span,
            )
            .with_hint(format!("for example: {kw} 2, {kw} >=1, or {kw} 1-3")));
        };
        self.pos += 1;
        Ok(match parse_num_item(&raw, span)? {
            NumRange { lo, hi } if lo == hi => CountSpec::Exact(lo),
            NumRange { lo, hi } => CountSpec::Range(lo, hi),
        })
    }
}

/// Split a raw numeric run into an inclusive range.
///
/// The separating `-` is the first one at index >= 1, so a leading minus stays
/// attached to the lower bound: `-5--1` is -5..=-1, not a malformed split.
fn parse_num_item(raw: &str, span: Span) -> Result<NumRange, ParseError> {
    let bad = |msg: &str| ParseError::new(msg.to_string(), span);

    let sep = raw
        .char_indices()
        .skip(1)
        .find(|&(_, c)| c == '-')
        .map(|(i, _)| i);

    match sep {
        None => {
            let v = raw.parse::<i64>().map_err(|_| bad("not a number"))?;
            Ok(NumRange { lo: v, hi: v })
        }
        Some(i) => {
            let lo = raw[..i].parse::<i64>().map_err(|_| bad("not a number"))?;
            let hi = raw[i + 1..]
                .parse::<i64>()
                .map_err(|_| bad("not a number"))?;
            if lo > hi {
                return Err(ParseError::new(
                    format!("range {lo}-{hi} runs backwards"),
                    span,
                )
                .with_hint("write the smaller bound first"));
            }
            Ok(NumRange { lo, hi })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(input: &str) -> Expr {
        match parse_statement(input).unwrap() {
            Statement::Count(e) => e,
            other => panic!("expected a bare expression, got {other:?}"),
        }
    }

    fn ident(name: &str) -> Expr {
        Expr::Ident(name.to_string())
    }

    #[test]
    fn and_binds_tighter_than_or() {
        assert_eq!(
            expr("a or b and c"),
            Expr::Or(
                Box::new(ident("a")),
                Box::new(Expr::And(Box::new(ident("b")), Box::new(ident("c"))))
            )
        );
    }

    #[test]
    fn not_binds_tighter_than_and_and_nests() {
        assert_eq!(
            expr("not a and b"),
            Expr::And(
                Box::new(Expr::Not(Box::new(ident("a")))),
                Box::new(ident("b"))
            )
        );
        assert_eq!(
            expr("not not a"),
            Expr::Not(Box::new(Expr::Not(Box::new(ident("a")))))
        );
    }

    #[test]
    fn parentheses_override_precedence() {
        assert_eq!(
            expr("(a or b) and c"),
            Expr::And(
                Box::new(Expr::Or(Box::new(ident("a")), Box::new(ident("b")))),
                Box::new(ident("c"))
            )
        );
    }

    #[test]
    fn symbolic_operators_match_their_words() {
        assert_eq!(expr("a & b"), expr("a and b"));
        assert_eq!(expr("a | b"), expr("a or b"));
        assert_eq!(expr("!a"), expr("not a"));
    }

    #[test]
    fn word_lists_stop_at_keywords() {
        assert_eq!(
            expr("resname SOL NA and sp3"),
            Expr::And(
                Box::new(Expr::ResName(vec!["SOL".into(), "NA".into()])),
                Box::new(Expr::Hybrid(Hybrid::Sp3))
            )
        );
    }

    #[test]
    fn num_lists_take_several_items() {
        assert_eq!(
            expr("resid 1 5 9-12"),
            Expr::ResId(vec![
                NumRange { lo: 1, hi: 1 },
                NumRange { lo: 5, hi: 5 },
                NumRange { lo: 9, hi: 12 },
            ])
        );
    }

    #[test]
    fn negative_residue_ranges() {
        assert_eq!(expr("resid -5--1"), Expr::ResId(vec![NumRange { lo: -5, hi: -1 }]));
    }

    #[test]
    fn with_takes_a_primary_not_the_whole_rest() {
        // `with 2 H and sp3` must be `(with 2 H) and sp3`, otherwise the count
        // predicate would swallow the conjunction.
        assert_eq!(
            expr("with 2 H and sp3"),
            Expr::And(
                Box::new(Expr::With {
                    count: CountSpec::Exact(2),
                    of: Box::new(ident("H")),
                }),
                Box::new(Expr::Hybrid(Hybrid::Sp3))
            )
        );
    }

    #[test]
    fn with_accepts_comparisons_and_nested_expressions() {
        assert_eq!(
            expr("with >=1 (element O or element N)"),
            Expr::With {
                count: CountSpec::Ge(1),
                of: Box::new(Expr::Or(
                    Box::new(Expr::Element(vec!["O".into()])),
                    Box::new(Expr::Element(vec!["N".into()])),
                )),
            }
        );
    }

    #[test]
    fn the_methylene_example_parses() {
        let parsed = parse_statement("CH2 = element C and sp3 and with 2 H").unwrap();
        let Statement::Assign { name, expr } = parsed else {
            panic!("expected an assignment");
        };
        assert_eq!(name, "CH2");
        // Left-associative: ((element C and sp3) and with 2 H)
        assert_eq!(
            expr,
            Expr::And(
                Box::new(Expr::And(
                    Box::new(Expr::Element(vec!["C".into()])),
                    Box::new(Expr::Hybrid(Hybrid::Sp3)),
                )),
                Box::new(Expr::With {
                    count: CountSpec::Exact(2),
                    of: Box::new(ident("H")),
                }),
            )
        );
    }

    #[test]
    fn verbs_parse() {
        assert_eq!(
            parse_statement("show DOM1 TAIL").unwrap(),
            Statement::Show(Targets::Named(vec!["DOM1".into(), "TAIL".into()]))
        );
        assert_eq!(parse_statement("hide all").unwrap(), Statement::Hide(Targets::All));
        assert_eq!(parse_statement("only PROT").unwrap(), Statement::Only("PROT".into()));
        assert_eq!(parse_statement("del DOM1").unwrap(), Statement::Del(vec!["DOM1".into()]));
        assert_eq!(
            parse_statement("rename DOM1 NTERM").unwrap(),
            Statement::Rename {
                from: "DOM1".into(),
                to: "NTERM".into()
            }
        );
        assert_eq!(parse_statement("list").unwrap(), Statement::List);
        assert_eq!(parse_statement("reset").unwrap(), Statement::Reset);
    }

    #[test]
    fn hide_all_names_a_component_called_all_when_more_words_follow() {
        // `hide all` is the bulk verb, but `hide all SOL` can only mean two
        // component names, so the bulk form requires `all` to stand alone.
        assert_eq!(
            parse_statement("hide all SOL").unwrap(),
            Statement::Hide(Targets::Named(vec!["all".into(), "SOL".into()]))
        );
    }

    #[test]
    fn error_spans_point_at_the_offending_token() {
        let err = parse_statement("DOM1 = PROT and resid 1-").unwrap_err();
        assert_eq!(err.span.start, 22, "caret sits under the '1-' token");
        assert!(err.message.contains("resid"), "{}", err.message);

        let err = parse_statement("DOM1 = PROT and").unwrap_err();
        assert!(err.message.contains("after 'and'"), "{}", err.message);

        let err = parse_statement("DOM1 = (a or b").unwrap_err();
        assert!(err.message.contains("')'"), "{}", err.message);
    }

    #[test]
    fn reserved_words_cannot_name_components() {
        assert!(parse_statement("all = PROT").is_err());
        assert!(parse_statement("index = PROT").is_err());
        // ...but quoting escapes the check.
        assert!(parse_statement("\"all\" = PROT").is_ok());
    }

    #[test]
    fn backwards_ranges_are_rejected() {
        let err = parse_statement("resid 100-1").unwrap_err();
        assert!(err.message.contains("backwards"), "{}", err.message);
    }

    #[test]
    fn bare_numbers_need_a_keyword() {
        let err = parse_statement("DOM1 = 1-100").unwrap_err();
        assert!(err.hint.as_deref().unwrap_or("").contains("index"), "{err:?}");
    }

    #[test]
    fn trailing_junk_is_reported() {
        let err = parse_statement("list extra").unwrap_err();
        assert!(err.message.contains("unexpected"), "{}", err.message);
    }

    #[test]
    fn within_is_reserved_with_a_clear_message() {
        let err = parse_statement("DOM1 = within 5 of PROT").unwrap_err();
        assert!(err.message.contains("not implemented"), "{}", err.message);
    }

    #[test]
    fn rendered_errors_put_the_caret_under_the_span() {
        let input = "DOM1 = PROT and resid 1-";
        let err = parse_statement(input).unwrap_err();

        let rendered = err.render(input);
        let caret_line = rendered.lines().nth(1).unwrap();
        assert_eq!(caret_line.find('^').unwrap() - 2, 22); // strip the gutter

        // `caret` drops the echoed input but keeps the same gutter, so it lines
        // up under the command log's own `> {line}` echo.
        let caret_only = err.caret(input);
        assert_eq!(caret_only.lines().next().unwrap().find('^').unwrap() - 2, 22);
        assert!(caret_only.lines().any(|l| l.contains("hint:")));
        assert_eq!(rendered.lines().skip(1).count(), caret_only.lines().count());
    }

    #[test]
    fn multibyte_input_does_not_shift_the_caret() {
        // Byte offsets index the input, but the caret counts characters.
        let input = "\"日本語\" = resid 1-";
        let err = parse_statement(input).unwrap_err();
        let caret_col = err.caret(input).lines().next().unwrap().find('^').unwrap() - 2;
        assert_eq!(caret_col, input.chars().count() - 2, "under the '1-' token");
    }
}
