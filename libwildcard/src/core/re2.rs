// SPDX-License-Identifier: Apache-2.0
//! RE2 portability.
//!
//! Patterns are written in the RE2 dialect, because ClickHouse (`match`),
//! pg_re2 and Hyperscan-backed `multiMatch*` are the execution engines. This
//! module
//! - rejects constructs RE2 does not support (so a pattern accepted here runs
//!   unchanged in ClickHouse), and
//! - produces the equivalent pattern for the Rust `regex` crate, whose
//!   defaults differ: we make `\d \w \s \b` ASCII-only and `.` match newline,
//!   exactly as ClickHouse does.

use regex_syntax::ast::{
    self, Assertion, AssertionKind, Ast, ClassPerl, ClassPerlKind, ClassSetBinaryOp, ClassSetItem,
    Flag, Visitor,
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum Re2Error {
    #[error(transparent)]
    TooLong(#[from] crate::core::pattern::PatternTooLong),
    #[error("invalid regex: {0}")]
    Syntax(String),
    #[error("not portable to RE2/ClickHouse: {0}")]
    NotPortable(String),
}

struct Collect<'p> {
    pattern: &'p str,
    edits: Vec<(usize, usize, String)>,
    issues: Vec<String>,
    class_depth: usize,
}

fn perl_ascii(kind: &ClassPerlKind, negated: bool, in_class: bool) -> Option<String> {
    let set = match kind {
        ClassPerlKind::Digit => "0-9",
        ClassPerlKind::Word => "0-9A-Za-z_",
        ClassPerlKind::Space => r"\t\n\x0C\r ",
    };
    match (negated, in_class) {
        (false, true) => Some(set.to_string()),
        (false, false) => Some(format!("[{set}]")),
        (true, false) => Some(format!("[^{set}]")),
        (true, true) => None,
    }
}

impl Visitor for Collect<'_> {
    type Output = (Vec<(usize, usize, String)>, Vec<String>);
    type Err = ();

    fn finish(self) -> Result<Self::Output, ()> {
        Ok((self.edits, self.issues))
    }

    fn visit_pre(&mut self, a: &Ast) -> Result<(), ()> {
        match a {
            Ast::ClassPerl(p) => {
                let ClassPerl {
                    span,
                    kind,
                    negated,
                } = p.as_ref();
                let r = perl_ascii(kind, *negated, false).ok_or(())?;
                self.edits.push((span.start.offset, span.end.offset, r));
            }
            Ast::Assertion(x) => {
                let Assertion { span, kind } = x.as_ref();
                match kind {
                    AssertionKind::WordBoundary => {
                        self.edits
                            .push((span.start.offset, span.end.offset, r"(?-u:\b)".into()))
                    }
                    AssertionKind::NotWordBoundary => {
                        self.edits
                            .push((span.start.offset, span.end.offset, r"(?-u:\B)".into()))
                    }
                    AssertionKind::StartLine
                    | AssertionKind::EndLine
                    | AssertionKind::StartText
                    | AssertionKind::EndText => {}
                    other => self.issues.push(format!(
                        "assertion {other:?} (`{}`)",
                        &self.pattern[span.start.offset..span.end.offset]
                    )),
                }
            }
            Ast::Flags(f) => self.check_flags(&f.flags),
            Ast::Group(g) => {
                if let ast::GroupKind::NonCapturing(flags) = &g.kind {
                    self.check_flags(flags);
                }
            }
            Ast::ClassBracketed(_) => {
                self.class_depth += 1;
                if self.class_depth > 1 {
                    self.issues.push("nested character class".into());
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn visit_post(&mut self, a: &Ast) -> Result<(), ()> {
        if let Ast::ClassBracketed(_) = a {
            self.class_depth -= 1;
        }
        Ok(())
    }

    fn visit_class_set_item_pre(&mut self, item: &ClassSetItem) -> Result<(), ()> {
        match item {
            ClassSetItem::Perl(p) => match perl_ascii(&p.kind, p.negated, true) {
                Some(r) => self.edits.push((p.span.start.offset, p.span.end.offset, r)),
                None => self
                    .issues
                    .push("negated Perl class (\\D \\W \\S) inside brackets".into()),
            },
            ClassSetItem::Bracketed(_) => self.issues.push("nested character class".into()),
            _ => {}
        }
        Ok(())
    }

    fn visit_class_set_binary_op_pre(&mut self, _op: &ClassSetBinaryOp) -> Result<(), ()> {
        self.issues.push("class set operation (&&, --, ~~)".into());
        Ok(())
    }
}

impl Collect<'_> {
    fn check_flags(&mut self, flags: &ast::Flags) {
        for item in &flags.items {
            if let ast::FlagsItemKind::Flag(f) = &item.kind {
                match f {
                    Flag::CaseInsensitive
                    | Flag::MultiLine
                    | Flag::DotMatchesNewLine
                    | Flag::SwapGreed => {}
                    other => self.issues.push(format!("flag {other:?}")),
                }
            }
        }
    }
}

/// Validate an RE2-dialect pattern and return its Rust-regex equivalent
/// (to be compiled with `unicode(true)` on bytes).
pub fn to_rust(pattern: &str) -> Result<String, Re2Error> {
    crate::core::pattern::check_pattern_len(pattern.as_bytes())?;
    if pattern.contains(r"\Q") || pattern.contains(r"\C") {
        return Err(Re2Error::NotPortable(
            r"\Q...\E and \C are not supported".into(),
        ));
    }
    let ast = ast::parse::ParserBuilder::new()
        .build()
        .parse(pattern)
        .map_err(|e| Re2Error::Syntax(e.to_string()))?;
    let c = Collect {
        pattern,
        edits: Vec::new(),
        issues: Vec::new(),
        class_depth: 0,
    };
    let (mut edits, mut issues) = ast::visit(&ast, c).unwrap_or_default();
    issues.dedup();
    if !issues.is_empty() {
        return Err(Re2Error::NotPortable(issues.join("; ")));
    }
    edits.sort_by_key(|e| e.0);
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut pos = 0;
    for (s, e, r) in edits {
        out.push_str(&pattern[pos..s]);
        out.push_str(&r);
        pos = e;
    }
    out.push_str(&pattern[pos..]);
    // ClickHouse compiles RE2 with dot_nl: `.` matches `\n` unless `(?-s)`.
    Ok(format!("(?s:{out})"))
}

/// List portability problems (empty when the pattern is portable).
pub fn check(pattern: &str) -> Vec<String> {
    match to_rust(pattern) {
        Ok(_) => Vec::new(),
        Err(e) => vec![e.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_perl_classes_to_ascii() {
        assert_eq!(
            to_rust(r"\d+\w\s").unwrap(),
            r"(?s:[0-9]+[0-9A-Za-z_][\t\n\x0C\r ])"
        );
        assert_eq!(to_rust(r"[\d_]").unwrap(), r"(?s:[0-9_])");
        assert_eq!(to_rust(r"\bcmd\B").unwrap(), r"(?s:(?-u:\b)cmd(?-u:\B))");
        assert_eq!(to_rust(r"\D").unwrap(), r"(?s:[^0-9])");
    }

    #[test]
    fn rejects_non_re2() {
        assert!(matches!(
            to_rust(r"[a-z&&[^aeiou]]"),
            Err(Re2Error::NotPortable(_))
        ));
        assert!(matches!(to_rust(r"[\D]"), Err(Re2Error::NotPortable(_))));
        assert!(matches!(to_rust(r"(?x)a b"), Err(Re2Error::NotPortable(_))));
        assert!(matches!(
            to_rust(r"\b{start}a"),
            Err(Re2Error::NotPortable(_))
        ));
        assert!(matches!(to_rust(r"\Qa.b\E"), Err(Re2Error::NotPortable(_))));
        assert!(matches!(to_rust("("), Err(Re2Error::Syntax(_))));
        assert!(check(r"(?i)powershell(\.exe)?\s+-e").is_empty());
    }
}
