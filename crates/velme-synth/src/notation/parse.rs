const TUPLE_HINT: &str = "write a list `[a, b]` (a record is `Type(field=value, …)`)";

use super::lex::{Kind, Lexer, Token};
use super::{Rejected, Rule, Span};

#[derive(Debug)]
pub(super) struct Expr {
    pub kind: Box<ExprKind>,
    pub span: Span,
    grouped: bool,
}

impl Expr {
    fn new(kind: ExprKind, span: Span) -> Self {
        Self {
            kind: Box::new(kind),
            span,
            grouped: false,
        }
    }
}

#[derive(Debug)]
pub(super) enum ExprKind {
    Number(String),
    Text(String),
    Boolean(bool),
    Nothing,
    Name(String),
    Field(Box<Expr>, Vec<(String, Span)>),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Unary(String, Box<Expr>),
    Chain(Box<Expr>, Vec<(String, Expr)>),
    Compare(Box<Expr>, Vec<(String, Expr)>),
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(String, Vec<Argument>),
    Lambda(Vec<Token>, Box<Expr>),
    Generator {
        item: Box<Expr>,
        var: Token,
        list: Box<Expr>,
        predicate: Option<Box<Expr>>,
        parenthesized: bool,
    },
    Comprehension(Box<Expr>),
}

#[derive(Debug)]
pub(super) struct Argument {
    pub name: Option<Token>,
    pub value: Expr,
}

#[derive(Debug)]
pub(super) struct Block {
    pub statements: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug)]
pub(super) enum Statement {
    Assign(Token, Expr),
    Return(Expr),
    If {
        branches: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
        span: Span,
    },
}

pub(super) fn body(text: &str) -> Result<Block, Rejected> {
    Parser {
        lexer: Lexer::new(text),
        tokens: Vec::new(),
        pos: 0,
        nesting: 0,
        end: text.len(),
    }
    .block(None)
}

struct Parser<'t> {
    lexer: Lexer<'t>,
    tokens: Vec<Result<Token, Rejected>>,
    pos: usize,
    nesting: usize,
    end: usize,
}

pub(super) fn keyword(name: &str) -> bool {
    matches!(
        name,
        "and"
            | "elif"
            | "else"
            | "False"
            | "for"
            | "if"
            | "in"
            | "is"
            | "lambda"
            | "None"
            | "not"
            | "or"
            | "return"
            | "True"
    )
}

fn reserved(name: &str) -> bool {
    matches!(
        name,
        "while"
            | "def"
            | "class"
            | "import"
            | "from"
            | "try"
            | "except"
            | "finally"
            | "with"
            | "as"
            | "pass"
            | "break"
            | "continue"
            | "raise"
            | "assert"
            | "del"
            | "global"
            | "nonlocal"
            | "yield"
            | "async"
            | "await"
            | "match"
    )
}

