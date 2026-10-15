use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use velme_ir::{BinaryOperator as Bin, ItemShape, Lambda, Node, ReduceLambda, Type, UnaryOperator as Un, limits};

use super::lex::{Kind, Token};
use super::parse::{Argument, Block, Expr, ExprKind, Statement};
use super::{Form, Lowered, Rejected, Rule, Span, Spans};
use crate::SynthRequest;

#[derive(Clone)]
struct Places {
    span: Span,
    form: Form,
    children: Vec<(String, Places)>,
    depth: usize,
    nodes: usize,
}
#[derive(Clone)]
struct Built {
    node: Node,
    places: Places,
}

#[derive(Default)]
struct Scope {
    names: BTreeMap<String, usize>,
    stack: Vec<String>,
}
impl Scope {
    fn len(&self) -> usize {
        self.stack.len()
    }
    fn contains(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }
    fn insert(&mut self, name: String) {
        *self.names.entry(name.clone()).or_default() += 1;
        self.stack.push(name);
    }
    fn truncate(&mut self, mark: usize) {
        while self.stack.len() > mark {
            if let Some(name) = self.stack.pop()
                && let Some(count) = self.names.get_mut(&name)
            {
                *count -= 1;
                if *count == 0 {
                    self.names.remove(&name);
                }
            }
        }
    }
}

struct Lower<'r> {
    request: &'r SynthRequest,
    scope: Scope,
    counter: usize,
    nodes: usize,
    blocks: usize,
}

pub(super) fn body(block: &Block, request: &SynthRequest) -> Result<Lowered, Rejected> {
    let mut lower = Lower {
        request,
        scope: Scope::default(),
        counter: 0,
        nodes: 0,
        blocks: 0,
    };
    let built = lower.block(block, 0)?;
    let mut spans = Spans::default();
    let mut stack = vec![("/body".to_owned(), built.places)];
    while let Some((path, places)) = stack.pop() {
        spans.0.insert(path.clone(), (places.span, places.form));
        for (suffix, child) in places.children.into_iter().rev() {
            stack.push((format!("{path}/{suffix}"), child));
        }
    }
    Ok(Lowered {
        body: built.node,
        spans,
    })
}

fn word(token: &Token) -> &str {
    match &token.kind {
        Kind::Word(s) => s,
        _ => "",
    }
}
fn pointer(text: &str) -> String {
    text.replace('~', "~0").replace('/', "~1")
}

