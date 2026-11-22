// SPDX-License-Identifier: Apache-2.0
//! RE2 portability.
//!
//! Patterns are written in the RE2 dialect used by ClickHouse (`match`)
//! and pg_re2. Hyperscan has different semantics and is not a verifier. This
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
    repetition_factors: Vec<u32>,
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
        self.repetition_factors.push(1);
        match a {
            Ast::Literal(literal) => self.check_literal(literal),
            Ast::ClassUnicode(class) => self.check_unicode_class(class),
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
        // RE2 bounds the product of nested counted repetitions, including
        // across groups, alternations and unbounded repetitions. Post-order
        // checking also rejects an invalid inner repeat beneath an outer {0}.
        // A zero repeat does not hide inner factors from another outer repeat.
        let mut factor = self.repetition_factors.pop().unwrap_or(1);
        if let Ast::Repetition(repetition) = a
            && let ast::RepetitionKind::Range(range) = &repetition.op.kind
        {
            let count = match *range {
                ast::RepetitionRange::Exactly(n) | ast::RepetitionRange::AtLeast(n) => n,
                ast::RepetitionRange::Bounded(_, max) => max,
            };
            factor = factor.saturating_mul(count.max(1));
            if factor > 1000 {
                self.issues
                    .push("counted repetition exceeds RE2's limit of 1000".into());
            }
        }
        if let Some(parent) = self.repetition_factors.last_mut() {
            *parent = (*parent).max(factor);
        }
        Ok(())
    }

    fn visit_class_set_item_pre(&mut self, item: &ClassSetItem) -> Result<(), ()> {
        match item {
            ClassSetItem::Literal(literal) => self.check_literal(literal),
            ClassSetItem::Range(range) => {
                self.check_literal(&range.start);
                self.check_literal(&range.end);
            }
            ClassSetItem::Unicode(class) => self.check_unicode_class(class),
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

// RE2's documented, case-sensitive general-category and script names.
// Source: https://github.com/google/re2/wiki/Syntax (Unicode 15 script list).
// `Any` is additionally supported by ClickHouse 26.9's RE2. Rust accepts many
// aliases and binary properties that RE2 rejects, so do not normalize names.
const RE2_UNICODE_CLASSES: &str = "
Any C Cc Cf Co Cs L Ll Lm Lo Lt Lu M Mc Me Mn N Nd Nl No P Pc Pd Pe Pf Pi Po Ps
S Sc Sk Sm So Z Zl Zp Zs
Adlam Ahom Anatolian_Hieroglyphs Arabic Armenian Avestan Balinese Bamum Bassa_Vah
Batak Bengali Bhaiksuki Bopomofo Brahmi Braille Buginese Buhid Canadian_Aboriginal
Carian Caucasian_Albanian Chakma Cham Cherokee Chorasmian Common Coptic Cuneiform
Cypriot Cypro_Minoan Cyrillic Deseret Devanagari Dives_Akuru Dogra Duployan
Egyptian_Hieroglyphs Elbasan Elymaic Ethiopic Georgian Glagolitic Gothic Grantha
Greek Gujarati Gunjala_Gondi Gurmukhi Han Hangul Hanifi_Rohingya Hanunoo Hatran
Hebrew Hiragana Imperial_Aramaic Inherited Inscriptional_Pahlavi
Inscriptional_Parthian Javanese Kaithi Kannada Katakana Kawi Kayah_Li Kharoshthi
Khitan_Small_Script Khmer Khojki Khudawadi Lao Latin Lepcha Limbu Linear_A Linear_B
Lisu Lycian Lydian Mahajani Makasar Malayalam Mandaic Manichaean Marchen
Masaram_Gondi Medefaidrin Meetei_Mayek Mende_Kikakui Meroitic_Cursive
Meroitic_Hieroglyphs Miao Modi Mongolian Mro Multani Myanmar Nabataean Nag_Mundari
Nandinagari New_Tai_Lue Newa Nko Nushu Nyiakeng_Puachue_Hmong Ogham Ol_Chiki
Old_Hungarian Old_Italic Old_North_Arabian Old_Permic Old_Persian Old_Sogdian
Old_South_Arabian Old_Turkic Old_Uyghur Oriya Osage Osmanya Pahawh_Hmong Palmyrene
Pau_Cin_Hau Phags_Pa Phoenician Psalter_Pahlavi Rejang Runic Samaritan Saurashtra
Sharada Shavian Siddham SignWriting Sinhala Sogdian Sora_Sompeng Soyombo Sundanese
Syloti_Nagri Syriac Tagalog Tagbanwa Tai_Le Tai_Tham Tai_Viet Takri Tamil Tangsa
Tangut Telugu Thaana Thai Tibetan Tifinagh Tirhuta Toto Ugaritic Vai Vithkuqi
Wancho Warang_Citi Yezidi Yi Zanabazar_Square
";

impl Collect<'_> {
    fn check_literal(&mut self, literal: &ast::Literal) {
        if matches!(
            literal.kind,
            ast::LiteralKind::HexFixed(
                ast::HexLiteralKind::UnicodeShort | ast::HexLiteralKind::UnicodeLong
            ) | ast::LiteralKind::HexBrace(
                ast::HexLiteralKind::UnicodeShort | ast::HexLiteralKind::UnicodeLong
            )
        ) {
            self.issues
                .push(r"Unicode escapes \u and \U (use \x{...})".into());
        }
    }

    fn check_unicode_class(&mut self, class: &ast::ClassUnicode) {
        let supported = match &class.kind {
            ast::ClassUnicodeKind::OneLetter(c) => "CLMNPSZ".contains(*c),
            ast::ClassUnicodeKind::Named(name) => RE2_UNICODE_CLASSES
                .split_ascii_whitespace()
                .any(|known| known == name),
            ast::ClassUnicodeKind::NamedValue { .. } => false,
        };
        if !supported {
            self.issues
                .push("Unicode class outside RE2 general categories and scripts".into());
        }
    }

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
        repetition_factors: Vec::new(),
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
    fn rejects_rust_only_unicode_and_counted_repetitions() {
        for pattern in [
            r"\p{Emoji}",
            r"[\p{Emoji}]",
            r"\p{Script=Greek}",
            r"\p{greek}",
            r"\u0041",
            r"[\u0041-Z]",
            r"\U00000041",
            r"\u{41}",
            "a{1001}",
            "a{0,1001}",
            "a{1001,}",
            "(a{1000}){8}",
            "(a{1000}|b){2}",
            "((a{1000})*){2}",
            "((a{1000}){2}){0}",
            "((a{1000}){0}){2}",
            "((a{1000}){0,}){2}",
        ] {
            assert!(
                matches!(to_rust(pattern), Err(Re2Error::NotPortable(_))),
                "{pattern}"
            );
        }
        for pattern in [
            r"\p{Greek}",
            r"[\p{Ll}\pN]",
            r"\P{Any}",
            r"\x41",
            r"\x{41}",
            r"\\u0041",
            "a{1000}",
            "(a{100}){10}",
            "(a{1000})+",
            "(a{1000}){0}",
            "a{1000}b{1000}",
            "(a{1000}){1,}",
        ] {
            assert!(to_rust(pattern).is_ok(), "{pattern}");
        }
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
