//! Domain declarations, typing and source literals (`language/11` §7.1, `language/13` §6.1, D2).
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use velme_diagnostics::{Code, Diagnostic};
use velme_sema::analyze;
use velme_sema::hir::{DomainId, DomainRule, Program, Type, signature_output};
use velme_syntax::SourceFile;

const DOMAINS: &str = "type Risk = Text one of \"DECLINE\", \"HIGH\", \"VERY LOW\"
type Age = Number where value >= 18 and value <= 120 and value is whole
type Score = Number where 0 <= value
type Label = Text where value.length >= 1 and value.length <= 40
type Offer where value.discount <= value.price:
    price: Number
    discount: Number
type Person:
    age: Age
    risk: Risk?
";

fn analyze_str(text: &str) -> (Option<Program>, Vec<Diagnostic>) {
    analyze(&SourceFile::new("test.velme", text))
}

fn with(rest: &str) -> String {
    format!("{DOMAINS}\n{rest}")
}

fn diags(rest: &str) -> Vec<Diagnostic> {
    analyze_str(&with(rest))
        .1
        .into_iter()
        .filter(Diagnostic::is_error)
        .collect()
}

fn codes(rest: &str) -> Vec<Code> {
    diags(rest).iter().map(|d| d.code).collect()
}

fn check(inputs: &str, item: &str) -> Vec<Code> {
    codes(&format!(
        "goal G({inputs}) -> Boolean:\n    plan: \"p\"\n    check:\n        - {item}\n"
    ))
}

#[test]
fn ac_typ_22_domains_type_check_and_arithmetic_is_base() {
    let (program, d) = analyze_str(&with(
        "goal G(age: Age, risk: Risk, label: Label, offer: Offer) -> Number:\n    \
                                         plan: \"p\"\n    check:\n        - result == age + 1\n        - risk == \"HIGH\"\n        \
                                         - label.length > 0 and offer.price >= offer.discount\n",
    ));
    assert!(d.is_empty(), "{d:#?}");
    let program = program.expect("a program");
    assert_eq!(program.domains.len(), 5);
    let age = &program.domains[1];
    assert_eq!(age.constraints.lower.as_ref().map(|b| b.value.as_str()), Some("18"));
    assert!(age.constraints.whole);
    let check = &program.goals[0].checks[0];
    let velme_sema::hir::ExprKind::Binary { rhs, .. } = &check.kind else {
        panic!("a comparison")
    };
    assert_eq!(rhs.ty, Type::Number, "Number-domain arithmetic is Number");
    assert!(matches!(&program.domains[0].rule, DomainRule::OneOf { members } if members.len() == 3));
}

#[test]
fn ac_typ_23_invalid_declarations_are_vl0209() {
    for decl in [
        "type E = Text one of",
        "type D = Text one of \"A\", \"A\"",
        "type N = Text one of \"A\", 1",
        "type W = Number one of \"A\"",
        "type B = Boolean where value",
        "type C = Number where value + 1",
        "type L = Number where value > 1 and value >= 2",
        "type I = Number where value > 1 and value < 2 and value is whole",
        "type T = Text where value.length > 3 and value.length < 4",
        "type X = Text where value.length >= 1.5",
        "type Q = Number where value > 5 and value < 5",
        "type S = Age where value > 20",
    ] {
        let (_, d) = analyze_str(&format!("type Age = Number where value > 0\n{decl}\n"));
        let codes: Vec<Code> = d.iter().filter(|d| d.is_error()).map(|d| d.code).collect();
        assert_eq!(codes, [Code::InvalidDomain], "{decl}: {d:#?}");
        assert!(d[0].message.starts_with("Domain `"), "{}", d[0].message);
    }
}

