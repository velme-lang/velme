//! The JSON value mapping of `language/11` §10 (D-23): decoding against a type and encoding output.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use velme_builtins::limits::MAX_LIST_SIZE;
use velme_builtins::{Number, Value};
use velme_diagnostics::Code;
use velme_diagnostics::Span;
use velme_ir::{
    DecodeError, DecodeProblem, SHOWN_CHARS, SHOWN_ITEMS, decode_str, decode_value, display_value, encode_value,
    from_json_str,
};
use velme_sema::hir::{FieldDef, Program, RecordType, Type, TypeId};
use velme_sema::{SourceFile, analyze};

const TYPES: &str = "\
type Player:\n    name: Text\n    score: Number\n    best: Number?\n\n\
type Team:\n    players: List<Player>\n    captain: Player?\n";

fn program() -> Program {
    let (program, diags) = analyze(&SourceFile::new("types.velme", TYPES));
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:#?}");
    program.expect("a program without errors")
}

fn record(program: &Program, name: &str) -> Type {
    let id = program.types.iter().position(|t| t.name == name).expect("declared");
    Type::Record(TypeId(id))
}

fn decode(text: &str, ty: &Type, program: &Program) -> Result<Value, DecodeError> {
    let json: serde_json::Value = from_json_str(text).expect("JSON");
    decode_value(&json, ty, program)
}

fn reject(text: &str, ty: &Type, program: &Program) -> DecodeError {
    decode(text, ty, program).expect_err("rejected")
}

/// `text` decoded as `ty` and encoded again.
fn round_trip(text: &str, ty: &Type, program: &Program) -> String {
    encode_value(&decode(text, ty, program).unwrap_or_else(|e| panic!("{text}: {e:?}")))
}

