//! Lexer and layout pass: golden token streams (CC-TEST-02) and the `language/10` criteria they own.

use velme_diagnostics::{Code, Diagnostic};
use velme_syntax::{SourceFile, Token, TokenKind, lex};

fn render(tokens: &[Token], diags: &[Diagnostic]) -> String {
    let mut out = String::new();
    for t in tokens {
        let kind = match &t.kind {
            TokenKind::Name(n) => format!("name {n}"),
            TokenKind::Keyword(k) => format!("keyword {k}"),
            TokenKind::Number(n) => format!("number {n}"),
            TokenKind::Unit(u) => format!("unit {}", u.as_str()),
            TokenKind::Text(s) => format!("text {s:?}"),
            TokenKind::BlockText(s) => format!("block_text {s:?}"),
            TokenKind::Punct(p) => format!("punct {}", p.as_str()),
            TokenKind::Newline => "NEWLINE".into(),
            TokenKind::Indent => "INDENT".into(),
            TokenKind::Dedent => "DEDENT".into(),
        };
        out.push_str(&format!("{}..{} {kind}\n", t.span.start, t.span.end));
    }
    for d in diags {
        out.push_str(&format!(
            "{}[{}] {}..{}: {}\n",
            d.severity_word(),
            d.code.as_str(),
            d.span.start,
            d.span.end,
            d.message
        ));
    }
    out
}

trait SeverityWord {
    fn severity_word(&self) -> &'static str;
}

impl SeverityWord for Diagnostic {
    fn severity_word(&self) -> &'static str {
        if self.is_error() { "error" } else { "warning" }
    }
}

fn lex_str(text: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    lex(&SourceFile::new("test.velme", text))
}

fn codes(text: &str) -> Vec<Code> {
    lex_str(text).1.iter().map(|d| d.code).collect()
}

fn kinds(text: &str) -> Vec<TokenKind> {
    lex_str(text).0.into_iter().map(|t| t.kind).collect()
}

#[test]
fn golden_token_streams() {
    insta::glob!("../../../tests/golden/lexer", "*.velme", |path| {
        let bytes = std::fs::read(path).expect("golden file is readable");
        let file = SourceFile::from_bytes("golden.velme", bytes).expect("golden file is UTF-8");
        let (tokens, diags) = lex(&file);
        insta::assert_snapshot!(render(&tokens, &diags));
    });
}

#[test]
fn ac_syn_02_tab_in_leading_whitespace_is_vl0103_and_lexing_continues() {
    let src = "goal A() -> Number:\n\tplan: \"x\"\n    check:\n        - result == 1\n";
    let (tokens, diags) = lex_str(src);
    assert_eq!(diags.iter().map(|d| d.code).collect::<Vec<_>>(), [Code::TabIndentation]);
    assert_eq!(diags[0].span.start, src.find('\t').unwrap());
    assert!(
        tokens.iter().any(|t| t.kind == TokenKind::Number("1".into())),
        "the rest of the file is lexed"
    );
}

#[test]
fn ac_syn_03_dedent_to_width_not_on_stack_is_vl0102() {
    let src = "goal A() -> Number:\n        plan: \"x\"\n    check:\n        - result == 1\n";
    assert_eq!(codes(src), [Code::InconsistentIndentation]);
}

