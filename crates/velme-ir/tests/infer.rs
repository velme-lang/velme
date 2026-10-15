//! Contextual list types use the validator's own typing (R-SYNTH-58).
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use serde_json::{Value, json};
use velme_ir::{Node, Origin, Request, Type, infer_list_types};
use velme_sema::SourceFile;

const SOURCE: &str = "language: velme/0.1

type Box:
    values: List<Number>

goal Lists(xs: List<Number>, opt: List<Number>?, c: Boolean) -> List<Number>:
    plan: \"Return lists.\"

goal Nested() -> List<List<Number>>:
    plan: \"Return nested lists.\"
";

fn empty() -> Value {
    json!({"kind":"list", "of":{"t":"Text"}, "items":[]})
}
fn input(name: &str) -> Value {
    json!({"kind":"input", "name":name})
}
fn number() -> Value {
    json!({"kind":"literal", "type":{"t":"Number"}, "value":1})
}
fn infer(value: Value, goal: usize) -> (Node, Vec<String>) {
    let (program, diagnostics) = velme_sema::analyze(&SourceFile::new("test.velme", SOURCE));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let program = program.expect("program");
    let mut node = serde_json::from_value(value).expect("node");
    let untyped = infer_list_types(
        &mut node,
        &Request {
            program: &program,
            goal: velme_sema::hir::GoalId(goal),
            calls: &[],
            origin: Origin::Candidate,
        },
    );
    (node, untyped)
}

#[test]
fn ac_synth_59_every_expected_context_overrides_a_list_annotation() {
    for value in [
        empty(),
        json!({"kind":"let", "bind":[], "body":empty()}),
        json!({"kind":"if", "cond":input("c"), "then":empty(), "else":input("xs")}),
        json!({"kind":"binary", "op":"eq", "left":empty(), "right":input("xs")}),
        json!({"kind":"record", "type":"Box", "fields":{"values":empty()}}),
        json!({"kind":"builtin", "name":"sum", "args":[empty()]}),
        json!({"kind":"unwrap_or", "of":input("opt"), "default":empty()}),
        json!({"kind":"reduce", "list":input("xs"), "init":empty(), "fn":{"acc":"a", "param":"i", "body":{"kind":"local", "name":"a"}}}),
    ] {
        let (node, untyped) = infer(value.clone(), 0);
        assert!(untyped.is_empty(), "{value}: {untyped:?}");
        for node in node.preorder() {
            if let Node::List { of, .. } = node {
                assert_eq!(*of, Type::Number {}, "{value}");
            }
        }
    }
}

#[test]
fn ac_synth_59_siblings_items_and_unknown_types_follow_the_table() {
    let nonempty = json!({"kind":"list", "of":{"t":"Text"}, "items":[number()]});
    for value in [
        json!({"kind":"if", "cond":input("c"), "then":empty(), "else":nonempty}),
        json!({"kind":"binary", "op":"eq", "left":empty(), "right":nonempty}),
    ] {
        let (node, untyped) = infer(json!({"kind":"let", "bind":[["a",value]], "body":input("xs")}), 0);
        assert!(untyped.is_empty());
        for node in node.preorder() {
            if let Node::List { of, .. } = node {
                assert_eq!(*of, Type::Number {});
            }
        }
    }
    let nested = json!({"kind":"list", "of":{"t":"Text"}, "items":[empty(),nonempty]});
    let (node, untyped) = infer(nested, 1);
    assert!(untyped.is_empty());
    let lists: Vec<_> = node
        .preorder()
        .into_iter()
        .filter_map(|n| if let Node::List { of, .. } = n { Some(of) } else { None })
        .collect();
    assert_eq!(
        lists,
        [
            &Type::List {
                of: Box::new(Type::Number {})
            },
            &Type::Number {},
            &Type::Number {}
        ]
    );
    let (_, untyped) = infer(json!({"kind":"let", "bind":[["a",empty()]], "body":input("xs")}), 0);
    assert_eq!(untyped, ["/body/bind/0/1"]);
    let (_, untyped) = infer(json!({"kind":"local", "name":"unknown"}), 0);
    assert!(untyped.is_empty());
    let (node, untyped) = infer(
        json!({"kind":"let", "bind":[["a",{"kind":"list", "of":{"t":"Text"}, "items":[{"kind":"local", "name":"unknown"}]}]], "body":input("xs")}),
        0,
    );
    assert!(untyped.is_empty());
    assert!(node.preorder().iter().any(|n| matches!(
        n,
        Node::List {
            of: Type::Nothing {},
            ..
        }
    )));
}