impl Lower<'_> {
    fn make(&mut self, node: Node, span: Span, form: Form, children: Vec<(String, Places)>) -> Result<Built, Rejected> {
        self.nodes += 1;
        let depth = 1 + children.iter().map(|(_, p)| p.depth).max().unwrap_or(0);
        if self.nodes > limits::MAX_NODES || depth > limits::MAX_DEPTH {
            return Err(Rule::Limit.reject(span));
        }
        let nodes = 1 + children.iter().map(|(_, p)| p.nodes).sum::<usize>();
        Ok(Built {
            node,
            places: Places {
                span,
                form,
                children,
                depth,
                nodes,
            },
        })
    }
    fn plain(&mut self, node: Node, span: Span) -> Result<Built, Rejected> {
        self.make(node, span, Form::Other, Vec::new())
    }
    fn literal(&mut self, ty: Type, value: Value, span: Span) -> Result<Built, Rejected> {
        self.plain(
            Node::Literal {
                ty,
                value: value.into(),
            },
            span,
        )
    }
    fn binary(&mut self, op: Bin, left: Built, right: Built, span: Span, form: Form) -> Result<Built, Rejected> {
        self.make(
            Node::BinaryOp {
                op,
                left: Box::new(left.node),
                right: Box::new(right.node),
            },
            span,
            form,
            vec![("left".to_owned(), left.places), ("right".to_owned(), right.places)],
        )
    }
    fn unary(&mut self, op: Un, arg: Built, span: Span) -> Result<Built, Rejected> {
        self.make(
            Node::UnaryOp {
                op,
                arg: Box::new(arg.node),
            },
            span,
            Form::Other,
            vec![("arg".to_owned(), arg.places)],
        )
    }
    fn condition(&mut self, cond: Built, then: Built, other: Built, span: Span) -> Result<Built, Rejected> {
        self.make(
            Node::Condition {
                cond: Box::new(cond.node),
                then: Box::new(then.node),
                otherwise: Box::new(other.node),
            },
            span,
            Form::Other,
            vec![
                ("cond".to_owned(), cond.places),
                ("then".to_owned(), then.places),
                ("else".to_owned(), other.places),
            ],
        )
    }
    fn builtin(&mut self, name: &str, args: Vec<Built>, span: Span) -> Result<Built, Rejected> {
        let (nodes, places): (Vec<Node>, Vec<(String, Places)>) = args
            .into_iter()
            .enumerate()
            .map(|(i, b)| (b.node, (format!("args/{i}"), b.places)))
            .unzip();
        let form = if name == "round" {
            Form::Round(nodes.len())
        } else {
            Form::Other
        };
        self.make(
            Node::Builtin {
                name: name.to_owned(),
                args: nodes,
            },
            span,
            form,
            places,
        )
    }
    fn narrow(&mut self, of: Built, default: Built, span: Span) -> Result<Built, Rejected> {
        self.make(
            Node::Narrow {
                of: Box::new(of.node),
                default: Box::new(default.node),
            },
            span,
            Form::Other,
            vec![("of".to_owned(), of.places), ("default".to_owned(), default.places)],
        )
    }
    fn bind(&mut self, token: &Token) -> Result<String, Rejected> {
        let name = word(token);
        let reserved = name
            .strip_prefix("__v")
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
        if reserved
            || self.scope.contains(name)
            || self.request.signature.params.iter().any(|p| p.name == name)
            || self.request.locals.iter().any(|p| p.name == name)
        {
            return Err(Rule::Binding.reject(token.span));
        }
        self.scope.insert(name.to_owned());
        Ok(name.to_owned())
    }
    fn fresh(&mut self) -> String {
        loop {
            let name = format!("__v{}", self.counter);
            self.counter += 1;
            if !self.request.signature.params.iter().any(|p| p.name == name)
                && !self.request.locals.iter().any(|p| p.name == name)
            {
                return name;
            }
        }
    }
    fn copy(&mut self, built: &Built) -> Result<Built, Rejected> {
        if self.nodes + built.places.nodes > limits::MAX_NODES {
            return Err(Rule::Limit.reject(built.places.span));
        }
        self.nodes += built.places.nodes;
        Ok(built.clone())
    }

    fn block(&mut self, block: &Block, offset: usize) -> Result<Built, Rejected> {
        let scope = self.scope.len();
        let result = self.statements(block, offset);
        self.scope.truncate(scope);
        result
    }
    fn statements(&mut self, block: &Block, offset: usize) -> Result<Built, Rejected> {
        self.blocks += 1;
        if self.blocks > limits::MAX_DEPTH {
            return Err(Rule::Limit.reject(block.span));
        }
        let result = self.statements_inner(block, offset);
        self.blocks -= 1;
        result
    }
    fn statements_inner(&mut self, block: &Block, offset: usize) -> Result<Built, Rejected> {
        let mut bind = Vec::new();
        let mut places = Vec::new();
        let mut start = None;
        let mut cursor = offset;
        while let Some(Statement::Assign(name, value)) = block.statements.get(cursor) {
            start.get_or_insert(name.span);
            // The binding starts before its value in lowering order.
            let bound = self.bind(name)?;
            self.scope.truncate(self.scope.len() - 1);
            let value = self.expr(value)?;
            self.scope.insert(bound.clone());
            places.push((format!("bind/{}/1", bind.len()), value.places));
            bind.push((bound, value.node));
            cursor += 1;
        }
        let statement = block
            .statements
            .get(cursor)
            .ok_or_else(|| Rule::Return.reject(block.span))?;
        let result = match statement {
            Statement::Return(expr) => self.expr(expr)?,
            Statement::If {
                branches,
                otherwise,
                span,
            } => {
                // Conditions and branches lower in text order, before the continuation.
                let mut built = Vec::new();
                for (cond, branch) in branches {
                    built.push((self.expr(cond)?, self.block(branch, 0)?));
                }
                let mut rest = if let Some(other) = otherwise {
                    self.block(other, 0)?
                } else {
                    self.statements(block, cursor + 1)?
                };
                let span = if otherwise.is_some() {
                    *span
                } else {
                    span.through(block.span)
                };
                for (cond, then) in built.into_iter().rev() {
                    rest = self.condition(cond, then, rest, span)?;
                }
                rest
            }
            Statement::Assign(_, _) => return Err(Rule::Syntax.reject(block.span)),
        };
        if let Some(start) = start {
            places.push(("body".to_owned(), result.places));
            self.make(
                Node::Let {
                    bind,
                    body: Box::new(result.node),
                },
                start.through(block.span),
                Form::Other,
                places,
            )
        } else {
            Ok(result)
        }
    }

    fn expr(&mut self, expr: &Expr) -> Result<Built, Rejected> {
        let span = expr.span;
        match expr.kind.as_ref() {
            ExprKind::Number(text) => {
                let value = serde_json::Number::from_str(text)
                    .map(Value::Number)
                    .map_err(|_| Rule::Literal.reject(span))?;
                self.literal(Type::Number {}, value, span)
            }
            ExprKind::Text(text) => self.literal(Type::Text {}, Value::String(text.clone()), span),
            ExprKind::Boolean(value) => self.literal(Type::Boolean {}, Value::Bool(*value), span),
            ExprKind::Nothing => self.literal(Type::Nothing {}, Value::Null, span),
            ExprKind::Name(name) => {
                let node =
                    if !self.scope.contains(name) && self.request.signature.params.iter().any(|p| p.name == *name) {
                        Node::Input { name: name.clone() }
                    } else {
                        Node::Local { name: name.clone() }
                    };
                self.plain(node, span)
            }
            ExprKind::Field(of, fields) => self.fields(of, fields, span),
            ExprKind::List(items) | ExprKind::Tuple(items) => self.list(items, span),
            ExprKind::Unary(op, value) => self.unary_expr(op, value, span),
            ExprKind::Chain(first, rest) => self.chain(first, rest, span),
            ExprKind::Compare(first, rest) => self.comparison(first, rest, span),
            ExprKind::Conditional(then, cond, other) => self.conditional_expr(then, cond, other, span),
            ExprKind::Call(name, args) => self.call(name, args, span),
            ExprKind::Comprehension(generator) => self.comprehension(generator),
            ExprKind::Lambda(_, _) | ExprKind::Generator { .. } => Err(Rule::Expression.reject(span)),
        }
    }

    fn fields(&mut self, of: &Expr, fields: &[(String, Span)], span: Span) -> Result<Built, Rejected> {
        let mut value = self.expr(of)?;
        for (field, end) in fields {
            let span = Span {
                start: span.start,
                end: end.end,
            };
            value = self.make(
                Node::FieldGet {
                    of: Box::new(value.node),
                    field: field.clone(),
                },
                span,
                Form::Other,
                vec![("of".to_owned(), value.places)],
            )?;
        }
        Ok(value)
    }

    fn list(&mut self, items: &[Expr], span: Span) -> Result<Built, Rejected> {
        let mut nodes = Vec::new();
        let mut places = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let item = self.expr(item)?;
            nodes.push(item.node);
            places.push((format!("items/{i}"), item.places));
        }
        self.make(
            Node::List {
                of: Type::Nothing {},
                items: nodes,
            },
            span,
            Form::Other,
            places,
        )
    }

    fn unary_expr(&mut self, op: &str, value: &Expr, span: Span) -> Result<Built, Rejected> {
        let value = self.expr(value)?;
        self.unary(if op == "not" { Un::Not } else { Un::Neg }, value, span)
    }

    fn chain(&mut self, first: &Expr, rest: &[(String, Expr)], span: Span) -> Result<Built, Rejected> {
        let mut value = self.expr(first)?;
        for (i, (op, right)) in rest.iter().enumerate() {
            let place = if i + 1 == rest.len() {
                span
            } else {
                first.span.through(right.span)
            };
            let right = self.expr(right)?;
            value = self.binary(
                binary(op),
                value,
                right,
                place,
                if op == "or" { Form::Or } else { Form::Other },
            )?;
        }
        Ok(value)
    }

    fn conditional_expr(&mut self, then: &Expr, cond: &Expr, other: &Expr, span: Span) -> Result<Built, Rejected> {
        let then = self.expr(then)?;
        let cond = self.expr(cond)?;
        let other = self.expr(other)?;
        self.condition(cond, then, other, span)
    }

    fn comparison(&mut self, first: &Expr, rest: &[(String, Expr)], span: Span) -> Result<Built, Rejected> {
        let mut left = self.expr(first)?;
        let mut previous_span = first.span;
        let mut previous_none = matches!(first.kind.as_ref(), ExprKind::Nothing);
        let mut result = None;
        for (i, (op, expr)) in rest.iter().enumerate() {
            let place = if rest.len() == 1 {
                span
            } else {
                previous_span.through(expr.span)
            };
            let right = self.expr(expr)?;
            let copy = if i + 1 < rest.len() {
                Some(self.copy(&right)?)
            } else {
                None
            };
            let none = matches!(expr.kind.as_ref(), ExprKind::Nothing);
            let compare = if op == "in" || op == "not in" {
                let contains = self.builtin("contains", vec![right, left], place)?;
                if op == "not in" {
                    self.unary(Un::Not, contains, place)?
                } else {
                    contains
                }
            } else {
                let (left, right) = if previous_none && !none && matches!(op.as_str(), "==" | "!=") {
                    (right, left)
                } else {
                    (left, right)
                };
                let form = if (none || previous_none) && matches!(op.as_str(), "==" | "!=" | "is" | "is not") {
                    Form::NoneTest
                } else {
                    Form::Other
                };
                self.binary(binary(op), left, right, place, form)?
            };
            result = Some(if let Some(result) = result {
                self.binary(Bin::And, result, compare, span, Form::Other)?
            } else {
                compare
            });
            left = if let Some(copy) = copy {
                copy
            } else {
                return result.ok_or_else(|| Rule::Syntax.reject(span));
            };
            previous_none = none;
            previous_span = expr.span;
        }
        result.ok_or_else(|| Rule::Syntax.reject(span))
    }

    fn comprehension(&mut self, generator: &Expr) -> Result<Built, Rejected> {
        let ExprKind::Generator {
            item,
            var,
            list,
            predicate,
            ..
        } = generator.kind.as_ref()
        else {
            return Err(Rule::Shape.reject(generator.span));
        };
        let scope = self.scope.len();
        let param = word(var).to_owned();
        self.scope.insert(param.clone());
        // Item text precedes the iterable and predicate, but its variable scopes only item and predicate.
        let identity_filter =
            predicate.is_some() && matches!(item.kind.as_ref(), ExprKind::Name(name) if name == &param);
        let mapped = if identity_filter { None } else { Some(self.expr(item)?) };
        self.scope.truncate(scope);
        self.bind(var)?;
        self.scope.truncate(scope);
        let mut list = self.expr(list)?;
        self.scope.insert(param.clone());
        if let Some(predicate) = predicate {
            let pred = self.expr(predicate)?;
            list = self.collection("filter", list, param.clone(), pred, generator.span, false)?;
            if identity_filter {
                self.scope.truncate(scope);
                return Ok(list);
            }
        }
        self.scope.truncate(scope);
        self.collection(
            "map",
            list,
            param,
            mapped.ok_or_else(|| Rule::Shape.reject(generator.span))?,
            generator.span,
            false,
        )
    }

    fn collection(
        &mut self,
        kind: &str,
        list: Built,
        param: String,
        body: Built,
        span: Span,
        descending: bool,
    ) -> Result<Built, Rejected> {
        let key = if kind == "sorted" { "key/body" } else { "fn/body" };
        let places = vec![("list".to_owned(), list.places), (key.to_owned(), body.places)];
        let func = Lambda {
            param,
            body: Box::new(body.node),
        };
        let list = Box::new(list.node);
        let node = match kind {
            "map" => Node::Map {
                list,
                func,
                items: ItemShape::default(),
            },
            "filter" => Node::Filter { list, func },
            "next" => Node::Find { list, func },
            "all" => Node::All { list, func },
            "any" => Node::Any { list, func },
            "sorted" => Node::Sort {
                list,
                key: func,
                descending,
            },
            _ => return Err(Rule::Shape.reject(span)),
        };
        self.make(node, span, Form::Other, places)
    }

    fn call(&mut self, name: &str, args: &[Argument], span: Span) -> Result<Built, Rejected> {
        match name {
            "len" | "unwrap_or" => {
                let first = self.expr(&args.first().ok_or_else(|| Rule::Shape.reject(span))?.value)?;
                if name == "len" {
                    return self.builtin("length", vec![first], span);
                }
                let default = self.expr(&args.get(1).ok_or_else(|| Rule::Shape.reject(span))?.value)?;
                return self.narrow(first, default, span);
            }
            "sorted" => return self.sorted(args, span),
            "reduce" => return self.reduce(args, span),
            "next" => return self.find(args, span),
            "all" | "any" | "sum"
                if args
                    .first()
                    .is_some_and(|a| matches!(a.value.kind.as_ref(), ExprKind::Generator { .. })) =>
            {
                return self.quantified(name, &args.first().ok_or_else(|| Rule::Shape.reject(span))?.value, span);
            }
            "all" | "any" => {
                let param = self.fresh();
                let list = self.expr(&args.first().ok_or_else(|| Rule::Shape.reject(span))?.value)?;
                let body = self.plain(Node::Local { name: param.clone() }, span)?;
                return self.collection(name, list, param, body, span, false);
            }
            _ => {}
        }
        self.catalog_call(name, args, span)
    }

    fn catalog_call(&mut self, name: &str, args: &[Argument], span: Span) -> Result<Built, Rejected> {
        if self.request.types.iter().any(|t| t.name == name) {
            return self.record(name, args, span);
        }

        if !self
            .request
            .builtins
            .iter()
            .any(|b| b.name == name && velme_builtins::Builtin::find(name).is_some_and(|b| b.function.is_some()))
            && let Some(rejection) = python_builtin(
                name,
                Span {
                    start: span.start,
                    end: span.start + name.len(),
                },
            )
        {
            return Err(rejection);
        }
        let mut built = Vec::new();
        for arg in args {
            if let Some(name) = &arg.name {
                return Err(Rule::Expression
                    .reject(name.span)
                    .hint("a builtin's arguments are positional"));
            }
            built.push(self.expr(&arg.value)?);
        }
        self.builtin(name, built, span)
    }

    fn record(&mut self, name: &str, args: &[Argument], span: Span) -> Result<Built, Rejected> {
        let mut seen = BTreeSet::new();
        if args
            .iter()
            .any(|a| a.name.as_ref().is_none_or(|n| !seen.insert(word(n))))
        {
            return Err(Rule::Shape
                .reject(span)
                .hint("Type(field=value, …) with each field once"));
        }
        let mut fields = BTreeMap::new();
        let mut places = BTreeMap::new();
        for arg in args {
            let field = arg.name.as_ref().map(word).unwrap_or_default();
            let value = self.expr(&arg.value)?;
            places.insert(field.to_owned(), value.places);
            fields.insert(field.to_owned(), value.node);
        }
        self.make(
            Node::Record {
                ty: name.to_owned(),
                fields,
            },
            span,
            Form::Other,
            places
                .into_iter()
                .map(|(n, p)| (format!("fields/{}", pointer(&n)), p))
                .collect(),
        )
    }

    fn quantified(&mut self, name: &str, generator: &Expr, span: Span) -> Result<Built, Rejected> {
        if name == "sum" {
            let list = self.comprehension(generator)?;
            return self.builtin(name, vec![list], span);
        }
        let ExprKind::Generator {
            item,
            var,
            list,
            predicate,
            ..
        } = generator.kind.as_ref()
        else {
            return Err(Rule::Shape.reject(span));
        };
        let scope = self.scope.len();
        let param = word(var).to_owned();
        self.scope.insert(param.clone());
        let body = self.expr(item)?;
        self.scope.truncate(scope);
        self.bind(var)?;
        self.scope.truncate(scope);
        let mut list = self.expr(list)?;
        self.scope.insert(param.clone());
        if let Some(predicate) = predicate {
            let pred = self.expr(predicate)?;
            list = self.collection("filter", list, param.clone(), pred, generator.span, false)?;
        }
        self.scope.truncate(scope);
        self.collection(name, list, param, body, span, false)
    }

    fn sorted(&mut self, args: &[Argument], span: Span) -> Result<Built, Rejected> {
        let key = args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.is("key")));
        let param = if key.is_none() { Some(self.fresh()) } else { None };
        let list = self.expr(&args.first().ok_or_else(|| Rule::Shape.reject(span))?.value)?;
        let mut descending = false;
        let mut lambda = None;
        for arg in args.iter().skip(1) {
            if arg.name.as_ref().is_some_and(|n| n.is("reverse")) {
                descending = matches!(arg.value.kind.as_ref(), ExprKind::Boolean(true));
            } else if let ExprKind::Lambda(params, body) = arg.value.kind.as_ref() {
                let scope = self.scope.len();
                let param = self.bind(params.first().ok_or_else(|| Rule::Shape.reject(span))?)?;
                let body = self.expr(body)?;
                self.scope.truncate(scope);
                lambda = Some((param, body));
            }
        }
        let (param, body) = if let Some(lambda) = lambda {
            lambda
        } else {
            let param = param.unwrap_or_default();
            let body = self.plain(Node::Local { name: param.clone() }, span)?;
            (param, body)
        };
        self.collection("sorted", list, param, body, span, descending)
    }

    fn reduce(&mut self, args: &[Argument], span: Span) -> Result<Built, Rejected> {
        let Some(ExprKind::Lambda(params, expr)) = args.first().map(|a| a.value.kind.as_ref()) else {
            return Err(Rule::Shape.reject(span));
        };
        let scope = self.scope.len();
        let acc = self.bind(params.first().ok_or_else(|| Rule::Shape.reject(span))?)?;
        let param = self.bind(params.get(1).ok_or_else(|| Rule::Shape.reject(span))?)?;
        let body = self.expr(expr)?;
        self.scope.truncate(scope);
        let list = self.expr(&args.get(1).ok_or_else(|| Rule::Shape.reject(span))?.value)?;
        let init = self.expr(&args.get(2).ok_or_else(|| Rule::Shape.reject(span))?.value)?;
        self.make(
            Node::Reduce {
                list: Box::new(list.node),
                init: Box::new(init.node),
                func: ReduceLambda {
                    acc,
                    param,
                    body: Box::new(body.node),
                },
            },
            span,
            Form::Other,
            vec![
                ("list".to_owned(), list.places),
                ("init".to_owned(), init.places),
                ("fn/body".to_owned(), body.places),
            ],
        )
    }

    fn find(&mut self, args: &[Argument], span: Span) -> Result<Built, Rejected> {
        let Some(ExprKind::Generator {
            var,
            list,
            predicate: Some(predicate),
            ..
        }) = args.first().map(|a| a.value.kind.as_ref())
        else {
            return Err(Rule::Shape.reject(span));
        };
        let scope = self.scope.len();
        let param = self.bind(var)?;
        self.scope.truncate(scope);
        let list = self.expr(list)?;
        self.scope.insert(param.clone());
        let pred = self.expr(predicate)?;
        self.scope.truncate(scope);
        let found = self.collection("next", list, param, pred, span, false)?;
        let default = &args.get(1).ok_or_else(|| Rule::Shape.reject(span))?.value;
        if matches!(default.kind.as_ref(), ExprKind::Nothing) {
            Ok(found)
        } else {
            let default = self.expr(default)?;
            self.narrow(found, default, span)
        }
    }
}