#[test]
fn ac_syn_04_block_plan_keeps_blank_lines_and_hashes_and_strips_common_indent() {
    let src = "goal A() -> Text:\n    plan: |\n        One.\n\n        # still text\n          two\n    check:\n        - result is not empty\n";
    let (tokens, diags) = lex_str(src);
    assert!(diags.is_empty(), "{diags:?}");
    let block: Vec<&str> = tokens
        .iter()
        .filter_map(|t| match &t.kind {
            TokenKind::BlockText(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(block, ["One.\n\n# still text\n  two"]);
    // The block ends at `check:`, which is at the `plan` column and lexes normally.
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Keyword(velme_syntax::Keyword::Check))
    );
}

#[test]
fn ac_syn_06_unterminated_text_is_vl0105_and_unknown_escape_is_vl0101() {
    assert_eq!(codes("plan: \"abc\n"), [Code::UnterminatedText]);
    assert_eq!(codes("plan: \"\\q\"\n"), [Code::UnexpectedToken]);
    assert_eq!(
        codes("plan: \"\\u{D800}\"\n"),
        [Code::UnexpectedToken],
        "a surrogate is not a scalar value"
    );
    assert_eq!(
        codes("plan: \"\\u{1234567}\"\n"),
        [Code::UnexpectedToken],
        "at most 6 hex digits"
    );
    assert_eq!(kinds("\"\\u{41}\\t\"\n")[0], TokenKind::Text("A\t".into()));
}

#[test]
fn ac_syn_14_bom_is_ignored_and_invalid_utf8_is_vl0901() {
    let with_bom = SourceFile::from_bytes("a.velme", b"\xef\xbb\xbfgoal A() -> Number:\n".to_vec()).unwrap();
    let (tokens, diags) = lex(&with_bom);
    assert!(diags.is_empty());
    // Spans are file byte offsets: `goal` starts after the 3-byte BOM (D-75).
    assert_eq!(tokens[0].span.start, 3);
    let err = SourceFile::from_bytes("a.velme", b"goal \xff".to_vec()).unwrap_err();
    assert_eq!(err.code, Code::FileError);
}

#[test]
fn ac_syn_15_lone_cr_is_vl0101_and_crlf_lexes_like_lf() {
    assert_eq!(codes("goal A() -> Number:\r    plan: \"x\"\n"), [Code::UnexpectedToken]);
    // A lone CR is never hidden by a comment, and still ends the line (reported once per file).
    let mac = "# old line endings\rgoal A() -> Number:\r    plan: \"x\"\r";
    assert_eq!(codes(mac), [Code::UnexpectedToken]);
    assert!(kinds(mac).contains(&TokenKind::Keyword(velme_syntax::Keyword::Goal)));
    assert_eq!(
        codes("plan: |\n        text\n   \r"),
        [Code::UnexpectedToken],
        "inside a block scalar too"
    );
    let lf = "goal A() -> Number:\n    plan: |\n        text\n    check:\n        - result == 1\n";
    assert_eq!(kinds(&lf.replace('\n', "\r\n")), kinds(lf));
}

#[test]
fn ac_syn_11_newlines_inside_brackets_are_joined() {
    let joined = "goal A(a: Number, b: Number) -> Number:\n    plan: \"x\"\n";
    let split = "goal A(\n    a: Number,\n        b: Number) -> Number:\n    plan: \"x\"\n";
    assert_eq!(kinds(split), kinds(joined));
}

/// D-21: plan lines lose trailing spaces and tabs only; other whitespace is text.
#[test]
fn ac_syn_04_block_plan_trims_only_spaces_and_tabs() {
    let (tokens, diags) = lex_str("plan: |\n    One \t\n    two\u{a0}\u{3000}\n");
    assert!(diags.is_empty(), "{diags:?}");
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::BlockText("One\ntwo\u{a0}\u{3000}".into()))
    );
}

/// R-SYN-22: bidi controls warn in comments too, including U+061C, U+200E and U+200F.
#[test]
fn bidi_controls_in_comments_and_text_are_lint_warnings() {
    for c in ['\u{61c}', '\u{200e}', '\u{200f}', '\u{202e}', '\u{2067}'] {
        assert_eq!(
            codes(&format!("# a{c}b\n")),
            [Code::LintWarning],
            "{c:?} on a comment line"
        );
        assert_eq!(
            codes(&format!("plan: \"x\" # a{c}b\n")),
            [Code::LintWarning],
            "{c:?} after code"
        );
        assert_eq!(
            codes(&format!("plan: \"a{c}b\"\n")),
            [Code::LintWarning],
            "{c:?} in text"
        );
    }
}

/// D-76: a run of characters Velme can't read is one error.
#[test]
fn a_run_of_unreadable_characters_is_one_error() {
    let (tokens, diags) = lex_str("x @@~ y\n");
    assert_eq!(
        diags
            .iter()
            .map(|d| (d.code, d.span.start, d.span.end))
            .collect::<Vec<_>>(),
        [(Code::UnexpectedToken, 2, 5)]
    );
    assert_eq!(
        tokens.iter().filter(|t| matches!(t.kind, TokenKind::Name(_))).count(),
        2
    );
}