#[test]
fn ac_typ_24_domains_widen_and_lists_stay_invariant() {
    let call = |param: &str, arg: &str| {
        codes(&format!(
            "goal F(x: {param}) -> Number:\n    plan: \"p\"\n\ngoal G(a: {arg}) -> Number:\n    call:\n        \
             r = F(a)\n    plan: \"p\"\n"
        ))
    };
    assert_eq!(call("Number", "Age"), []);
    assert_eq!(call("List<Number?>", "List<Age?>"), []);
    assert_eq!(call("Number?", "Age"), []);
    assert_eq!(call("List<Number?>", "List<Number>"), [Code::TypeMismatch]);
    assert_eq!(check("o: Offer, p: Offer", "o == p and o.price > 0"), []);
}

#[test]
fn ac_typ_25_distinct_domains_and_base_arguments_are_rejected() {
    let src = "goal F(x: Age) -> Number:\n    plan: \"p\"\n\ngoal Make() -> Number:\n    plan: \"p\"\n\n\
               goal G(s: Score) -> Number:\n    call:\n        m = Make()\n        a = F(s)\n        b = F(m)\n    plan: \"p\"\n";
    let d = diags(src);
    assert_eq!(
        d.iter().map(|d| d.code).collect::<Vec<_>>(),
        [Code::TypeMismatch, Code::TypeMismatch]
    );
    assert_eq!(d[1].help.as_deref(), Some("declare `Make`'s output as `Age`"));
}

#[test]
fn ac_typ_26_source_literals_meet_simple_constraints() {
    let goal = |args: &str| {
        diags(&format!(
            "goal G(age: Age, label: Label, people: List<Person>) -> Number:\n    plan: \"p\"\n    examples:\n        \
             - G({args}) == 1\n"
        ))
    };
    assert_eq!(goal("18, \"a\", []").len(), 0);
    assert_eq!(goal("120, \"a\", [Person(age: 30, risk: nothing)]").len(), 0);
    for bad in [
        "17, \"a\", []",
        "-1, \"a\", []",
        "18.5, \"a\", []",
        "18, \"\", []",
        "18, \"a\", [Person(age: 200, risk: nothing)]",
        "18, \"a\", [Person(age: 20, risk: \"LOW\")]",
    ] {
        let d = goal(bad);
        assert_eq!(
            d.iter().map(|d| d.code).collect::<Vec<_>>(),
            [Code::TypeMismatch],
            "{bad}: {d:#?}"
        );
        assert!(d[0].message.contains("isn't a valid `"), "{}", d[0].message);
    }
    let d = goal("17, \"a\", []");
    assert_eq!(d[0].message, "`17` isn't a valid `Age`: it must be at least 18.");
}

#[test]
fn ac_typ_27_closed_set_comparisons_name_members() {
    for item in ["r == \"LOW\"", "\"LOW\" != r", "q == \"LOW\""] {
        assert_eq!(check("r: Risk, q: Risk?", item), [Code::TypeMismatch], "{item}");
    }
    assert_eq!(check("r: Risk, t: Text", "r == \"HIGH\" and r != t"), []);
}

#[test]
fn ac_typ_29_closed_set_limits() {
    let set = |members: Vec<String>| {
        let (_, d) = analyze_str(&format!("type Big = Text one of {}\n", members.join(", ")));
        d.iter().filter(|d| d.is_error()).map(|d| d.code).collect::<Vec<_>>()
    };
    let numbered = |n: usize| (0..n).map(|i| format!("\"m{i}\"")).collect::<Vec<_>>();
    assert_eq!(set(numbered(1000)), []);
    assert_eq!(set(numbered(1001)), [Code::InvalidDomain]);
    assert_eq!(set(vec![format!("\"{}\"", "a".repeat(65_536))]), []);
    let half = |c: &str, n: usize| format!("\"{}\"", c.repeat(n));
    assert_eq!(set(vec![half("a", 32_768), half("b", 32_768)]), []);
    assert_eq!(set(vec![half("a", 32_768), half("b", 32_769)]), [Code::InvalidDomain]);
}

#[test]
fn ac_typ_31_record_domain_constructor_is_the_underlying_record() {
    assert_eq!(check("o: Offer", "o == Offer(price: 1, discount: 5)"), []);
}

