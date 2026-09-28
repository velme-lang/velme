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

/// A goal `G(inputs) -> output` with one check item.
fn check_item(inputs: &str, output: &str, item: &str) -> String {
    format!(
        "type Player:\n    name: Text\n    score: Number\n\ntype Summary:\n    name: Text\n    score: Number\n\n\
         goal G({inputs}) -> {output}:\n    plan: \"Do it.\"\n    check:\n        - {item}\n"
    )
}

#[test]
fn ac_typ_04_field_of_optional_needs_narrowing() {
    let d = only(&check_item("p: Player", "Player?", "result.score > 0"));
    assert_eq!(d.code, Code::NullableAccess);
    assert_eq!(d.message, "`result` might be empty — check `is not empty` first.");
    program(&check_item(
        "p: Player",
        "Player?",
        "result is not empty and result.score > 0",
    ));
}

#[test]
fn ac_typ_09_text_ordering_and_plus() {
    assert_eq!(
        only(&check_item("t: Text", "Number", "\"b\" < \"a\"")).code,
        Code::InvalidOperandType
    );
    let d = only(&check_item("t: Text", "Number", "\"a\" + \"b\" == t"));
    assert_eq!(d.code, Code::InvalidOperandType);
    assert_eq!(d.help.as_deref(), Some("use `concat(a, b)` to join text"));
}

#[test]
fn ac_typ_10_records_of_different_types_are_not_comparable() {
    let d = only(&check_item("a: Player, b: Summary", "Number", "a == b"));
    assert_eq!(d.message, "`==` can't be used with Player and Summary.");
    program(&check_item("a: Player, b: Player", "Number", "a == b"));
}

#[test]
fn ac_typ_12_record_literal_missing_a_nullable_field() {
    let text = "type P:\n    name: Text\n    nick: Text?\n\ngoal G(p: P) -> Text:\n    plan: \"Name.\"\n    \
                examples:\n        - G(P(name: \"Ada\")) == \"Ada\"\n";
    let d = only(text);
    assert_eq!(d.code, Code::TypeMismatch);
    assert_eq!(d.message, "This `P` is missing `nick`.");
}

#[test]
fn ac_typ_13_projection_and_length() {
    let p = program(&check_item(
        "players: List<Player>",
        "Number",
        "players.score == [1] and players.length == 1",
    ));
    let velme_sema::hir::ExprKind::Binary { lhs, rhs, .. } = &p.goals[0].checks[0].kind else {
        panic!("an `and`");
    };
    let velme_sema::hir::ExprKind::Binary { lhs: projection, .. } = &lhs.kind else {
        panic!("an `==`");
    };
    assert_eq!(p.type_name(&projection.ty), "List<Number>");
    let velme_sema::hir::ExprKind::Binary { lhs: length, .. } = &rhs.kind else {
        panic!("an `==`");
    };
    assert!(matches!(
        length.kind,
        velme_sema::hir::ExprKind::Builtin { name: "length", .. }
    ));
}

#[test]
fn ac_typ_18_unknown_field() {
    let d = only(&check_item("p: Player", "Number", "p.scor > 0"));
    assert_eq!(d.code, Code::UnknownField);
    assert_eq!(d.message, "A `Player` doesn't have a field called `scor`.");
    assert_eq!(d.help.as_deref(), Some("did you mean `score`?"));
}

/// The static half of AC-TYP-20: both narrowings of a compound condition apply (R-TYP-27).
#[test]
fn ac_typ_20_compound_narrowing() {
    for item in [
        "if a is not empty and b is not empty then result == a.score + b.score",
        "a is empty or b is empty or result == a.score + b.score",
        "if not (a is empty or b is empty) then result == a.score + b.score",
    ] {
        program(&check_item("a: Player?, b: Player?", "Number", item));
    }
    assert_eq!(
        codes(&check_item(
            "a: Player?, b: Player?",
            "Number",
            "a is empty and b.score > 0"
        )),
        [Code::NullableAccess]
    );
}

/// Narrowing doesn't cross check items or flow out of built-in calls (R-TYP-22).
#[test]
fn narrowing_stays_in_its_item_and_skips_builtins() {
    let text = check_item("p: Player?", "Number", "p is not empty\n        - p.score > 0");
    assert_eq!(codes(&text), [Code::NullableAccess]);
    let d = only(&check_item("xs: List<Number>", "Number", "maximum(xs) > 0"));
    assert_eq!(d.code, Code::NullableAccess);
}

#[test]
fn ac_typ_21_optional_list_narrows_to_list() {
    program(&check_item(
        "xs: List<Number>?",
        "Number",
        "xs is not empty and sum(xs) > 0",
    ));
    program(&check_item("t: Text?", "Number", "t is empty or t.length > 0"));
}

#[test]
fn list_literals_take_the_common_type() {
    program(&check_item(
        "xs: List<Number?>",
        "Number",
        "xs == [1, nothing] and [nothing, 2] == xs",
    ));
    assert_eq!(
        codes(&check_item("p: Player", "Number", "[1, \"a\"] == []")),
        [Code::TypeMismatch]
    );
    let d = only(&check_item("p: Player", "Number", "[] == []"));
    assert_eq!(d.message, "I can't tell what kind of list this is.");
}

