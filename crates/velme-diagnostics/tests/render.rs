//! The renderers (`compiler/20` R-CMP-15, `tooling/40` R-CLI-08, R-CLI-17, D-68).

use velme_diagnostics::render::{self, JsonDiagnostic, JsonSpan, MAX_SHOWN};
use velme_diagnostics::{Code, Diagnostic, Span};

fn diag(code: Code, start: usize, end: usize, message: &str) -> Diagnostic {
    Diagnostic::new(code, Span::new(start, end), message)
}

#[test]
fn json_span_counts_lines_and_scalar_values() {
    // `é` is two bytes and `👋` four, but each is one column (D-68); `\r\n` ends a line like `\n` (R-SYN-02).
    let text = "a\r\né👋x\nlast";
    let at = |start| {
        let span = JsonSpan::new(text, Span::new(start, start + 1));
        (span.line, span.column)
    };
    assert_eq!(at(0), (1, 1));
    assert_eq!(at(3), (2, 1));
    assert_eq!(at(9), (2, 3));
    assert_eq!(at(11), (3, 1));
    // Past the end, and with no text at all, it stays on a real position.
    assert_eq!(at(100), (3, 5));
    assert_eq!(
        (
            JsonSpan::new("", Span::new(0, 0)).line,
            JsonSpan::new("", Span::new(0, 0)).column
        ),
        (1, 1)
    );
}

#[test]
fn json_diagnostic_matches_r_cli_08() {
    let d = diag(Code::UnknownType, 6, 11, "I don't know a type called Playr.")
        .with_label(Span::new(0, 4), "in this type")
        .with_note("types are declared with `type`")
        .with_help("did you mean `Player`?");
    let value = serde_json::to_value(JsonDiagnostic::new(&d, "game.velme", "type Playr:\n")).unwrap();
    insta::assert_json_snapshot!(value, @r#"
    {
      "code": "VL0201",
      "file": "game.velme",
      "help": "did you mean `Player`?",
      "labels": [
        {
          "span": {
            "column": 1,
            "end": 4,
            "line": 1,
            "start": 0
          },
          "text": "in this type"
        }
      ],
      "message": "I don't know a type called Playr.",
      "notes": [
        "types are declared with `type`"
      ],
      "severity": "error",
      "span": {
        "column": 7,
        "end": 11,
        "line": 1,
        "start": 6
      }
    }
    "#);
    let no_help = serde_json::to_value(JsonDiagnostic::new(&diag(Code::TabIndentation, 0, 1, "m"), "f", "\t")).unwrap();
    assert!(no_help.get("help").is_none());
}

#[test]
fn ac_err_03_headline_ends_with_the_code() {
    let text = "goal A() -> Text:\n    plan: \"x\"\n";
    let diags = [
        diag(Code::UnexpectedToken, 5, 6, "I didn't expect `A` here.").with_help("a hint"),
        diag(Code::LintWarning, 5, 6, "`A` works, but Velme style writes it `Ab`."),
    ];
    let out = render::render_human(&diags, "a.velme", Some(text), false);
    let headlines: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("Error:") || l.starts_with("Warning:"))
        .collect();
    assert_eq!(
        headlines,
        [
            "Error: I didn't expect `A` here.  [VL0101]",
            "Warning: `A` works, but Velme style writes it `Ab`.  [VL0107]",
        ]
    );
    insta::assert_snapshot!(out);
}

#[test]
fn human_output_without_source() {
    let d = diag(Code::FileError, 0, 0, "I couldn't find `a.velme`.")
        .with_note("os says no")
        .with_help("check it");
    insta::assert_snapshot!(render::render_human(&[d], "a.velme", None, false), @r"
    Error: I couldn't find `a.velme`.  [VL0901]
       ╭─[ a.velme ]
       │ Note: os says no
       │ Help: check it
    ───╯
    ");
}

#[test]
fn human_output_stops_after_the_cap() {
    let text = "x\n".repeat(30);
    let diags: Vec<_> = (0..25)
        .map(|i| diag(Code::UnexpectedToken, i * 2, i * 2 + 1, "Bad."))
        .collect();
    let out = render::render_human(&diags, "a.velme", Some(&text), false);
    assert_eq!(out.lines().filter(|l| l.starts_with("Error:")).count(), MAX_SHOWN);
    assert_eq!(out.lines().last(), Some("…and 5 more"));
    let exact = render::render_human(&diags[..MAX_SHOWN], "a.velme", Some(&text), false);
    assert!(!exact.contains("more"));
}

/// R-CLI-17: no control or bidi character from a message or from the quoted source reaches the terminal.
#[test]
fn human_output_escapes_control_and_bidi_characters() {
    let text = "plan: \"\u{1b}]52;c;x\u{7} \u{202e}\"\r\nx\ry\n";
    let diags = [
        diag(Code::UnexpectedToken, 7, 8, "I didn't expect `\u{1b}` here.")
            .with_label(Span::new(17, 20), "the \u{202e} is here")
            .with_help("remove \u{9b}it"),
        diag(Code::UnexpectedToken, 24, 25, "Lone carriage return."),
    ];
    let out = render::render_human(&diags, "a\u{1b}.velme", Some(text), false);
    assert!(
        !out.chars()
            .any(|c| (c.is_control() && c != '\n') || ('\u{202a}'..='\u{202e}').contains(&c)),
        "{out:?}"
    );
    assert!(out.contains("I didn't expect `\\u{1b}` here."));
    assert!(out.contains("the \\u{202e} is here"));
    assert!(out.contains("remove \\u{9b}it"));
    // `x\ry` stays one line, as Velme counts it.
    assert!(out.contains("a\\u{1b}.velme:2:2"), "{out}");
}

#[test]
fn sort_orders_by_start_then_code() {
    let mut diags = vec![
        diag(Code::ReservedWord, 5, 6, "b"),
        diag(Code::UnexpectedToken, 5, 9, "a"),
        diag(Code::TabIndentation, 0, 1, "c"),
    ];
    render::sort(&mut diags);
    let order: Vec<_> = diags.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(order, ["c", "a", "b"]);
}
