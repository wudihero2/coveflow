//! Recursive-descent parser for the restricted expression language.

use serde_json::Value;

use crate::ExprError;
use crate::lexer::{Token, tokenize};

#[derive(Debug, Clone, PartialEq)]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// Parsed expression. Opaque to callers; evaluate with [`crate::eval`].
#[derive(Debug, Clone, PartialEq)]
pub enum Ast {
    Lit(Value),
    /// A root identifier (`flow`, `steps`, `step`, ...).
    Var(String),
    Field(Box<Ast>, String),
    Index(Box<Ast>, Box<Ast>),
    Unary(UnOp, Box<Ast>),
    Binary(BinOp, Box<Ast>, Box<Ast>),
    Ternary(Box<Ast>, Box<Ast>, Box<Ast>),
}

/// Max nesting depth (parens / unary / member-access chains). Bounds recursion
/// in both the parser and the evaluator so a pathological expression cannot
/// overflow the (shared, non-sandboxed) process stack. Kept well below the
/// point of overflow: the recursive-descent chain is ~11 frames per nesting
/// level, and worker/test threads run on ~2 MB stacks, so 64 levels (~700
/// frames) is safe while being far deeper than any legitimate expression.
pub(crate) const MAX_DEPTH: usize = 64;

/// Parse an expression string into an [`Ast`].
pub fn parse(src: &str) -> Result<Ast, ExprError> {
    let tokens = tokenize(src)?;
    let mut p = Parser {
        tokens,
        pos: 0,
        depth: 0,
    };
    let ast = p.expr()?;
    if p.pos != p.tokens.len() {
        return Err(ExprError::Parse(format!(
            "unexpected trailing tokens at position {}",
            p.pos
        )));
    }
    Ok(ast)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Current recursion depth (guarded against MAX_DEPTH).
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, want: &Token) -> Result<(), ExprError> {
        if self.peek() == Some(want) {
            self.pos += 1;
            Ok(())
        } else {
            Err(ExprError::Parse(format!("expected {want:?}")))
        }
    }

    // expr := ternary
    fn expr(&mut self) -> Result<Ast, ExprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ExprError::Parse("expression too deeply nested".into()));
        }
        let r = self.ternary()?;
        self.depth -= 1;
        Ok(r)
    }

    // ternary := or ('?' expr ':' expr)?
    fn ternary(&mut self) -> Result<Ast, ExprError> {
        let cond = self.or()?;
        if self.peek() == Some(&Token::Question) {
            self.pos += 1;
            let then = self.expr()?;
            self.eat(&Token::Colon)?;
            let els = self.expr()?;
            return Ok(Ast::Ternary(Box::new(cond), Box::new(then), Box::new(els)));
        }
        Ok(cond)
    }

    fn or(&mut self) -> Result<Ast, ExprError> {
        let mut left = self.and()?;
        while self.peek() == Some(&Token::OrOr) {
            self.pos += 1;
            let right = self.and()?;
            left = Ast::Binary(BinOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Ast, ExprError> {
        let mut left = self.equality()?;
        while self.peek() == Some(&Token::AndAnd) {
            self.pos += 1;
            let right = self.equality()?;
            left = Ast::Binary(BinOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn equality(&mut self) -> Result<Ast, ExprError> {
        let mut left = self.comparison()?;
        loop {
            let op = match self.peek() {
                Some(Token::EqEq) => BinOp::Eq,
                Some(Token::NotEq) => BinOp::Ne,
                _ => break,
            };
            self.pos += 1;
            let right = self.comparison()?;
            left = Ast::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn comparison(&mut self) -> Result<Ast, ExprError> {
        let mut left = self.additive()?;
        loop {
            let op = match self.peek() {
                Some(Token::Lt) => BinOp::Lt,
                Some(Token::Le) => BinOp::Le,
                Some(Token::Gt) => BinOp::Gt,
                Some(Token::Ge) => BinOp::Ge,
                _ => break,
            };
            self.pos += 1;
            let right = self.additive()?;
            left = Ast::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn additive(&mut self) -> Result<Ast, ExprError> {
        let mut left = self.multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let right = self.multiplicative()?;
            left = Ast::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn multiplicative(&mut self) -> Result<Ast, ExprError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                _ => break,
            };
            self.pos += 1;
            let right = self.unary()?;
            left = Ast::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Ast, ExprError> {
        let op = match self.peek() {
            Some(Token::Bang) => UnOp::Not,
            Some(Token::Minus) => UnOp::Neg,
            _ => return self.postfix(),
        };
        self.pos += 1;
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ExprError::Parse("expression too deeply nested".into()));
        }
        let inner = self.unary()?;
        self.depth -= 1;
        Ok(Ast::Unary(op, Box::new(inner)))
    }

    // postfix := primary ('.' ident | '[' expr ']')*
    fn postfix(&mut self) -> Result<Ast, ExprError> {
        let mut base = self.primary()?;
        let mut chain = 0usize;
        loop {
            chain += 1;
            if chain > MAX_DEPTH {
                return Err(ExprError::Parse("member-access chain too long".into()));
            }
            match self.peek() {
                Some(Token::Dot) => {
                    self.pos += 1;
                    match self.bump() {
                        Some(Token::Ident(name)) => {
                            base = Ast::Field(Box::new(base), name);
                        }
                        other => {
                            return Err(ExprError::Parse(format!(
                                "expected field name after '.', got {other:?}"
                            )));
                        }
                    }
                }
                Some(Token::LBracket) => {
                    self.pos += 1;
                    let idx = self.expr()?;
                    self.eat(&Token::RBracket)?;
                    base = Ast::Index(Box::new(base), Box::new(idx));
                }
                _ => break,
            }
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<Ast, ExprError> {
        match self.bump() {
            Some(Token::Number(n)) => Ok(Ast::Lit(number_value(n))),
            Some(Token::Str(s)) => Ok(Ast::Lit(Value::String(s))),
            Some(Token::True) => Ok(Ast::Lit(Value::Bool(true))),
            Some(Token::False) => Ok(Ast::Lit(Value::Bool(false))),
            Some(Token::Null) => Ok(Ast::Lit(Value::Null)),
            Some(Token::Ident(name)) => Ok(Ast::Var(name)),
            Some(Token::LParen) => {
                let inner = self.expr()?;
                self.eat(&Token::RParen)?;
                Ok(inner)
            }
            other => Err(ExprError::Parse(format!("expected a value, got {other:?}"))),
        }
    }
}

/// Build a JSON number, preferring an integer representation when the value is
/// integral so `41 + 1` serializes as `42` rather than `42.0`.
pub(crate) fn number_value(n: f64) -> Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < i64::MAX as f64 {
        Value::Number((n as i64).into())
    } else {
        serde_json::Number::from_f64(n)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}
