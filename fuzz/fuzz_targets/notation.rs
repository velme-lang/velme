//! R-SYNTH-55: UTF-8 bodies always lower or reject; accepted bodies are deterministic and keep every place.
#![no_main]

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use velme_ir::{Origin, Request, calls, validate_detailed};
use velme_sema::hir::{GoalId, Program};
use velme_synth::{build_request, notation};

const SOURCE: &str = include_str!("../../tests/golden/ir/goals.velme");
static PROGRAM: LazyLock<Option<Program>> =
    LazyLock::new(|| velme_sema::analyze(&velme_sema::SourceFile::new("goals.velme", SOURCE)).0);

fuzz_target!(|data: &[u8]| {
    let (Ok(text), Some(program)) = (std::str::from_utf8(data), PROGRAM.as_ref()) else {
        return;
    };
    for (i, _) in program.goals.iter().enumerate() {
        let id = GoalId(i);
        let (Ok(request), Ok(calls)) = (build_request(program, id, SOURCE), calls(program, id)) else {
            continue;
        };
        let ir = Request {
            program,
            goal: id,
            calls: &calls,
            origin: Origin::Candidate,
        };
        if let Ok(mut fresh) = notation::lower_untyped(text, &request) {
            assert_node_spans(&serde_json::to_value(&fresh.body).expect("body"), &fresh.spans);
            for pointer in velme_ir::infer_list_types(&mut fresh.body, &ir) {
                assert!(fresh.spans.exact(&pointer).is_some(), "{pointer}");
            }
        }
        if let Ok(lowered) = notation::lower(text, &request, &ir) {
            let again = notation::lower(text, &request, &ir).expect("deterministic lowering");
            let first = velme_ir::to_canonical_string(&serde_json::to_value(&lowered.body).expect("IR"));
            let second = velme_ir::to_canonical_string(&serde_json::to_value(again.body).expect("IR"));
            assert_eq!(first, second);

            let body = serde_json::to_value(&lowered.body).expect("IR");
            if let Ok(assembled) = request.assemble(&body) {
                if let Err(found) = validate_detailed(&assembled, &ir) {
                    for finding in found {
                        // R-SYNTH-59 leaves envelope findings (such as the whole-document byte cap) without a place.
                        if finding.path.starts_with("/body") {
                            assert!(lowered.spans.at(&finding.path).is_some(), "{}", finding.path);
                        }
                    }
                }
            }
        }
    }
});

fn assert_node_spans(body: &serde_json::Value, spans: &notation::Spans) {
    let mut pending = vec![("/body".to_owned(), body)];
    while let Some((path, value)) = pending.pop() {
        match value {
            serde_json::Value::Object(fields) => {
                if fields.contains_key("kind") {
                    assert!(spans.exact(&path).is_some(), "missing node span: {path}");
                }
                for (key, value) in fields {
                    if !(fields.get("kind").and_then(serde_json::Value::as_str) == Some("literal") && key == "value") {
                        pending.push((format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")), value));
                    }
                }
            }
            serde_json::Value::Array(items) => {
                pending.extend(items.iter().enumerate().map(|(i, v)| (format!("{path}/{i}"), v)));
            }
            _ => {}
        }
    }
}
