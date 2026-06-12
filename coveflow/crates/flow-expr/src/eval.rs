//! Evaluator for the restricted expression language.

use serde_json::Value;

use crate::ExprError;
use crate::parser::{Ast, BinOp, MAX_DEPTH, UnOp, number_value};

/// Evaluate a parsed expression against the JSON context root `ctx`.
///
/// Path navigation that doesn't resolve yields `Value::Null`; operators applied
/// to incompatible types return [`ExprError::Type`].
pub fn eval(ast: &Ast, ctx: &Value) -> Result<Value, ExprError> {
    eval_depth(ast, ctx, 0)
}

fn eval_depth(ast: &Ast, ctx: &Value, depth: usize) -> Result<Value, ExprError> {
    // Bound recursion so a deeply-nested AST cannot overflow the stack.
    if depth > MAX_DEPTH {
        return Err(ExprError::Parse("expression too deeply nested".into()));
    }
    let eval = |a: &Ast, c: &Value| eval_depth(a, c, depth + 1);
    match ast {
        Ast::Lit(v) => Ok(v.clone()),
        Ast::Var(name) => Ok(ctx.get(name).cloned().unwrap_or(Value::Null)),
        Ast::Field(base, field) => {
            let b = eval(base, ctx)?;
            Ok(b.get(field).cloned().unwrap_or(Value::Null))
        }
        Ast::Index(base, idx) => {
            let b = eval(base, ctx)?;
            let i = eval(idx, ctx)?;
            Ok(index_value(&b, &i))
        }
        Ast::Unary(op, inner) => {
            let v = eval(inner, ctx)?;
            match op {
                UnOp::Not => Ok(Value::Bool(!truthy(&v))),
                UnOp::Neg => match as_f64(&v) {
                    Some(n) => Ok(number_value(-n)),
                    None => Err(ExprError::Type("unary '-' expects a number".into())),
                },
            }
        }
        Ast::Binary(op, l, r) => eval_binary(op, l, r, ctx, depth),
        Ast::Ternary(cond, then, els) => {
            if truthy(&eval(cond, ctx)?) {
                eval(then, ctx)
            } else {
                eval(els, ctx)
            }
        }
    }
}

fn eval_binary(
    op: &BinOp,
    l: &Ast,
    r: &Ast,
    ctx: &Value,
    depth: usize,
) -> Result<Value, ExprError> {
    let eval = |a: &Ast, c: &Value| eval_depth(a, c, depth + 1);
    // Short-circuit logical operators before evaluating the right side.
    match op {
        BinOp::And => {
            let lv = eval(l, ctx)?;
            if !truthy(&lv) {
                return Ok(Value::Bool(false));
            }
            return Ok(Value::Bool(truthy(&eval(r, ctx)?)));
        }
        BinOp::Or => {
            let lv = eval(l, ctx)?;
            if truthy(&lv) {
                return Ok(Value::Bool(true));
            }
            return Ok(Value::Bool(truthy(&eval(r, ctx)?)));
        }
        _ => {}
    }

    let lv = eval(l, ctx)?;
    let rv = eval(r, ctx)?;

    match op {
        BinOp::Eq => Ok(Value::Bool(lv == rv)),
        BinOp::Ne => Ok(Value::Bool(lv != rv)),
        BinOp::Add => add(&lv, &rv),
        BinOp::Sub => arith(&lv, &rv, |a, b| a - b),
        BinOp::Mul => arith(&lv, &rv, |a, b| a * b),
        BinOp::Div => {
            let (a, b) = nums(&lv, &rv, "/")?;
            if b == 0.0 {
                Err(ExprError::DivByZero)
            } else {
                Ok(number_value(a / b))
            }
        }
        BinOp::Lt => compare(&lv, &rv, |o| o.is_lt()),
        BinOp::Le => compare(&lv, &rv, |o| o.is_le()),
        BinOp::Gt => compare(&lv, &rv, |o| o.is_gt()),
        BinOp::Ge => compare(&lv, &rv, |o| o.is_ge()),
        BinOp::And | BinOp::Or => unreachable!("handled above"),
    }
}

fn add(l: &Value, r: &Value) -> Result<Value, ExprError> {
    match (l, r) {
        (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{a}{b}"))),
        _ => arith(l, r, |a, b| a + b),
    }
}

fn arith(l: &Value, r: &Value, f: impl Fn(f64, f64) -> f64) -> Result<Value, ExprError> {
    let (a, b) = nums(l, r, "arithmetic")?;
    Ok(number_value(f(a, b)))
}

fn nums(l: &Value, r: &Value, what: &str) -> Result<(f64, f64), ExprError> {
    match (as_f64(l), as_f64(r)) {
        (Some(a), Some(b)) => Ok((a, b)),
        _ => Err(ExprError::Type(format!("{what} expects numbers"))),
    }
}

fn compare(
    l: &Value,
    r: &Value,
    f: impl Fn(std::cmp::Ordering) -> bool,
) -> Result<Value, ExprError> {
    let ord = match (l, r) {
        (Value::Number(_), Value::Number(_)) => {
            let (a, b) = nums(l, r, "comparison")?;
            a.partial_cmp(&b)
                .ok_or_else(|| ExprError::Type("cannot compare NaN".into()))?
        }
        (Value::String(a), Value::String(b)) => a.cmp(b),
        _ => {
            return Err(ExprError::Type(
                "comparison expects two numbers or two strings".into(),
            ));
        }
    };
    Ok(Value::Bool(f(ord)))
}

fn index_value(base: &Value, idx: &Value) -> Value {
    match (base, idx) {
        (Value::Array(arr), Value::Number(n)) => n
            .as_u64()
            .and_then(|u| arr.get(u as usize))
            .cloned()
            .unwrap_or(Value::Null),
        (Value::Object(map), Value::String(k)) => map.get(k).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// Truthiness: bool as-is; null false; numbers nonzero; non-empty string/array/object.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
