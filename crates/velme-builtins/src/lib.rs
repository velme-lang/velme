//! Velme built-ins: the catalog of built-in functions, defined once as data (`language/14` R-BLT-01).
#![forbid(unsafe_code)]

/// The catalog's version, recorded in every artifact manifest and synthesis cache key (R-BLT-09).
pub const BUILTINS_VERSION: &str = "0.1";

/// A type in a built-in's signature. `T` and `U` stand for any type, the same one wherever they repeat in one
/// signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `Number`
    Number,
    /// `Text`
    Text,
    /// `Boolean`
    Boolean,
    /// `T`
    T,
    /// `U`
    U,
    /// `List<…>`
    List(&'static Shape),
    /// `…?`
    Optional(&'static Shape),
    /// A lambda `(…) -> …`, taken only by the collection primitives (§4).
    Lambda(&'static [Shape], &'static Shape),
}

/// One way to call a built-in: its parameter types in order, and its output type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    /// The parameter types.
    pub params: &'static [Shape],
    /// The output type.
    pub output: Shape,
}

/// A built-in function (`language/14` §2, §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Builtin {
    /// Its name, as called.
    pub name: &'static str,
    /// Its signatures; more than one for an overloaded built-in such as `length`.
    pub signatures: &'static [Signature],
    /// Callable from `check` items (R-BLT-02).
    pub in_checks: bool,
    /// Callable from synthesized IR (R-BLT-02).
    pub in_ir: bool,
}

impl Builtin {
    /// The built-in called `name`, if there is one.
    pub fn find(name: &str) -> Option<&'static Builtin> {
        CATALOG.iter().find(|b| b.name == name)
    }
}

const NUMBER: Shape = Shape::Number;
const TEXT: Shape = Shape::Text;
const BOOLEAN: Shape = Shape::Boolean;
const T: Shape = Shape::T;
const U: Shape = Shape::U;
const LIST_T: Shape = Shape::List(&T);
const LIST_NUMBER: Shape = Shape::List(&NUMBER);

const fn sig(params: &'static [Shape], output: Shape) -> Signature {
    Signature { params, output }
}

/// A value built-in (§2): callable from checks and IR.
const fn value(name: &'static str, signatures: &'static [Signature]) -> Builtin {
    Builtin {
        name,
        signatures,
        in_checks: true,
        in_ir: true,
    }
}

/// A collection primitive (§4): IR only, since it takes a lambda.
const fn primitive(name: &'static str, signatures: &'static [Signature]) -> Builtin {
    Builtin {
        name,
        signatures,
        in_checks: false,
        in_ir: true,
    }
}

/// Every built-in of [`BUILTINS_VERSION`], in `language/14` order.
pub const CATALOG: &[Builtin] = &[
    value("length", &[sig(&[LIST_T], NUMBER), sig(&[TEXT], NUMBER)]),
    value(
        "is_empty",
        &[
            sig(&[Shape::Optional(&T)], BOOLEAN),
            sig(&[LIST_T], BOOLEAN),
            sig(&[TEXT], BOOLEAN),
        ],
    ),
    value("maximum", &[sig(&[LIST_NUMBER], Shape::Optional(&NUMBER))]),
    value("minimum", &[sig(&[LIST_NUMBER], Shape::Optional(&NUMBER))]),
    value("sum", &[sig(&[LIST_NUMBER], NUMBER)]),
    value("contains", &[sig(&[LIST_T, T], BOOLEAN)]),
    value("abs", &[sig(&[NUMBER], NUMBER)]),
    value("floor", &[sig(&[NUMBER], NUMBER)]),
    value("ceil", &[sig(&[NUMBER], NUMBER)]),
    value("round", &[sig(&[NUMBER], NUMBER)]),
    value("clamp", &[sig(&[NUMBER, NUMBER, NUMBER], NUMBER)]),
    value("concat", &[sig(&[TEXT, TEXT], TEXT)]),
    value("to_text", &[sig(&[NUMBER], TEXT)]),
    value("range", &[sig(&[NUMBER], LIST_NUMBER)]),
    value("random", &[sig(&[NUMBER, NUMBER], NUMBER)]),
    primitive("map", &[sig(&[LIST_T, Shape::Lambda(&[T], &U)], Shape::List(&U))]),
    primitive("filter", &[sig(&[LIST_T, Shape::Lambda(&[T], &BOOLEAN)], LIST_T)]),
    primitive(
        "find",
        &[sig(&[LIST_T, Shape::Lambda(&[T], &BOOLEAN)], Shape::Optional(&T))],
    ),
    primitive("reduce", &[sig(&[LIST_T, U, Shape::Lambda(&[U, T], &U)], U)]),
    primitive(
        "sort_by",
        &[sig(&[LIST_T, Shape::Lambda(&[T], &NUMBER), BOOLEAN], LIST_T)],
    ),
    primitive("all", &[sig(&[LIST_T, Shape::Lambda(&[T], &BOOLEAN)], BOOLEAN)]),
    primitive("any", &[sig(&[LIST_T, Shape::Lambda(&[T], &BOOLEAN)], BOOLEAN)]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique() {
        for (i, b) in CATALOG.iter().enumerate() {
            assert!(
                CATALOG.iter().skip(i + 1).all(|other| other.name != b.name),
                "{}",
                b.name
            );
        }
    }
}