#[test]
fn ac_typ_32_record_base_is_invalid() {
    let (_, d) = analyze_str("type Person:\n    age: Number\ntype Adult = Person where value.age >= 18\n");
    assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), [Code::InvalidDomain]);
    assert!(
        d[0].help.as_deref().is_some_and(|h| h.contains("type Adult where")),
        "{d:#?}"
    );
}

#[test]
fn ac_chk_13_is_whole_needs_a_number() {
    assert_eq!(check("x: Number", "x is whole or x is not whole"), []);
    assert_eq!(check("x: Number?", "x is whole"), [Code::NullableAccess]);
    assert_eq!(check("x: Number?", "x is not empty and x is whole"), []);
    assert_eq!(check("x: Text", "x is whole"), [Code::InvalidOperandType]);
}

#[test]
fn ac_chk_14_conditions_see_only_value() {
    let (_, d) =
        analyze_str("type Offer where value.discount <= value.price:\n    price: Number\n    discount: Number\n");
    assert!(d.is_empty(), "{d:#?}");
    for cond in ["value > limit", "result > 0", "value > x"] {
        let (_, d) = analyze_str(&format!(
            "type A = Number where {cond}\ngoal G(limit: Number, x: Number) -> Number:\n    call:\n        r = H()\n    \
             plan: \"p\"\n\ngoal H() -> Number:\n    plan: \"p\"\n"
        ));
        let codes: Vec<Code> = d.iter().map(|d| d.code).collect();
        assert_eq!(codes, [Code::InvalidDomain], "{cond}: {d:#?}");
        assert!(d[0].notes.iter().any(|n| n.contains("VL0202")), "{d:#?}");
    }
}

#[test]
fn ac_chk_15_split_extracts_only_simple_conjuncts() {
    let program = |decl: &str| {
        let (p, d) = analyze_str(&format!("{decl}\n"));
        assert!(d.is_empty(), "{decl}: {d:#?}");
        p.expect("a program").domains.remove(0)
    };
    let d = program("type A = Number where (0 <= value and value < 10) and value is whole");
    assert_eq!(
        d.constraints.lower.as_ref().map(|b| (b.value.as_str(), b.inclusive)),
        Some(("0", true))
    );
    assert_eq!(
        d.constraints.upper.as_ref().map(|b| (b.value.as_str(), b.inclusive)),
        Some(("10", false))
    );
    let d = program("type B = Number where value > -5 or value == 3");
    assert_eq!(d.constraints, Default::default());
    let d = program("type C = Text where value.length <= 3 + 1 and 2 <= value.length");
    assert!(d.constraints.max_length.is_none());
    assert_eq!(d.constraints.min_length.as_ref().map(|b| b.value.as_str()), Some("2"));
    let (_, d) = analyze_str("type D = Number where value >= 1 and 0 < value\n");
    assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), [Code::InvalidDomain]);
    let (_, d) = analyze_str("type E = Text where value.length >= -1\n");
    assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), [Code::InvalidDomain]);
}

#[test]
fn ac_err_07_non_member_and_producer_help() {
    let d = diags("goal G(r: Risk) -> Number:\n    plan: \"p\"\n    examples:\n        - G(\"MAYBE\") == 1\n");
    assert_eq!(
        d[0].message,
        "`\"MAYBE\"` isn't a valid `Risk`: it isn't one of its members."
    );
}

// ---- D2 review findings ----

/// D2 review #1 (AC-TYP-25): a different domain never fits `D?`, at the top or in a list; the same domain does.
#[test]
fn ac_typ_25_a_different_domain_never_fits_an_optional_domain() {
    let field = |ty: &str, value: &str, param: &str| {
        codes(&format!(
            "type Grade = Number where value >= 0\ntype R:\n    g: {ty}\n\ngoal G(a: {param}) -> R:\n    plan: \"p\"\n    \
             check:\n        - result == R(g: {value})\n"
        ))
    };
    assert_eq!(field("Grade?", "a", "Age"), [Code::TypeMismatch]);
    assert_eq!(field("Grade", "a", "Age"), [Code::TypeMismatch]);
    assert_eq!(field("List<Grade?>", "a", "List<Age>"), [Code::TypeMismatch]);
    assert_eq!(field("Age?", "a", "Age"), []);
    assert_eq!(field("Number?", "a", "Age"), []);
}