#[test]
fn ac_typ_11_extra_field_and_null_are_invalid_input() {
    let program = program();
    let player = record(&program, "Player");
    let error = reject(
        r#"{"name": "Ada", "score": 3, "best": null, "scroe": 4}"#,
        &player,
        &program,
    );
    assert_eq!(error.code(), Code::InvalidInput);
    assert_eq!(error.pointer(), "/scroe");
    assert_eq!(error.problem.to_string(), "`Player` has no field `scroe`");
    let DecodeProblem::UnknownField { help, .. } = &error.problem else {
        panic!("{error:?}")
    };
    assert!(help.as_deref().is_some_and(|h| h.contains("score")), "{help:?}");

    let error = reject(r#"{"name": "Ada", "score": null, "best": null}"#, &player, &program);
    assert_eq!(error.code(), Code::InvalidInput);
    assert_eq!(error.pointer(), "/score");
    assert_eq!(error.problem.to_string(), "this value isn't Number");

    // A missing key, even for a nullable field (R-TYP-17).
    let error = reject(r#"{"name": "Ada", "score": 1}"#, &player, &program);
    assert_eq!(error.code(), Code::InvalidInput);
    assert_eq!(error.problem.to_string(), "a `Player` needs the field `best`");
    assert_eq!(reject("null", &player, &program).code(), Code::InvalidInput);
}

#[test]
fn ac_typ_14_output_lists_fields_in_declaration_order() {
    let program = program();
    let team = record(&program, "Team");
    let input = r#"{"captain": null, "players": [{"score": 2.50, "best": 7, "name": "Bo"}]}"#;
    assert_eq!(
        round_trip(input, &team, &program),
        r#"{"players":[{"name":"Bo","score":2.5,"best":7}],"captain":null}"#
    );
}

#[test]
fn ac_typ_15_number_input_round_trips_exactly() {
    let program = program();
    for (input, output) in [
        ("0.1", "0.1"),
        ("-0", "0"),
        ("1e2", "100"),
        ("2.50", "2.5"),
        ("0.3333333333333333333333333333", "0.3333333333333333333333333333"),
        ("79228162514264337593543950335", "79228162514264337593543950335"),
    ] {
        assert_eq!(round_trip(input, &Type::Number, &program), output, "{input}");
    }
    let sum = Number::parse("0.1")
        .and_then(|a| a.checked_add(Number::parse("0.2")?).ok())
        .expect("fits");
    assert_eq!(encode_value(&Value::Number(sum)), "0.3");
}

#[test]
fn json_number_outside_the_range_is_invalid_input() {
    let program = program();
    for input in [
        "1e-29",
        "0.12345678901234567890123456789",
        "79228162514264337593543950336",
        "1e29",
    ] {
        let error = reject(input, &Type::Number, &program);
        assert_eq!(error.code(), Code::InvalidInput, "{input}");
        assert!(
            matches!(error.problem, DecodeProblem::NumberOutOfRange { .. }),
            "{error:?}"
        );
    }
}

#[test]
fn json_mapping_of_every_type() {
    let program = program();
    let optional = Type::Optional(Box::new(Type::Text));
    assert_eq!(decode("null", &optional, &program), Ok(Value::Nothing));
    assert_eq!(decode(r#""hi""#, &optional, &program), Ok(Value::text("hi")));
    assert_eq!(decode("true", &Type::Boolean, &program), Ok(Value::Boolean(true)));
    assert_eq!(decode("null", &Type::Nothing, &program), Ok(Value::Nothing));
    assert_eq!(
        reject("1", &Type::Text, &program).problem.to_string(),
        "this value isn't Text"
    );
    let numbers = Type::List(Box::new(Type::Number));
    assert_eq!(round_trip("[1, 2.0, -0.0]", &numbers, &program), "[1,2,0]");
    let error = reject(r#"[1, "x"]"#, &numbers, &program);
    assert_eq!((error.pointer(), error.code()), ("/1".to_owned(), Code::InvalidInput));
    // Control characters are escaped; other text is written as is.
    assert_eq!(
        round_trip(r#""a\"b\n\u0001é""#, &Type::Text, &program),
        r#""a\"b\n\u0001é""#
    );
}

#[test]
fn json_list_above_the_list_limit_is_a_size_error() {
    let program = program();
    let numbers = Type::List(Box::new(Type::Number));
    let at_limit = format!("[{}]", vec!["0"; MAX_LIST_SIZE as usize].join(","));
    assert!(decode(&at_limit, &numbers, &program).is_ok());
    let above = format!("[{}]", vec!["0"; MAX_LIST_SIZE as usize + 1].join(","));
    let error = reject(&above, &numbers, &program);
    assert_eq!(error.code(), Code::SizeLimitExceeded);
    assert_eq!(error.pointer(), "");
}

#[test]
fn json_mismatch_says_what_was_found() {
    let program = program();
    let player = record(&program, "Player");
    for (text, ty, found) in [
        (r#""x""#, &Type::Number, "text"),
        ("1", &Type::Text, "a number"),
        ("null", &Type::Number, "nothing"),
        ("[1]", &Type::Number, "a list"),
        ("{}", &Type::Number, "a record"),
        ("true", &Type::Number, "true"),
        ("false", &player, "false"),
    ] {
        let DecodeProblem::Mismatch { found: got, .. } = reject(text, ty, &program).problem else {
            panic!("{text}")
        };
        assert_eq!(got, found, "{text}");
    }
}

#[test]
fn json_text_input_rejects_a_repeated_key() {
    let program = program();
    let player = record(&program, "Player");
    let error = decode_str(r#"{"name":"a","name":1}"#, &player, &program).expect_err("repeated key");
    assert_eq!(error.code(), Code::InvalidInput);
    assert_eq!(error.pointer(), "/name");
    assert!(matches!(error.problem, DecodeProblem::InvalidJson { .. }), "{error:?}");
    let error = decode_str("{", &player, &program).expect_err("malformed");
    assert_eq!(error.code(), Code::InvalidInput);
    let value = decode_str(r#"{"name":"a","score":1,"best":null}"#, &player, &program).expect("decodes");
    assert_eq!(encode_value(&value), r#"{"name":"a","score":1,"best":null}"#);
}

#[test]
fn a_type_that_already_has_an_error_accepts_any_json() {
    let program = Program {
        language_version: "0.1".to_owned(),
        types: vec![RecordType {
            name: "Broken".to_owned(),
            fields: vec![FieldDef {
                name: "x".to_owned(),
                ty: Type::List(Box::new(Type::Error)),
                span: Span::default(),
            }],
            span: Span::default(),
        }],
        goals: Vec::new(),
    };
    assert!(decode(r#"[1, "a", null]"#, &Type::List(Box::new(Type::Error)), &program).is_ok());
    assert!(decode(r#"{"x": [true, {}]}"#, &Type::Record(TypeId(0)), &program).is_ok());
    // The error is inside the list, so a non-list is still wrong.
    let error = reject(r#"{"x": 1}"#, &Type::Record(TypeId(0)), &program);
    assert_eq!(error.pointer(), "/x");
}

/// The human view cuts long lists and texts with a count of what was left out, at any depth (`runtime/30` R-RUN-20).
#[test]
fn display_cuts_long_lists_and_texts() {
    let numbers = |n: i64| Value::list((0..n).map(|i| Value::Number(Number::from(i))).collect());
    assert_eq!(display_value(&numbers(10)), encode_value(&numbers(10)));
    assert_eq!(display_value(&numbers(12)), "[0,1,2,3,4,5,6,7,8,9,…(+2 items)]");
    assert_eq!(SHOWN_ITEMS, 10);

    let text = |n: usize| Value::text(&"é".repeat(n));
    assert_eq!(display_value(&text(SHOWN_CHARS)), encode_value(&text(SHOWN_CHARS)));
    let long = display_value(&Value::list(vec![text(SHOWN_CHARS + 3)]));
    assert_eq!(long, format!("[\"{}\"…(+3 characters)]", "é".repeat(SHOWN_CHARS)));
}
