//! Semantic analysis: the golden HIR corpus (R-CMP-12) and the criteria of `language/11..13` it owns. Rejecting golden
//! files are snapshotted as `velme check --json` in the CLI tests (AC-CMP-04).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use velme_builtins::limits;
use velme_diagnostics::{Code, Diagnostic};
use velme_sema::analyze;
use velme_sema::hir::{Budget, GoalKind, Program, Type};
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

/// A misspelled goal name in an example gets a "did you mean" help, not advice about `call:`.
#[test]
fn example_with_a_misspelled_goal_name_suggests_the_goal() {
    let d = only("goal Score(x: Number) -> Number:\n    plan: \"x\"\n    examples:\n        - Scroe(2) == 1\n");
    assert_eq!(d.code, Code::InvalidCall);
    assert_eq!(d.help.as_deref(), Some("did you mean `Score`?"));
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

/// `language/12` §8.1 and §8.2, as written there.
const WORKED_PROGRAMS: &str = r#"type Player:
    name: Text
    jump_height: Number
    score: Number

type PlayerSummary:
    name: Text
    score: Number
    badge: Text

goal CalculateScore(player: Player) -> Number:
    plan: "Return the player's score."
    check:
        - result == player.score

goal FindBadge(player: Player) -> Text:
    plan: |
        Give the player Gold for a score of at least 1000,
        Silver for a score of at least 500, Bronze otherwise.
    check:
        - result == "Gold" or result == "Silver" or result == "Bronze"
    examples:
        - FindBadge(Player(name: "Lina", jump_height: 3, score: 820)) == "Silver"
        - FindBadge(Player(name: "Tom", jump_height: 5, score: 1000)) == "Gold"

goal BuildPlayerSummary(player: Player) -> PlayerSummary:
    call:
        score = CalculateScore(player)
        badge = FindBadge(player)
    plan: |
        Build a player summary using the player's name,
        calculated score, and badge.
    check:
        - result.name == player.name
        - result.score == score
        - result.badge == badge

goal FindHighestJumpingPlayer(players: List<Player>) -> Player?:
    plan: |
        Look at all the players.
        Find the one whose jump_height is the biggest.
        Return that player.
    check:
        - if players is empty then result is empty
        - if players is not empty then result is not empty
        - if result is not empty then every p in players has result.jump_height >= p.jump_height
"#;

/// `type Player` and a leaf `Score(player: Player) -> Number`, then `goal G(player: Player, x: Number) -> Number`
/// with `lines` as its `call:` block.
fn with_calls(lines: &str) -> String {
    format!(
        "type Stats:\n    height: Number\n\ntype Player:\n    name: Text\n    stats: Stats\n\n\
         goal Score(player: Player) -> Number:\n    plan: \"x\"\n\ngoal Height(stats: Stats) -> Number:\n    plan: \"x\"\n\n\
         goal G(player: Player, x: Number) -> Number:\n    call:\n{lines}    plan: \"x\"\n"
    )
}

fn repo_file(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/../../{rel}", env!("CARGO_MANIFEST_DIR"))).expect("repository file")
}

#[test]
fn ac_goal_01_goal_needs_a_plan_or_a_result_binding() {
    let d = only("goal G(x: Number) -> Number:\n    check:\n        - result > x\n");
    assert_eq!(d.code, Code::GoalHasNoBody);
    assert_eq!(d.message, "`G` needs a `plan:` that says what it should do.");
    assert_eq!(
        codes("goal G(x: Number) -> Number:\n    plan: \"\"\n"),
        [Code::GoalHasNoBody]
    );
}

#[test]
fn ac_goal_02_argument_type_is_checked_before_synthesis() {
    let d = only(&with_calls("        score = Score(\"hello\")\n"));
    assert_eq!(d.code, Code::TypeMismatch);
    assert_eq!(d.message, "Expected Player, but got Text.");
}

#[test]
fn ac_goal_03_self_call_and_cycle_path() {
    let d = only("goal A(x: Number) -> Number:\n    call:\n        value = A(x)\n    plan: \"x\"\n");
    assert_eq!(d.code, Code::CallCycle);
    assert_eq!(d.message, "These goals call each other in a circle: A → A.");
    let d = only(
        "goal A(x: Number) -> Number:\n    call:\n        b = B(x)\n    plan: \"x\"\n\n\
         goal B(x: Number) -> Number:\n    call:\n        a = A(x)\n    plan: \"x\"\n",
    );
    assert_eq!(d.code, Code::CallCycle);
    assert_eq!(d.message, "These goals call each other in a circle: A → B → A.");
}

#[test]
fn ac_goal_04_binding_used_before_its_line() {
    let d = only(&with_calls("        b = Score(a)\n        a = Score(player)\n"));
    assert_eq!(d.code, Code::BindingUsedBeforeDefinition);
    assert_eq!(d.message, "`a` is used before it's made — move its line up.");
}

#[test]
fn ac_goal_05_arguments_are_paths_or_literals() {
    let d = only(&with_calls("        s = Score(x + 1)\n"));
    assert_eq!(d.code, Code::InvalidCall);
    let p = program(&with_calls("        s = Height(player.stats)\n"));
    let s = &p.goals[2].bindings[0];
    assert_eq!((s.ty.clone(), s.wave), (Type::Number, 1));
    assert_eq!(
        codes(&with_calls("        s = Score(player.name.length)\n")),
        [Code::InvalidCall]
    );
}

/// A call block can't narrow, so a field of a value that might be empty gets the call-block advice (R-TYP-12).
#[test]
fn optional_field_path_in_a_call() {
    let text = "type R:\n    name: Text\n\ntype P:\n    rival: R?\n\ngoal N(name: Text) -> Number:\n    plan: \"x\"\n\n\
                goal G(p: P) -> Number:\n    call:\n        n = N(p.rival.name)\n    plan: \"x\"\n";
    let d = only(text);
    assert_eq!(d.code, Code::NullableAccess);
    assert_eq!(d.message, "`p.rival` might be empty, so a call can't use its `name`.");
    assert_eq!(
        d.help.as_deref(),
        Some("a call can't check that first — pass `p.rival` itself to an input of type `R?`")
    );
}

#[test]
fn ac_goal_06_unknown_goal_and_arity() {
    let d = only(&with_calls("        s = Scor(player)\n"));
    assert_eq!(d.code, Code::UnknownGoal);
    assert_eq!(d.message, "I don't know a goal called `Scor`.");
    assert_eq!(d.help.as_deref(), Some("did you mean `Score`?"));
    let d = only(&with_calls("        s = Score(player, x)\n"));
    assert_eq!(d.code, Code::CallArityMismatch);
    assert_eq!(d.message, "`Score` needs 1 input, but got 2.");
}

#[test]
fn ac_goal_07_mixed_waves() {
    let p = program(&repo_file("examples/games/level_summary.velme"));
    let goal = p
        .goals
        .iter()
        .find(|g| g.name == "CreateLevelSummary")
        .expect("the §8.4 goal");
    let waves: Vec<(&str, usize)> = goal.bindings.iter().map(|b| (b.name.as_str(), b.wave)).collect();
    assert_eq!(
        waves,
        [
            ("enemies", 1),
            ("treasures", 1),
            ("score", 1),
            ("difficulty", 2),
            ("reward", 2)
        ]
    );
    assert_eq!(goal.kind, GoalKind::Composite);
}

#[test]
fn ac_goal_10_budget_only_lowers_the_caps() {
    let d = only("goal G(x: Number) -> Number:\n    budget cpu=10ms calls=999\n    plan: \"x\"\n");
    assert_eq!(d.code, Code::InvalidBudget);
    assert_eq!(
        d.message,
        format!(
            "`budget` can only make limits smaller — `calls` can be at most `{}`.",
            limits::MAX_GOAL_CALLS
        )
    );
    let p = program("goal G(x: Number) -> Number:\n    budget cpu=10ms depth=2\n    plan: \"x\"\n");
    let budget = p.goals[0].budget;
    assert_eq!(budget.max_call_depth, 2);
    assert_eq!(budget.max_fuel, 10 * limits::FUEL_PER_MS);
    assert_eq!(
        (budget.max_goal_calls, budget.max_memory),
        (Budget::SYSTEM.max_goal_calls, Budget::SYSTEM.max_memory)
    );
}

#[test]
fn ac_goal_12_result_binding_must_fit_the_output() {
    let text = "goal Name(x: Number) -> Text:\n    plan: \"x\"\n\n\
                goal Main(x: Number) -> Number:\n    call:\n        result = Name(x)\n";
    let d = only(text);
    assert_eq!(d.code, Code::TypeMismatch);
    assert_eq!(d.message, "Expected Number, but got Text.");
    let optional = "goal Maybe(x: Number) -> Number:\n    plan: \"x\"\n\n\
                    goal Main(x: Number) -> Number?:\n    call:\n        result = Maybe(x)\n";
    assert_eq!(program(optional).goals[1].kind, GoalKind::Wired);
}

#[test]
fn ac_goal_13_worked_programs_check_clean() {
    let p = program(WORKED_PROGRAMS);
    let kinds: Vec<GoalKind> = p.goals.iter().map(|g| g.kind).collect();
    assert_eq!(
        kinds,
        [GoalKind::Leaf, GoalKind::Leaf, GoalKind::Composite, GoalKind::Leaf]
    );
}

#[test]
fn ac_goal_14_fractional_and_wrong_unit_cpu() {
    let budget = |line: &str| {
        only(&format!(
            "goal G(x: Number) -> Number:\n    budget {line}\n    plan: \"x\"\n"
        ))
    };
    let d = budget("cpu=1.5ms");
    assert_eq!(
        (d.code, d.message.as_str()),
        (Code::InvalidBudget, "`cpu` is written as a whole number, without a `.`")
    );
    let d = budget("cpu=10.0ms");
    assert_eq!(d.message, "`cpu` is written as a whole number, without a `.`");
    let d = budget("depth=1.0");
    assert_eq!(d.message, "`depth` is written as a whole number, without a `.`");
    assert!(d.help.is_none());
    let d = budget("memory=1.5mb");
    assert_eq!(d.message, "`memory` is written as a whole number, without a `.`");
    assert_eq!(d.help.as_deref(), Some("write `memory=1536kb`"));
    assert_eq!(budget("memory=1.5_0mb").help.as_deref(), Some("write `memory=1536kb`"));
    assert!(budget("memory=128.5mb").help.is_none());
    let d = budget("memory=1.0mb");
    assert_eq!(d.help.as_deref(), Some("write `memory=1024kb`"));
    program("goal G(x: Number) -> Number:\n    budget calls=1_0\n    plan: \"x\"\n");
    let d = budget("cpu=10s");
    assert_eq!(
        (d.code, d.message.as_str()),
        (
            Code::InvalidBudget,
            "`cpu` is a time in whole milliseconds, like `cpu=10ms`."
        )
    );
}

#[test]
fn ac_goal_16_duplicate_binding_and_parameter_name() {
    let d = only(&with_calls(
        "        total = Score(player)\n        total = Score(player)\n",
    ));
    assert_eq!(d.code, Code::DuplicateBinding);
    assert_eq!(d.message, "`total` is already used in this goal.");
    assert_eq!(
        codes(&with_calls("        x = Score(player)\n")),
        [Code::DuplicateBinding]
    );
}

#[test]
fn ac_goal_18_result_names_only_the_last_binding() {
    let early = with_calls("        result = Score(player)\n        s = Score(player)\n");
    assert_eq!(codes(&early), [Code::UnexpectedToken]);
    assert_eq!(
        codes(&with_calls("        s = Score(result)\n")),
        [Code::UnexpectedToken]
    );
    let d = only(&with_calls("        s = Height(result.stats + 1)\n"));
    assert_eq!(
        (d.code, d.message.as_str()),
        (Code::UnexpectedToken, "`result` can only name the last binding.")
    );
    let wired = "goal Score(x: Number) -> Number:\n    plan: \"x\"\n\n\
                 goal Main(x: Number) -> Number:\n    call:\n        s = Score(x)\n        result = Score(s)\n";
    let p = program(wired);
    assert_eq!(p.goals[1].kind, GoalKind::Wired);
    assert_eq!(p.goals[1].bindings[1].wave, 2);
}

/// Every goal can be the top of a run, so each run tree is held to the system caps; a goal above one that is already
/// over isn't reported again.
#[test]
fn ac_run_06_call_tree_over_the_cap_is_rejected_statically() {
    let fan = |name: &str, callee: &str, n: u64| {
        let lines: String = (0..n).map(|i| format!("        c{i} = {callee}(x)\n")).collect();
        format!("goal {name}(x: Number) -> Number:\n    call:\n{lines}    plan: \"x\"\n\n")
    };
    let leaf = "goal Leaf(x: Number) -> Number:\n    plan: \"x\"\n\n";
    program(&format!("{leaf}{}", fan("Wide", "Leaf", limits::MAX_GOAL_CALLS - 1)));
    let text = format!(
        "{leaf}{}{}",
        fan("Wide", "Leaf", limits::MAX_GOAL_CALLS),
        fan("Top", "Wide", 1)
    );
    let d = only(&text);
    assert_eq!(d.code, Code::CallLimitExceeded);
    assert_eq!(d.message, "Too many goals were called while running `Wide`.");
    assert_eq!(
        d.notes,
        [format!(
            "it would run {} goals, counting itself, but its limit is {}",
            limits::MAX_GOAL_CALLS + 1,
            limits::MAX_GOAL_CALLS
        )]
    );
}

/// `max_call_depth` counts the calls below the top of the run (`runtime/30` §7).
#[test]
fn call_depth_over_the_cap_is_rejected_statically() {
    let chain = |n: u64| {
        let mut text = "goal G0(x: Number) -> Number:\n    plan: \"x\"\n".to_owned();
        for i in 1..=n {
            text.push_str(&format!(
                "\ngoal G{i}(x: Number) -> Number:\n    call:\n        y = G{}(x)\n    plan: \"x\"\n",
                i - 1
            ));
        }
        text
    };
    program(&chain(limits::MAX_CALL_DEPTH));
    let d = only(&chain(limits::MAX_CALL_DEPTH + 2));
    assert_eq!(d.code, Code::CallLimitExceeded);
    assert_eq!(
        d.message,
        format!(
            "Goals call each other too deeply while running `G{}`.",
            limits::MAX_CALL_DEPTH + 1
        )
    );
    assert_eq!(
        d.notes,
        [format!(
            "its calls would nest {} deep, but its limit is {}",
            limits::MAX_CALL_DEPTH + 1,
            limits::MAX_CALL_DEPTH
        )]
    );
}

/// The recursive-type label points at the field that leads back, even when a duplicate field comes first.
#[test]
fn recursive_type_label_survives_a_duplicate_field() {
    let text = "type Node:\n    a: Number\n    a: Number\n    next: Node?\n";
    let d = analyze_str(text).1;
    let d = d.iter().find(|d| d.code == Code::RecursiveType).expect("recursive");
    assert_eq!(d.labels.len(), 1);
    let start = d.labels[0].span.start;
    assert_eq!(&text[start..start + 4], "next");
}

/// An example that calls another goal gets advice fit for an example, not for a `call:` block.
#[test]
fn example_calling_another_goal_says_what_examples_may_call() {
    let text = "goal Half(x: Number) -> Number:\n    plan: \"x\"\n\ngoal Double(x: Number) -> Number:\n    plan: \"x\"\n    examples:\n        - Half(2) == 1\n";
    let d = only(text);
    assert_eq!(d.code, Code::InvalidCall);
    assert_eq!(d.message, "An example can only call `Double`, the goal it belongs to.");
}

/// A goal with a repeated parameter still takes as many inputs as it lists, so calls to it aren't reported again.
#[test]
fn duplicate_parameter_keeps_the_goal_arity() {
    let text = "goal Pair(a: Number, a: Number) -> Number:\n    plan: \"x\"\n\n\
                goal G(x: Number) -> Number:\n    call:\n        p = Pair(x, x)\n        q = Pair(x)\n    plan: \"x\"\n";
    assert_eq!(codes(text), [Code::DuplicateDeclaration, Code::CallArityMismatch]);
    let d = &analyze_str(text).1[1];
    assert_eq!(d.message, "`Pair` needs 2 inputs, but got 1.");
}

#[test]
fn ac_cmp_03_independent_errors_from_three_phases() {
    let text = repo_file("tests/golden/sema/reject/independent_errors.velme");
    assert_eq!(
        codes(&text),
        [Code::UnexpectedToken, Code::UnknownType, Code::CallCycle]
    );
}