/// `notation-10` with its hint by name, when `name` is a listed Python built-in (R-SYNTH-59).
pub(super) fn python_builtin(name: &str, span: Span) -> Option<Rejected> {
    const PYTHON: [&str; 19] = [
        "max",
        "min",
        "int",
        "float",
        "str",
        "bool",
        "list",
        "dict",
        "set",
        "tuple",
        "print",
        "enumerate",
        "zip",
        "map",
        "filter",
        "pow",
        "divmod",
        "isinstance",
        "type",
    ];
    if !PYTHON.contains(&name) {
        return None;
    }
    let rejection = Rule::PythonBuiltin.reject(span);
    Some(match name {
        "max" | "min" => rejection.hint("`max(a, b)` is `a if a >= b else b`, and `maximum(xs)` is `None` for `[]`"),
        "int" => rejection.hint("floor(x)"),
        "str" => rejection.hint("to_text(x)"),
        "pow" | "divmod" => rejection.hint(Rule::Operator.reject(span).hint.as_deref().unwrap_or_default()),
        "map" | "filter" | "enumerate" | "zip" => rejection.hint("write a comprehension"),
        _ => rejection,
    })
}

use std::str::FromStr;

fn binary(op: &str) -> Bin {
    match op {
        "+" => Bin::Add,
        "-" => Bin::Sub,
        "*" => Bin::Mul,
        "/" => Bin::Div,
        "<" => Bin::Lt,
        "<=" => Bin::Le,
        ">" => Bin::Gt,
        ">=" => Bin::Ge,
        "!=" | "is not" => Bin::Ne,
        "and" => Bin::And,
        "or" => Bin::Or,
        _ => Bin::Eq,
    }
}