#[test]
fn ac_chk_01_check_item_is_boolean() {
    let d = only(&check_item("x: Number", "Number", "result + 1"));
    assert_eq!(d.code, Code::TypeMismatch);
    assert_eq!(d.message, "Expected Boolean, but got Number.");
}

#[test]
fn ac_chk_02_bindings_are_visible_and_unknown_names_are_not() {
    let text = "goal Score(x: Number) -> Number:\n    plan: \"x\"\n\n\
                goal Main(x: Number) -> Number:\n    call:\n        score = Score(x)\n    plan: \"Use score.\"\n    \
                check:\n        - result == score\n        - result == scor\n";
    let d = only(text);
    assert_eq!(d.code, Code::UnknownName);
    assert_eq!(d.message, "I don't know what `scor` is here.");
    assert_eq!(d.help.as_deref(), Some("did you mean `score`?"));
}

#[test]
fn ac_chk_03_goal_called_in_a_check() {
    let d = only(&check_item("p: Player", "Number", "G(p) > 0"));
    assert_eq!(d.code, Code::InvalidCall);
    assert_eq!(d.message, "`G` can only be used after it's listed in `call:`.");
}

#[test]
fn ac_chk_12_quantifier_variable_repeats_an_input() {
    let d = only(&check_item(
        "player: Player",
        "Number",
        "every player in [player] has player.score > 0",
    ));
    assert_eq!(d.code, Code::DuplicateBinding);
    assert_eq!(d.message, "`player` is already used in this goal.");
}

#[test]
fn ac_goal_11_example_calls_its_own_goal() {
    let text = "goal Double(x: Number) -> Number:\n    plan: \"Twice x.\"\n    examples:\n        - Half(2) == 1\n        \
                - Double(2, 3) == 4\n        - Double(\"2\") == 4\n";
    assert_eq!(
        codes(text),
        [Code::InvalidCall, Code::CallArityMismatch, Code::TypeMismatch]
    );
}

/// Examples follow R-TYP-20: a `Number` or `nothing` for a `T?` input (AC-TYP-02, static half).
#[test]
fn examples_assign_to_optional_inputs() {
    let p = program(
        "type P:\n    x: Number\n\ngoal G(n: Number?, p: P?) -> Number:\n    plan: \"x\"\n    examples:\n        \
         - G(3, nothing) == 3\n        - G(nothing, P(x: 1)) == 1\n",
    );
    assert_eq!(p.goals[0].examples.len(), 2);
}

#[test]
fn ac_blt_08_ir_primitive_in_a_check() {
    let d = only(&check_item("xs: List<Number>", "Number", "map(xs) == xs"));
    assert_eq!(d.code, Code::UnknownName);
}

#[test]
fn ac_blt_14_builtin_call_ignores_an_input_of_the_same_name() {
    program(&check_item("sum: Number, xs: List<Number>", "Number", "sum(xs) > sum"));
}

#[test]
fn builtin_arguments_are_checked_against_the_signature() {
    let d = only(&check_item("t: Text", "Number", "sum(t) > 0"));
    assert_eq!(d.message, "Expected List<Number>, but got Text.");
    let d = only(&check_item("t: Number", "Number", "t.length > 0"));
    assert_eq!(d.code, Code::UnknownField);
    assert_eq!(
        codes(&check_item(
            "xs: List<Number>",
            "Number",
            "contains(xs, \"a\") and clamp(1, 2) > 0"
        )),
        [Code::TypeMismatch, Code::CallArityMismatch]
    );
    program(&check_item(
        "xs: List<Number?>",
        "Number",
        "contains(xs, nothing) and contains(xs, 3)",
    ));
}

/// R-CMP-10: an error inside an expression isn't reported again by what contains it.
#[test]
fn errors_do_not_cascade() {
    assert_eq!(
        codes(&check_item(
            "t: Text",
            "Number",
            "t + \"!\" == \"a\" and missing.score > 0"
        )),
        [Code::InvalidOperandType, Code::UnknownName]
    );
}

/// AC-BLT-14, second half: a goal named like a built-in doesn't hide the built-in from checks (D-64).
#[test]
fn ac_blt_14_goal_named_like_a_builtin_keeps_the_builtin_in_checks() {
    let text = "goal sum(x: Number) -> Number:\n    plan: \"x\"\n\ngoal G(xs: List<Number>) -> Number:\n    \
                plan: \"x\"\n    check:\n        - sum(xs) >= 0\n";
    let (program, diags) = analyze_str(text);
    assert!(program.is_some(), "{diags:#?}");
    assert_eq!(codes(text), [Code::LintWarning]);
}

/// `[]` takes its element type from an expected optional list too (R-TYP-13, R-TYP-20).
#[test]
fn empty_list_fits_an_optional_list() {
    let p = program(
        "goal G(tags: List<Text>?) -> Number:\n    plan: \"x\"\n    check:\n        - tags == [] or tags is empty\n    \
         examples:\n        - G([]) == 0\n",
    );
    assert_eq!(p.type_name(&p.goals[0].examples[0].args[0].ty), "List<Text>");
}
