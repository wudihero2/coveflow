//! Restricted expression language for flow input bindings.
//!
//! Evaluates expressions like `steps.a.result.total + 1` or
//! `flow.input.enabled && steps.b.result.count > 0` against a JSON context
//! root. This is **not** a programming language: the grammar is closed to
//! literals, path navigation, arithmetic/comparison/logical operators and a
//! ternary. There are no function calls, assignment, loops, or IO, so it cannot
//! execute arbitrary code — safe to run in the engine process (outside the
//! sandbox), unlike an embedded scripting engine.
//!
//! The context root is a JSON object the caller builds, e.g.
//! `{ "flow": { "input": {...} }, "steps": { "a": { "result": {...} } },
//!    "step": { "iter": ..., "index": 0 } }`.
//! Unknown paths evaluate to `null`; type mismatches in operators are errors.

use serde_json::Value;

mod eval;
mod lexer;
mod parser;

pub use eval::eval;
pub use parser::{Ast, parse};

/// An expression error (parse or evaluation).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExprError {
    /// Lexing/parsing failure with a human-readable reason.
    #[error("expression parse error: {0}")]
    Parse(String),
    /// Operator applied to incompatible types.
    #[error("expression type error: {0}")]
    Type(String),
    /// Division by zero.
    #[error("expression division by zero")]
    DivByZero,
}

/// Parse and evaluate `src` against the JSON context `ctx` in one call.
pub fn eval_str(src: &str, ctx: &Value) -> Result<Value, ExprError> {
    let ast = parse(src)?;
    eval(&ast, ctx)
}

/// Validate that `src` parses (used by FlowSpec save-time checks). Does not eval.
pub fn check(src: &str) -> Result<(), ExprError> {
    parse(src).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Value {
        json!({
            "flow": { "input": { "enabled": true, "name": "etl" } },
            "steps": {
                "a": { "result": { "total": 41, "rows": [10, 20, 30] } },
                "b": { "result": { "ok": false } }
            },
            "step": { "iter": { "amount": 5 }, "index": 2 }
        })
    }

    #[test]
    fn navigation_and_arithmetic() {
        assert_eq!(
            eval_str("steps.a.result.total + 1", &ctx()).unwrap(),
            json!(42)
        );
        assert_eq!(
            eval_str("steps.a.result.rows[1]", &ctx()).unwrap(),
            json!(20)
        );
        assert_eq!(eval_str("flow.input.name", &ctx()).unwrap(), json!("etl"));
        assert_eq!(eval_str("step.iter.amount * 2", &ctx()).unwrap(), json!(10));
        assert_eq!(eval_str("step.index", &ctx()).unwrap(), json!(2));
    }

    #[test]
    fn logical_comparison_ternary() {
        assert_eq!(
            eval_str("flow.input.enabled && steps.a.result.total > 40", &ctx()).unwrap(),
            json!(true)
        );
        assert_eq!(eval_str("steps.b.result.ok", &ctx()).unwrap(), json!(false));
        assert_eq!(eval_str("!steps.b.result.ok", &ctx()).unwrap(), json!(true));
        assert_eq!(
            eval_str("steps.b.result.ok ? \"yes\" : \"no\"", &ctx()).unwrap(),
            json!("no")
        );
    }

    #[test]
    fn string_concat_and_equality() {
        assert_eq!(
            eval_str("flow.input.name + \"-job\"", &ctx()).unwrap(),
            json!("etl-job")
        );
        assert_eq!(
            eval_str("flow.input.name == \"etl\"", &ctx()).unwrap(),
            json!(true)
        );
        assert_eq!(
            eval_str("steps.a.result.total != 0", &ctx()).unwrap(),
            json!(true)
        );
    }

    #[test]
    fn unknown_path_is_null() {
        assert_eq!(eval_str("steps.zzz.result.x", &ctx()).unwrap(), Value::Null);
        assert_eq!(eval_str("flow.input.nope", &ctx()).unwrap(), Value::Null);
        assert_eq!(
            eval_str("steps.a.result.rows[99]", &ctx()).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn type_mismatch_is_error() {
        // null (unknown) + number → type error, not a silent 0.
        assert!(matches!(
            eval_str("flow.input.nope + 1", &ctx()),
            Err(ExprError::Type(_))
        ));
        assert_eq!(eval_str("1 / 0", &ctx()), Err(ExprError::DivByZero));
    }

    #[test]
    fn precedence_and_parens() {
        assert_eq!(eval_str("1 + 2 * 3", &ctx()).unwrap(), json!(7));
        assert_eq!(eval_str("(1 + 2) * 3", &ctx()).unwrap(), json!(9));
        assert_eq!(eval_str("-5 + 10", &ctx()).unwrap(), json!(5));
    }

    #[test]
    fn rejects_pathologically_deep_expressions() {
        // Deeply nested parens / unary / member chains must be rejected at parse
        // so a malicious flow definition cannot overflow the shared process stack.
        let deep = format!("{}1{}", "(".repeat(500), ")".repeat(500));
        assert!(matches!(check(&deep), Err(ExprError::Parse(_))));
        let unary = format!("{}1", "!".repeat(500));
        assert!(matches!(check(&unary), Err(ExprError::Parse(_))));
        let chain = format!("a{}", ".b".repeat(500));
        assert!(matches!(check(&chain), Err(ExprError::Parse(_))));
        // A reasonable expression still parses + evaluates.
        assert!(check("steps.a.result.rows[0] + 1").is_ok());
    }

    #[test]
    fn rejects_non_expression_syntax() {
        // No function calls, assignment, statements — these must fail to parse,
        // proving arbitrary code cannot be smuggled in.
        assert!(matches!(check("foo()"), Err(ExprError::Parse(_))));
        assert!(matches!(check("x = 1"), Err(ExprError::Parse(_))));
        assert!(matches!(check("a; b"), Err(ExprError::Parse(_))));
        assert!(matches!(
            check("steps.a.result.total +"),
            Err(ExprError::Parse(_))
        ));
    }
}