/// The output of an `unwrap_or`-shaped signature, `(T?, T) -> T`, on arguments of types `args`: `unwrap_or` is an IR
/// node, not a catalog built-in, but unifies `T` through the same [`signature_output`] (R-TYP-30).
fn unwrap_or(args: &[Type]) -> Option<Type> {
    use velme_builtins::{Shape, Signature};
    const SIGNATURE: Signature = Signature {
        params: &[Shape::Optional(&Shape::T), Shape::T],
        output: Shape::T,
    };
    signature_output(&SIGNATURE, args)
}

fn age() -> Type {
    Type::Domain(DomainId(0), Box::new(Type::Number))
}

/// D2 review #2 (AC-TYP-24, R-TYP-30): a type variable bound to a domain takes its base later, and the reverse, and
/// unifies to the base.
#[test]
fn ac_typ_24_type_variables_unify_a_domain_with_its_base() {
    assert_eq!(check("risks: List<Risk>", "contains(risks, \"HIGH\")"), []);
    assert_eq!(check("ages: List<Age>, n: Number", "contains(ages, n)"), []);
    assert_eq!(check("ns: List<Number>, a: Age", "contains(ns, a)"), []);
    let optional = |t: Type| Type::Optional(Box::new(t));
    assert_eq!(unwrap_or(&[optional(age()), Type::Number]), Some(Type::Number));
    assert_eq!(unwrap_or(&[optional(Type::Number), age()]), Some(Type::Number));
    assert_eq!(unwrap_or(&[optional(age()), age()]), Some(age()));
}

/// D2 review #2, accepted behaviour: `unwrap_or` of two distinct domains is their base, `Number`; a list literal
/// against a domain-bound `T` is still checked for membership.
#[test]
fn ac_typ_24_distinct_domains_unify_to_the_base() {
    let score = Type::Domain(DomainId(1), Box::new(Type::Number));
    assert_eq!(unwrap_or(&[Type::Optional(Box::new(age())), score]), Some(Type::Number));
    assert_eq!(check("ages: List<Age>, s: Score", "contains(ages, s)"), []);
    assert_eq!(
        check("xs: List<List<Age>>", "contains(xs, [200])"),
        [Code::TypeMismatch]
    );
}

/// D2 review #4 (AC-TYP-23): a strict lower length bound at `i64::MAX` is an empty interval, `VL0209`, not an
/// overflow. (A negative upper bound is already its own `VL0209`.)
#[test]
fn ac_typ_23_length_bounds_past_i64_are_empty() {
    let (_, d) = analyze_str("type T = Text where value.length > 9223372036854775807\n");
    assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), [Code::InvalidDomain]);
    assert!(d[0].message.contains("no text length fits"), "{}", d[0].message);
}

/// D2 review #5 (AC-TYP-31): a list of optional record-domain values has no field to list, `VL0207`, in a check and in
/// a call block.
#[test]
fn ac_typ_31_optional_record_domains_in_a_list_are_nullable() {
    assert_eq!(check("xs: List<Offer?>", "xs.price == []"), [Code::NullableAccess]);
    let call = codes(
        "goal F(p: List<Number>) -> Number:\n    plan: \"p\"\n\ngoal G(xs: List<Offer?>) -> Number:\n    call:\n        \
         r = F(xs.price)\n    plan: \"p\"\n",
    );
    assert_eq!(call, [Code::NullableAccess]);
}