impl Parser<'_> {
    fn peek(&mut self, ahead: usize) -> Result<Token, Rejected> {
        while self.tokens.len() <= self.pos + ahead {
            self.tokens.push(self.lexer.next());
        }
        self.tokens.get(self.pos + ahead).cloned().unwrap_or_else(|| {
            Err(Rule::Syntax.reject(Span {
                start: self.end,
                end: self.end,
            }))
        })
    }
    fn token(&mut self) -> Result<Token, Rejected> {
        self.peek(0)
    }
    fn take(&mut self) -> Result<Token, Rejected> {
        let token = self.token()?;
        self.pos += 1;
        Ok(token)
    }
    fn is(&mut self, s: &str) -> Result<bool, Rejected> {
        Ok(self.token()?.is(s))
    }
    fn eat(&mut self, s: &str) -> Result<bool, Rejected> {
        if self.is(s)? {
            self.take()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn want(&mut self, s: &str) -> Result<Token, Rejected> {
        let t = self.token()?;
        if t.is(s) { self.take() } else { Err(self.unexpected(&t)) }
    }
    fn unexpected(&self, token: &Token) -> Rejected {
        if token.is(";")
            || matches!(&token.kind, Kind::Symbol(s) if ["+=", "-=", "*=", "/=", "//=", "%=", "**="].contains(&s.as_str()))
        {
            Rule::Statement.reject(token.span)
        } else if matches!(&token.kind, Kind::Symbol(s) if ["**", "%", "//", "@", "&", "|", "^", "~", "<<", ">>", ":="].contains(&s.as_str()))
        {
            Rule::Operator.reject(token.span)
        } else if token.is("{") || token.is("}") {
            Rule::Expression.reject(token.span).hint(TUPLE_HINT)
        } else if token.kind == Kind::Indent {
            Rule::Indentation.reject(token.span)
        } else {
            Rule::Syntax.reject(token.span)
        }
    }
    fn enter(&mut self, span: Span) -> Result<(), Rejected> {
        self.nesting += 1;
        if self.nesting > velme_ir::limits::MAX_DEPTH {
            return Err(Rule::Limit.reject(span));
        }
        Ok(())
    }
    fn name(&mut self) -> Result<Token, Rejected> {
        let t = self.token()?;
        if matches!(&t.kind, Kind::Word(s) if !keyword(s)) {
            self.take()
        } else {
            Err(self.unexpected(&t))
        }
    }

    fn block(&mut self, header: Option<Span>) -> Result<Block, Rejected> {
        let start = self.token()?.span;
        let mut statements = Vec::new();
        let mut terminal = false;
        loop {
            let t = self.token()?;
            if matches!(t.kind, Kind::End | Kind::Dedent) {
                if !terminal {
                    return Err(Rule::Return.reject(header.unwrap_or(Span {
                        start: self.end,
                        end: self.end,
                    })));
                }
                let end = self
                    .tokens
                    .get(self.pos.saturating_sub(1))
                    .and_then(|r| r.as_ref().ok())
                    .map_or(start, |t| t.span);
                return Ok(Block {
                    statements,
                    span: start.through(end),
                });
            }
            if t.kind == Kind::Newline {
                self.take()?;
                continue;
            }
            if t.is("elif") || t.is("else") {
                return Err(Rule::Syntax.reject(t.span));
            }
            if t.kind == Kind::Indent {
                return Err(Rule::Indentation.reject(t.span));
            }
            if terminal {
                return Err(Rule::Return.reject(t.span));
            }
            if t.is("return") {
                self.take()?;
                let e = self.expr(false, false)?;
                self.line_end()?;
                statements.push(Statement::Return(e));
                terminal = true;
            } else if t.is("if") {
                let statement = self.if_statement()?;
                terminal = matches!(&statement, Statement::If { otherwise: Some(_), .. });
                statements.push(statement);
            } else {
                if t.is("for") {
                    return Err(Rule::Statement.reject(t.span));
                }
                let is_name = matches!(&t.kind, Kind::Word(s) if !keyword(s));
                if is_name {
                    let next = self.peek(1)?;
                    if matches!(&t.kind, Kind::Word(s) if reserved(s)) && !next.is("=") && !next.is(".") {
                        let hint = if t.is("def") || t.is("class") {
                            "write only the body, and inputs are read by their names"
                        } else if t.is("import") || t.is("from") {
                            "builtins need no import, write `floor(x)`, `reduce(…)`"
                        } else {
                            return Err(Rule::Statement.reject(t.span));
                        };
                        return Err(Rule::Statement.reject(t.span).hint(hint));
                    }
                    if next.is(",") || next.is(":") {
                        return Err(Rule::Statement.reject(next.span));
                    }
                    if next.is("=") {
                        let name = self.take()?;
                        self.take()?;
                        let value = self.expr(false, false)?;
                        self.line_end()?;
                        statements.push(Statement::Assign(name, value));
                        continue;
                    }
                }
                let e = self.expr(false, false)?;
                self.line_end()?;
                let next = self.token()?;
                return Err(if matches!(next.kind, Kind::End | Kind::Dedent) {
                    Rule::Return
                } else {
                    Rule::Statement
                }
                .reject(e.span));
            }
        }
    }

    fn line_end(&mut self) -> Result<(), Rejected> {
        let t = self.token()?;
        if t.kind == Kind::Newline {
            self.take()?;
            Ok(())
        } else if t.is("=") {
            Err(Rule::Statement.reject(t.span))
        } else if t.is("for") {
            Err(Rule::Expression.reject(t.span))
        } else if t.is(",") {
            Err(Rule::Expression.reject(t.span).hint(TUPLE_HINT))
        } else if matches!(t.kind, Kind::Text(_)) {
            Err(Rule::Literal.reject(t.span))
        } else {
            Err(self.unexpected(&t))
        }
    }

    fn if_statement(&mut self) -> Result<Statement, Rejected> {
        let start = self.take()?.span;
        let mut branches = Vec::new();
        let mut header = start;
        loop {
            let condition = self.expr(false, false)?;
            if self.is("=")? {
                return Err(Rule::Statement.reject(self.token()?.span));
            }
            if self.is(",")? {
                return Err(Rule::Expression.reject(self.token()?.span).hint(TUPLE_HINT));
            }
            self.want(":")?;
            branches.push((condition, self.branch(header)?));
            if self.is("elif")? {
                header = self.take()?.span;
            } else {
                break;
            }
        }
        let otherwise = if self.eat("else")? {
            let header = self
                .tokens
                .get(self.pos - 1)
                .and_then(|r| r.as_ref().ok())
                .map_or(start, |t| t.span);
            self.want(":")?;
            Some(self.branch(header)?)
        } else {
            None
        };
        let end = otherwise
            .as_ref()
            .map(|b| b.span)
            .or_else(|| branches.last().map(|(_, b)| b.span))
            .unwrap_or(start);
        Ok(Statement::If {
            branches,
            otherwise,
            span: start.through(end),
        })
    }

    fn branch(&mut self, header: Span) -> Result<Block, Rejected> {
        if self.eat("return")? {
            let e = self.expr(false, false)?;
            let span = e.span;
            self.line_end()?;
            return Ok(Block {
                statements: vec![Statement::Return(e)],
                span: header.through(span),
            });
        }
        let token = self.token()?;
        if token.kind != Kind::Newline {
            return Err(Rule::Return.reject(token.span));
        }
        self.take()?;
        if self.token()?.kind != Kind::Indent {
            return Err(Rule::Return.reject(header));
        }
        self.take()?;
        self.enter(header)?;
        let block = self.block(Some(header))?;
        self.nesting -= 1;
        let token = self.token()?;
        if token.kind != Kind::Dedent {
            return Err(self.unexpected(&token));
        }
        self.take()?;
        Ok(block)
    }

    fn expr(&mut self, allow_lambda: bool, allow_generator: bool) -> Result<Expr, Rejected> {
        if self.is("lambda")? {
            return self.lambda(allow_lambda);
        }
        let value = self.disjunction(allow_generator)?;
        if self.eat("if")? {
            self.conditional(value)
        } else {
            Ok(value)
        }
    }

    fn lambda(&mut self, allowed: bool) -> Result<Expr, Rejected> {
        let start = self.take()?.span;
        if !allowed {
            return Err(Rule::Expression.reject(start));
        }
        self.enter(start)?;
        let mut params = vec![self.name()?];
        while self.eat(",")? {
            params.push(self.name()?);
        }
        self.want(":")?;
        let body = self.expr(false, false)?;
        self.nesting -= 1;
        let span = start.through(body.span);
        Ok(Expr::new(ExprKind::Lambda(params, Box::new(body)), span))
    }

    fn conditional(&mut self, value: Expr) -> Result<Expr, Rejected> {
        self.enter(value.span)?;
        let cond = self.disjunction(false)?;
        self.want("else")?;
        let other = self.expr(false, false)?;
        self.nesting -= 1;
        let span = value.span.through(other.span);
        Ok(Expr::new(
            ExprKind::Conditional(Box::new(value), Box::new(cond), Box::new(other)),
            span,
        ))
    }

    fn disjunction(&mut self, generator: bool) -> Result<Expr, Rejected> {
        // Operator stacks keep precedence and chains flat without adding call frames per precedence level.
        let mut values = Vec::new();
        let mut operators: Vec<(String, u8, Span)> = Vec::new();
        let mut allow_not = true;
        let mut tuple = false;
        loop {
            while allow_not && self.is("not")? {
                let token = self.take()?;
                self.enter(token.span)?;
                operators.push(("not".to_owned(), 3, token.span));
            }
            values.push(self.primary(generator && values.is_empty() && operators.is_empty(), tuple)?);
            if matches!(self.token()?.kind, Kind::Text(_)) {
                return Err(Rule::Literal.reject(self.token()?.span));
            }
            let Some((operator, precedence)) = self.operator()? else {
                break;
            };
            while operators.last().is_some_and(|(_, p, _)| *p >= precedence) {
                self.reduce_operator(&mut values, &mut operators)?;
            }
            allow_not = precedence <= 2;
            tuple = operator == "in" || operator == "not in";
            operators.push((operator, precedence, Span::default()));
        }
        while !operators.is_empty() {
            self.reduce_operator(&mut values, &mut operators)?;
        }
        values.pop().ok_or_else(|| {
            Rule::Syntax.reject(Span {
                start: self.end,
                end: self.end,
            })
        })
    }

    fn operator_ahead(&mut self) -> Result<bool, Rejected> {
        let t = self.token()?;
        Ok([
            "or", "and", "==", "!=", "<", "<=", ">", ">=", "in", "not", "is", "+", "-", "*", "/",
        ]
        .iter()
        .any(|s| t.is(s)))
    }

    fn operator(&mut self) -> Result<Option<(String, u8)>, Rejected> {
        let token = self.token()?;
        let text = match &token.kind {
            Kind::Word(s) | Kind::Symbol(s) => s.as_str(),
            _ => return Ok(None),
        };
        let precedence = match text {
            "or" => 1,
            "and" => 2,
            "==" | "!=" | "<" | "<=" | ">" | ">=" | "in" | "not" | "is" => 4,
            "+" | "-" => 5,
            "*" | "/" => 6,
            _ => return Ok(None),
        };
        let mut op = text.to_owned();
        self.take()?;
        if op == "not" {
            self.want("in")?;
            op = "not in".to_owned();
        }
        if op == "is" {
            if self.eat("not")? {
                op = "is not".to_owned();
            }
            if !self.is("None")? {
                return Err(Rule::Operator.reject(token.span));
            }
        }
        Ok(Some((op, precedence)))
    }

    fn reduce_operator(
        &mut self,
        values: &mut Vec<Expr>,
        operators: &mut Vec<(String, u8, Span)>,
    ) -> Result<(), Rejected> {
        let Some((op, precedence, start)) = operators.pop() else {
            return Err(Rule::Syntax.reject(Span::default()));
        };
        let right = values.pop().ok_or_else(|| Rule::Syntax.reject(Span::default()))?;
        if op == "not" {
            self.nesting -= 1;
            let span = start.through(right.span);
            values.push(Expr::new(ExprKind::Unary(op, Box::new(right)), span));
        } else {
            let mut left = values.pop().ok_or_else(|| Rule::Syntax.reject(right.span))?;
            if matches!(left.kind.as_ref(), ExprKind::Tuple(_))
                || (matches!(right.kind.as_ref(), ExprKind::Tuple(_)) && op != "in" && op != "not in")
            {
                return Err(Rule::Expression
                    .reject(if matches!(left.kind.as_ref(), ExprKind::Tuple(_)) {
                        left.span
                    } else {
                        right.span
                    })
                    .hint(TUPLE_HINT));
            }
            let span = left.span.through(right.span);
            if !left.grouped {
                let existing = match left.kind.as_mut() {
                    ExprKind::Compare(_, rest) if precedence == 4 => Some(rest),
                    ExprKind::Chain(_, rest)
                        if rest
                            .first()
                            .is_some_and(|(previous, _)| op_precedence(previous) == precedence) =>
                    {
                        Some(rest)
                    }
                    _ => None,
                };
                if let Some(rest) = existing {
                    rest.push((op, right));
                    left.span = span;
                    values.push(left);
                    return Ok(());
                }
            }
            let kind = if precedence == 4 {
                ExprKind::Compare(Box::new(left), vec![(op, right)])
            } else {
                ExprKind::Chain(Box::new(left), vec![(op, right)])
            };
            values.push(Expr::new(kind, span));
        }
        Ok(())
    }

    fn primary(&mut self, generator: bool, tuple: bool) -> Result<Expr, Rejected> {
        let token = self.token()?;
        if token.is("+") {
            return Err(Rule::Operator.reject(token.span));
        }
        if token.is("-") {
            return self.negative(generator);
        }
        let value = self.atom(generator, tuple)?;
        self.postfix(value, tuple)
    }

    fn negative(&mut self, _generator: bool) -> Result<Expr, Rejected> {
        let token = self.take()?;
        if let Kind::Number(number) = self.token()?.kind {
            let end = self.take()?.span;
            return Ok(Expr::new(
                ExprKind::Number(format!("-{number}")),
                token.span.through(end),
            ));
        }
        self.enter(token.span)?;
        let value = self.primary(false, false)?;
        self.nesting -= 1;
        let span = token.span.through(value.span);
        Ok(Expr::new(ExprKind::Unary("-".to_owned(), Box::new(value)), span))
    }

    fn postfix(&mut self, mut value: Expr, tuple: bool) -> Result<Expr, Rejected> {
        while self.eat(".")? {
            let field = self.take()?;
            let Kind::Word(name) = field.kind else {
                return Err(self.unexpected(&field));
            };
            if matches!(value.kind.as_ref(), ExprKind::Tuple(_)) {
                return Err(Rule::Expression.reject(value.span).hint(TUPLE_HINT));
            }
            let span = value.span.through(field.span);
            if let ExprKind::Field(_, fields) = value.kind.as_mut() {
                fields.push((name, field.span));
                value.span = span;
            } else {
                value = Expr::new(ExprKind::Field(Box::new(value), vec![(name, field.span)]), span);
            }
        }
        if self.is("[")? || self.is("(")? {
            let mut rejected = Rule::Expression.reject(self.token()?.span);
            if matches!(value.kind.as_ref(), ExprKind::Field(base, _) if matches!(base.kind.as_ref(), ExprKind::Name(s) if s == "math" || s == "functools"))
            {
                rejected = rejected.hint("builtins need no import, write `floor(x)`, `reduce(…)`");
            }
            return Err(rejected);
        }
        if matches!(value.kind.as_ref(), ExprKind::Tuple(_)) && !tuple {
            return Err(Rule::Expression.reject(value.span).hint(TUPLE_HINT));
        }
        Ok(value)
    }

    fn atom(&mut self, generator: bool, tuple: bool) -> Result<Expr, Rejected> {
        let t = self.take()?;
        let kind = match &t.kind {
            Kind::Number(s) => ExprKind::Number(s.clone()),
            Kind::Text(s) => ExprKind::Text(s.clone()),
            Kind::Word(s) if s == "True" || s == "False" => ExprKind::Boolean(s == "True"),
            Kind::Word(s) if s == "None" => ExprKind::Nothing,
            Kind::Word(s) if !keyword(s) => {
                if self.is("(")? {
                    return self.call(s.clone(), t.span);
                }
                ExprKind::Name(s.clone())
            }
            Kind::Symbol(s) if s == "(" || s == "[" => return self.bracket(t, generator, tuple),
            Kind::Word(s) if s == "lambda" => return Err(Rule::Expression.reject(t.span)),
            _ => return Err(self.unexpected(&t)),
        };
        Ok(Expr::new(kind, t.span))
    }

    fn bracket(&mut self, open: Token, generator: bool, tuple_allowed: bool) -> Result<Expr, Rejected> {
        self.enter(open.span)?;
        let list = open.is("[");
        let close = if list { "]" } else { ")" };
        let mut items = Vec::new();
        let mut comma = false;
        if !self.is(close)? {
            let first = self.expr(false, generator && !list)?;
            if self.is("for")? {
                if !list && !generator {
                    return Err(Rule::Expression.reject(self.token()?.span));
                }
                let for_span = self.token()?.span;
                let mut expr = self.generator(first, !list)?;
                let end = self.want(close)?.span;
                expr.span = open.span.through(end);
                self.nesting -= 1;
                if list {
                    let span = expr.span;
                    return Ok(Expr::new(ExprKind::Comprehension(Box::new(expr)), span));
                }
                if self.is("if")? || self.is(".")? || self.is("[")? || self.is("(")? || self.operator_ahead()? {
                    return Err(Rule::Expression.reject(for_span));
                }
                return Ok(expr);
            }
            items.push(first);
            while self.eat(",")? {
                if !list && !tuple_allowed {
                    return Err(Rule::Expression.reject(open.span).hint(TUPLE_HINT));
                }
                comma = true;
                if self.is(close)? {
                    break;
                }
                items.push(self.expr(false, false)?);
            }
        }
        let end = self.want(close)?.span;
        self.nesting -= 1;
        let span = open.span.through(end);
        if list {
            Ok(Expr::new(ExprKind::List(items), span))
        } else if comma || items.is_empty() {
            if !tuple_allowed {
                return Err(Rule::Expression.reject(open.span).hint(TUPLE_HINT));
            }
            Ok(Expr::new(ExprKind::Tuple(items), span))
        } else {
            let mut value = items.pop().ok_or_else(|| Rule::Syntax.reject(span))?;
            value.span = span;
            value.grouped = true;
            Ok(value)
        }
    }

    fn generator(&mut self, item: Expr, parenthesized: bool) -> Result<Expr, Rejected> {
        self.want("for")?;
        let var = self.name()?;
        if self.is(",")? {
            return Err(Rule::Expression.reject(self.token()?.span).hint(TUPLE_HINT));
        }
        self.want("in")?;
        let list = self.disjunction(false)?;
        let predicate = if self.eat("if")? {
            Some(Box::new(self.disjunction(false)?))
        } else {
            None
        };
        if self.is("for")? || self.is("if")? {
            return Err(Rule::Expression.reject(self.token()?.span));
        }
        let span = item.span.through(predicate.as_ref().map_or(list.span, |p| p.span));
        Ok(Expr::new(
            ExprKind::Generator {
                item: Box::new(item),
                var,
                list: Box::new(list),
                predicate,
                parenthesized,
            },
            span,
        ))
    }

    fn call(&mut self, name: String, start: Span) -> Result<Expr, Rejected> {
        self.want("(")?;
        self.enter(start)?;
        let mut args = Vec::new();
        if !self.is(")")? {
            loop {
                let t = self.token()?;
                if t.is("*") || t.is("**") {
                    return Err(Rule::Expression.reject(t.span));
                }
                let arg_name = if matches!(t.kind, Kind::Word(_)) && self.peek(1)?.is("=") {
                    let token = self.take()?;
                    self.take()?;
                    Some(token)
                } else {
                    None
                };
                let lambda = (name == "sorted" && arg_name.as_ref().is_some_and(|t| t.is("key")))
                    || (name == "reduce" && args.is_empty() && arg_name.is_none());
                let generators =
                    matches!(name.as_str(), "sum" | "all" | "any" | "next") && args.is_empty() && arg_name.is_none();
                let mut value = self.expr(lambda, generators)?;
                if self.is("for")? {
                    if !matches!(name.as_str(), "sum" | "all" | "any" | "next")
                        || arg_name.is_some()
                        || !args.is_empty()
                    {
                        return Err(Rule::Expression.reject(self.token()?.span));
                    }
                    value = self.generator(value, false)?;
                    args.push(Argument { name: None, value });
                    if name == "next" && self.eat(",")? && !self.is(")")? {
                        continue;
                    }
                    break;
                }
                args.push(Argument { name: arg_name, value });
                if !self.eat(",")? || self.is(")")? {
                    break;
                }
            }
        }
        let end = self.want(")")?.span;
        self.nesting -= 1;
        let span = start.through(end);
        check_shape(&name, &args, span)?;
        Ok(Expr::new(ExprKind::Call(name, args), span))
    }
}

fn check_shape(name: &str, args: &[Argument], span: Span) -> Result<(), Rejected> {
    let positional = args.iter().all(|a| a.name.is_none());
    let count = args.len();
    let at = |i: usize| args.get(i).map(|a| a.value.kind.as_ref());
    let hint = match name {
        "len" if !positional || count != 1 => "len(x)",
        "unwrap_or" if !positional || count != 2 => "unwrap_or(x, default)",
        "all" | "any" if !positional || count != 1 => "all(c for x in xs)",
        "sum"
            if args
                .iter()
                .any(|a| matches!(a.value.kind.as_ref(), ExprKind::Generator { .. }))
                && (!positional || count != 1) =>
        {
            "sum(e for x in xs)"
        }
        "next" => {
            let valid = matches!(at(0), Some(ExprKind::Generator { item, var, predicate: Some(_), parenthesized: true, .. }) if matches!(item.kind.as_ref(), ExprKind::Name(s) if var.is(s)));
            if positional && count == 2 && valid {
                return Ok(());
            }
            "next((x for x in xs if c), None)"
        }
        "reduce" => {
            if positional && count == 3 && matches!(at(0), Some(ExprKind::Lambda(p, _)) if p.len() == 2) {
                return Ok(());
            }
            "reduce(lambda acc, x: e, xs, init)"
        }
        "sorted" => {
            let mut key = false;
            let mut reverse = false;
            let valid = args.first().is_some_and(|a| a.name.is_none())
                && args.iter().skip(1).all(|a| {
                    if a.name.as_ref().is_some_and(|t| t.is("key")) && !key {
                        key = true;
                        matches!(a.value.kind.as_ref(), ExprKind::Lambda(p, _) if p.len() == 1)
                    } else if a.name.as_ref().is_some_and(|t| t.is("reverse")) && !reverse {
                        reverse = true;
                        matches!(a.value.kind.as_ref(), ExprKind::Boolean(_))
                    } else {
                        false
                    }
                });
            if valid {
                return Ok(());
            }
            "sorted(xs, key=lambda x: k, reverse=True)"
        }
        _ => return Ok(()),
    };
    Err(Rule::Shape.reject(span).hint(hint))
}

fn op_precedence(op: &str) -> u8 {
    match op {
        "or" => 1,
        "and" => 2,
        "+" | "-" => 5,
        _ => 6,
    }
}
