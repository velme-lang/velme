//! Semantic analysis: the golden HIR corpus (R-CMP-12) and the criteria of `language/11..13` it owns. Rejecting golden
//! files are snapshotted as `velme check --json` in the CLI tests (AC-CMP-04).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use velme_diagnostics::{Code, Diagnostic};
use velme_sema::analyze;
use velme_sema::hir::{Program, Type};
use velme_syntax::SourceFile;

fn analyze_str(text: &str) -> (Option<Program>, Vec<Diagnostic>) {
    analyze(&SourceFile::new("test.velme", text))
}

/// The codes `text` produces, in order.
fn codes(text: &str) -> Vec<Code> {
    analyze_str(text).1.iter().map(|d| d.code).collect()
}

/// The diagnostic `text` produces, which must be exactly one.
fn only(text: &str) -> Diagnostic {
    let (_, mut diags) = analyze_str(text);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    diags.remove(0)
}

fn program(text: &str) -> Program {
    let (program, diags) = analyze_str(text);
    assert!(diags.is_empty(), "{diags:#?}");
    program.expect("a program without errors")
}

#[test]
fn golden_accept() {
    insta::glob!("../../../tests/golden/sema/accept", "*.velme", |path| {
        let bytes = std::fs::read(path).expect("golden file is readable");
        let file = SourceFile::from_bytes(path.display().to_string(), bytes).expect("golden file is UTF-8");
        let (program, diags) = analyze(&file);
        assert!(diags.is_empty(), "{}: {diags:#?}", path.display());
        insta::assert_json_snapshot!(program.expect("a program without errors"));
    });
}

#[test]
fn records_resolve_to_nominal_types() {
    let p = program(
        "type Stats:\n    height: Number\n\ntype Player:\n    stats: List<Stats?>\n\n\
         goal Best(players: List<Player>) -> Player?:\n    plan: \"Pick one.\"\n",
    );
    let player = &p.types[1];
    assert_eq!(p.type_name(&player.fields[0].ty), "List<Stats?>");
    assert_eq!(
        p.goals[0].output,
        Type::Optional(Box::new(Type::Record(velme_sema::hir::TypeId(1))))
    );
}

#[test]
fn ac_typ_05_recursive_type_names_the_cycle() {
    let d = only("type Node:\n    value: Number\n    next: Node?\n");
    assert_eq!(d.code, Code::RecursiveType);
    assert_eq!(d.notes, ["Node → Node"]);
    let d = only("type A:\n    b: List<B>\n\ntype B:\n    a: A?\n");
    assert_eq!(d.notes, ["A → B → A"]);
}

#[test]
fn ac_typ_17_nothing_is_not_a_field_type() {
    assert_eq!(
        codes("type Bad:\n    value: Nothing\n    maybe: Nothing?\n"),
        [Code::TypeMismatch, Code::TypeMismatch]
    );
}

#[test]
fn ac_typ_18_duplicate_fields() {
    let d = only("type P:\n    x: Number\n    x: Number\n");
    assert_eq!(d.code, Code::DuplicateDeclaration);
    assert_eq!(d.message, "`x` is already defined on line 2.");
}

#[test]
fn ac_typ_19_builtin_type_name_and_unknown_type() {
    assert_eq!(codes("type Number:\n    x: Text\n"), [Code::DuplicateDeclaration]);
    let d = only("goal G(x: Unknown) -> Number:\n    plan: \"Use x.\"\n");
    assert_eq!(d.code, Code::UnknownType);
    assert_eq!(d.message, "I don't know a type called `Unknown`.");
}

#[test]
fn unknown_type_suggests_a_close_name() {
    let d = only("type Player:\n    name: Text\n\ngoal G(p: Plyer) -> Number:\n    plan: \"Use p.\"\n");
    assert_eq!(d.help.as_deref(), Some("did you mean `Player`?"));
}

#[test]
fn ac_goal_15_duplicate_goals_and_parameters() {
    let two_goals =
        "goal Score(x: Number) -> Number:\n    plan: \"x\"\n\ngoal Score(y: Number) -> Number:\n    plan: \"y\"\n";
    assert_eq!(codes(two_goals), [Code::DuplicateDeclaration]);
    assert_eq!(
        codes("goal G(x: Number, x: Text) -> Number:\n    plan: \"x\"\n"),
        [Code::DuplicateDeclaration]
    );
}

/// D-64: a declaration named like a built-in function gets one warning, the naming one, and the program is returned.
#[test]
fn builtin_function_name_is_one_warning() {
    let (program, diags) = analyze_str("goal sum(x: Number) -> Number:\n    plan: \"x\"\n");
    assert!(program.is_some());
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].message, "`sum` works, but Velme style writes it `Sum`.");
}

/// CC-ERR-04: a declaration whose name is taken is still checked.
#[test]
fn duplicate_declaration_body_is_still_checked() {
    assert_eq!(
        codes("type Score:\n    x: Number\n\ntype Score:\n    rank: Nubmer\n"),
        [Code::DuplicateDeclaration, Code::UnknownType]
    );
}

/// R-SYN-17: a type that failed to parse isn't reported again where it's used.
#[test]
fn failed_declaration_is_not_reported_again() {
    assert_eq!(
        codes("type Player:\n    name Text\n\ngoal Rank(p: Player) -> Number:\n    plan: \"Rank p.\"\n"),
        [Code::UnexpectedToken]
    );
}