/// D2 review #6 (AC-ERR-04): a re-issued condition error keeps a standalone "I", and a non-Boolean condition's help
/// names `value`, never `result`.
#[test]
fn ac_err_04_condition_errors_read_well() {
    let (_, d) = analyze_str("type Age = Number where x > 0\n");
    assert!(d[0].message.contains("problem: I "), "{}", d[0].message);
    let (_, d) = analyze_str("type C = Number where value + 1\n");
    let help = d[0].help.as_deref().unwrap_or_default();
    assert!(help.contains("`value > 0`") && !help.contains("result"), "{help}");
}

/// D2 review #7 (AC-TYP-23, CC-ERR-04): a domain whose name is taken still has its condition and closed set checked.
#[test]
fn ac_typ_23_taken_domain_names_are_still_checked() {
    let codes = |text: &str| {
        let (_, d) = analyze_str(text);
        d.iter().filter(|d| d.is_error()).map(|d| d.code).collect::<Vec<_>>()
    };
    assert_eq!(
        codes("type Age = Number where value > 0\ntype Age = Number where nope > 0\n"),
        [Code::DuplicateDeclaration, Code::InvalidDomain]
    );
    assert_eq!(
        codes("type Risk = Text one of \"A\"\ntype Risk = Text one of \"B\", \"B\"\n"),
        [Code::DuplicateDeclaration, Code::InvalidDomain]
    );
    assert_eq!(
        codes("type Offer:\n    price: Number\ntype Offer where nope:\n    price: Number\n"),
        [Code::DuplicateDeclaration, Code::InvalidDomain]
    );
}

/// D2 review #8 (AC-TYP-27, R-TYP-31/32): an operand of `==` isn't a domain position, so its list literal's items
/// aren't membership-checked; a record literal's fields still are, by their declaration.
#[test]
fn ac_typ_27_equality_checks_only_closed_set_literals_and_record_fields() {
    assert_eq!(check("ages: List<Age>", "ages == [200]"), []);
    assert_eq!(check("risks: List<Risk>", "risks == [\"NOPE\"]"), []);
    assert_eq!(
        check("p: Person", "p == Person(age: 200, risk: nothing)"),
        [Code::TypeMismatch]
    );
    assert_eq!(check("r: Risk", "r == \"NOPE\""), [Code::TypeMismatch]);
}

const TEAM: &str = "type Age = Number where value >= 0 and value <= 120
type Score = Number where value >= 0 and value <= 1000
type Team:
    ages: List<Age>
type Player:
    age: Age
    score: Score
";

/// Gate F1 (D-163, R-TYP-30): a list literal with an item of a different domain than the expected element is `VL0204`
/// in a check, an example and a call argument, and `VL0209` in a domain condition, whatever the other items are; the
/// join must not widen it to `List<Number>`.
#[test]
fn gate_f1_mixed_domain_list() {
    let goal = |item: &str| {
        let text = format!(
            "language: velme/0.2\n\n{TEAM}\ngoal MakeTeam(p: Player) -> Team:\n    plan: \"A team.\"\n    check:\n        - {item}\n"
        );
        let (_, diags) = analyze_str(&text);
        diags.into_iter().filter(Diagnostic::is_error).collect::<Vec<_>>()
    };
    for bad in [
        "result == Team(ages: [p.age, p.score])",
        "result == Team(ages: [1, p.score])",
        "result == Team(ages: [p.score, p.age])",
    ] {
        let d = goal(bad);
        assert_eq!(
            d.iter().map(|d| d.code).collect::<Vec<_>>(),
            [Code::TypeMismatch],
            "{bad}: {d:#?}"
        );
        assert_eq!(d[0].message, "Expected Age, but got Score.", "{bad}");
    }
    // Every offending item is reported; an item of the right domain or of its base is fine.
    assert_eq!(goal("result == Team(ages: [p.score, p.age, p.score])").len(), 2);
    assert!(goal("result == Team(ages: [p.age, p.age, 3])").is_empty());
    // Nested lists meet the same rule through the inner list's expected type.
    let nested = format!(
        "language: velme/0.2\n\n{TEAM}type Teams:\n    teams: List<List<Age>>\n\ngoal G(p: Player) -> Teams:\n    plan: \"x\"\n    check:\n        - result == Teams(teams: [[p.age], [p.score]])\n"
    );
    let d: Vec<_> = analyze_str(&nested)
        .1
        .into_iter()
        .filter(Diagnostic::is_error)
        .collect();
    assert_eq!(
        d.iter().map(|d| d.code).collect::<Vec<_>>(),
        [Code::TypeMismatch],
        "{d:#?}"
    );
    // A domain condition reports it as `VL0209`.
    let condition =         "language: velme/0.2\n\ntype Age = Number where value >= 0 and value <= 120\ntype Score = Number where value >= 0 and value <= 1000\ntype Team:\n    ages: List<Age>\ntype Player where Team(ages: [value.age, value.score]).ages.length == 2:\n    age: Age\n    score: Score\n".to_string();
    let d: Vec<_> = analyze_str(&condition)
        .1
        .into_iter()
        .filter(Diagnostic::is_error)
        .collect();
    assert_eq!(
        d.iter().map(|d| d.code).collect::<Vec<_>>(),
        [Code::InvalidDomain],
        "{d:#?}"
    );
    assert!(d[0].message.contains("expected Age, but got Score"), "{}", d[0].message);
}

/// Gate F1: sema and the IR validator share one predicate, so they can't drift.
#[test]
fn gate_f1_domain_mismatch_is_shared() {
    use velme_sema::hir::{DomainId, Type, domain_mismatch};
    let age = Type::Domain(DomainId(0), Box::new(Type::Number));
    let score = Type::Domain(DomainId(1), Box::new(Type::Number));
    assert!(domain_mismatch(&score, &age));
    assert!(domain_mismatch(
        &Type::List(Box::new(score.clone())),
        &Type::List(Box::new(age.clone()))
    ));
    assert!(!domain_mismatch(&age, &age));
    assert!(!domain_mismatch(&Type::Number, &age), "a base fills a domain position");
    assert!(!domain_mismatch(&age, &Type::Number), "a domain widens to its base");
    assert!(
        !domain_mismatch(&Type::Text, &age),
        "a different base is the position's own error"
    );
}

/// Gate R2 (R-TYP-31): a literal in a domain condition meets the simple constraints of the domains it names, whichever
/// is declared first, and in a domain whose name is taken too.
#[test]
fn gate_r2_condition_literals_meet_simple_constraints() {
    let pos = "type Pos = Number where value > 0\ntype Pt:\n    x: Pos\n";
    let q = "type Q = Number where Pt(x: -1).x < value\n";
    for text in [format!("{pos}{q}"), format!("{q}{pos}")] {
        let (_, diags) = analyze_str(&format!("language: velme/0.2\n\n{text}"));
        let errors: Vec<_> = diags.into_iter().filter(Diagnostic::is_error).collect();
        assert_eq!(
            errors.iter().map(|d| d.code).collect::<Vec<_>>(),
            [Code::InvalidDomain],
            "{text}: {errors:#?}"
        );
        assert!(
            errors[0]
                .message
                .contains("`-1` isn\'t a valid `Pos`: it must be more than 0"),
            "{}",
            errors[0].message
        );
    }
    // A member is accepted.
    let (program, diags) = analyze_str(&format!(
        "language: velme/0.2\n\n{pos}type Q = Number where Pt(x: 1).x < value\n"
    ));
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:#?}");
    assert!(program.is_some());
    // A duplicate-named domain is checked and dropped, and still reports it (CC-ERR-04).
    let (_, diags) = analyze_str(&format!(
        "language: velme/0.2\n\n{pos}{q}type Q = Number where Pt(x: -1).x < value\n"
    ));
    let codes: Vec<_> = diags.iter().filter(|d| d.is_error()).map(|d| d.code).collect();
    assert!(
        codes.iter().filter(|c| **c == Code::InvalidDomain).count() >= 2,
        "{diags:#?}"
    );
}
