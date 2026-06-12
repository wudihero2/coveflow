//! Extract a script's `main()` signature into a JSON Schema so the flow editor
//! can auto-detect and prefill node input bindings.
//!
//! The worker invokes scripts as `mod.main(**args)` (all keyword), so we only
//! care about parameters that can be passed by name. `*args` is ignored (it can
//! never receive a keyword argument); `**kwargs` becomes `additionalProperties`.
//!
//! Type/default extraction is intentionally shallow and best-effort: anything we
//! cannot map cleanly is simply omitted rather than guessed.

use rustpython_parser::{Mode, ast, parse};
use serde_json::{Map, Value, json};

/// Result of inspecting a Python source for its `main()` entrypoint.
pub enum MainSchema {
    /// Parsed successfully and found a top-level `main()`; carries its JSON Schema.
    Found(Value),
    /// Parsed successfully but there is no top-level `main()` function.
    NoMain,
    /// The source could not be parsed by our parser (do not treat as an error).
    Unparseable,
}

/// Inspect Python source and derive the `main()` input schema.
pub fn extract_main_schema(source: &str) -> MainSchema {
    let module = match parse(source, Mode::Module, "<script>") {
        Ok(m) => m,
        Err(_) => return MainSchema::Unparseable,
    };
    let body = match module {
        ast::Mod::Module(m) => m.body,
        _ => return MainSchema::Unparseable,
    };
    match find_main_args(&body) {
        Some(args) => MainSchema::Found(build_schema(args)),
        None => MainSchema::NoMain,
    }
}

/// Find the arguments of a top-level `def main` / `async def main`.
fn find_main_args(body: &[ast::Stmt]) -> Option<&ast::Arguments> {
    for stmt in body {
        match stmt {
            ast::Stmt::FunctionDef(f) if f.name.as_str() == "main" => return Some(&f.args),
            ast::Stmt::AsyncFunctionDef(f) if f.name.as_str() == "main" => return Some(&f.args),
            _ => {}
        }
    }
    None
}

/// Parameters the worker injects itself (signature-aware), so they are NOT flow
/// inputs — the editor must not surface them for binding. `ctx` is the execution
/// context auto-injected by the Python wrapper when `main()` declares it.
const RESERVED_PARAMS: &[&str] = &["ctx"];

fn build_schema(args: &ast::Arguments) -> Value {
    let mut properties = Map::new();
    let mut required: Vec<String> = Vec::new();
    let mut order: Vec<String> = Vec::new();

    // Positional-only + ordinary positional, then keyword-only. All are
    // passable by keyword via main(**args) except positional-only, which we
    // still list (rare) since omitting them would hide a real parameter.
    let positional = args.posonlyargs.iter().chain(args.args.iter());
    for awd in positional.chain(args.kwonlyargs.iter()) {
        // Skip worker-injected params (e.g. `ctx`): they are auto-supplied at run
        // time, never bound from the flow.
        if RESERVED_PARAMS.contains(&awd.def.arg.as_str()) {
            continue;
        }
        add_param(awd, &mut properties, &mut required, &mut order);
    }

    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    schema.insert(
        "required".into(),
        Value::Array(required.into_iter().map(Value::String).collect()),
    );
    schema.insert(
        "order".into(),
        Value::Array(order.into_iter().map(Value::String).collect()),
    );
    // **kwargs => the function accepts arbitrarily-named inputs.
    if args.kwarg.is_some() {
        schema.insert("additionalProperties".into(), json!(true));
    }
    Value::Object(schema)
}

fn add_param(
    awd: &ast::ArgWithDefault,
    properties: &mut Map<String, Value>,
    required: &mut Vec<String>,
    order: &mut Vec<String>,
) {
    let name = awd.def.arg.as_str().to_string();
    let mut prop = Map::new();

    if let Some(ann) = &awd.def.annotation {
        if let Some(t) = type_of_annotation(ann) {
            prop.insert("type".into(), json!(t));
        }
    }

    if let Some(default) = &awd.default {
        if let Some(v) = const_to_json(default) {
            // Infer type from a literal default when there was no annotation.
            if !prop.contains_key("type") {
                if let Some(t) = json_type_name(&v) {
                    prop.insert("type".into(), json!(t));
                }
            }
            prop.insert("default".into(), v);
        }
    }

    properties.insert(name.clone(), Value::Object(prop));
    order.push(name.clone());
    if awd.default.is_none() {
        required.push(name);
    }
}

/// Map the outermost name of a type annotation to a JSON Schema type.
fn type_of_annotation(expr: &ast::Expr) -> Option<&'static str> {
    match expr {
        ast::Expr::Name(n) => map_type_name(n.id.as_str()),
        ast::Expr::Subscript(s) => {
            if let ast::Expr::Name(base) = s.value.as_ref() {
                if base.id.as_str() == "Optional" {
                    return type_of_annotation(&s.slice);
                }
                return map_type_name(base.id.as_str());
            }
            None
        }
        // `X | None` (and unions generally): take the first side that maps.
        ast::Expr::BinOp(b) => type_of_annotation(&b.left).or_else(|| type_of_annotation(&b.right)),
        _ => None,
    }
}

fn map_type_name(name: &str) -> Option<&'static str> {
    match name {
        "int" => Some("integer"),
        "float" => Some("number"),
        "str" => Some("string"),
        "bool" => Some("boolean"),
        "list" | "List" | "tuple" | "Tuple" | "set" | "Set" => Some("array"),
        "dict" | "Dict" => Some("object"),
        _ => None,
    }
}

/// Convert a literal-constant default expression to JSON, or None if it is not
/// a pure literal (e.g. a function call or name reference).
fn const_to_json(expr: &ast::Expr) -> Option<Value> {
    match expr {
        ast::Expr::Constant(c) => constant_to_json(&c.value),
        ast::Expr::List(l) => l
            .elts
            .iter()
            .map(const_to_json)
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        ast::Expr::Tuple(t) => t
            .elts
            .iter()
            .map(const_to_json)
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        ast::Expr::Dict(d) => {
            let mut m = Map::new();
            for (k, v) in d.keys.iter().zip(d.values.iter()) {
                let key = match k {
                    Some(ast::Expr::Constant(c)) => match &c.value {
                        ast::Constant::Str(s) => s.clone(),
                        _ => return None,
                    },
                    _ => return None,
                };
                m.insert(key, const_to_json(v)?);
            }
            Some(Value::Object(m))
        }
        // Negative numeric literals are parsed as USub on a constant.
        ast::Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::USub) => {
            match const_to_json(&u.operand)? {
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Some(Value::from(-i))
                    } else {
                        n.as_f64()
                            .and_then(|f| serde_json::Number::from_f64(-f))
                            .map(Value::Number)
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn constant_to_json(c: &ast::Constant) -> Option<Value> {
    match c {
        ast::Constant::None => Some(Value::Null),
        ast::Constant::Bool(b) => Some(Value::Bool(*b)),
        ast::Constant::Str(s) => Some(Value::String(s.clone())),
        ast::Constant::Int(i) => i.to_string().parse::<i64>().ok().map(Value::from),
        ast::Constant::Float(f) => serde_json::Number::from_f64(*f).map(Value::Number),
        _ => None,
    }
}

fn json_type_name(v: &Value) -> Option<&'static str> {
    match v {
        Value::Bool(_) => Some("boolean"),
        Value::Number(n) => Some(if n.is_f64() { "number" } else { "integer" }),
        Value::String(_) => Some("string"),
        Value::Array(_) => Some("array"),
        Value::Object(_) => Some("object"),
        Value::Null => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(src: &str) -> Value {
        match extract_main_schema(src) {
            MainSchema::Found(v) => v,
            _ => panic!("expected Found"),
        }
    }

    #[test]
    fn extracts_params_types_defaults_and_required() {
        let v = schema("def main(label: str, threshold: int = 0):\n    return 1\n");
        assert_eq!(v["properties"]["label"]["type"], json!("string"));
        assert_eq!(v["properties"]["threshold"]["type"], json!("integer"));
        assert_eq!(v["properties"]["threshold"]["default"], json!(0));
        assert_eq!(v["required"], json!(["label"]));
        assert_eq!(v["order"], json!(["label", "threshold"]));
    }

    #[test]
    fn no_params_is_empty_schema() {
        let v = schema("def main():\n    return 1\n");
        assert_eq!(v["properties"], json!({}));
        assert_eq!(v["required"], json!([]));
    }

    #[test]
    fn kwargs_sets_additional_properties() {
        let v = schema("def main(**kwargs):\n    return 1\n");
        assert_eq!(v["additionalProperties"], json!(true));
    }

    #[test]
    fn varargs_are_ignored() {
        let v = schema("def main(a, *args):\n    return 1\n");
        assert_eq!(v["order"], json!(["a"]));
    }

    #[test]
    fn optional_and_union_unwrap_to_inner_type() {
        let v = schema(
            "from typing import Optional\ndef main(a: Optional[int], b: str | None):\n    return 1\n",
        );
        assert_eq!(v["properties"]["a"]["type"], json!("integer"));
        assert_eq!(v["properties"]["b"]["type"], json!("string"));
    }

    #[test]
    fn ctx_param_is_excluded() {
        // `ctx` is worker-injected, not a flow input — it must not appear.
        let v = schema("def main(name, ctx):\n    return ctx\n");
        assert_eq!(v["order"], json!(["name"]));
        assert_eq!(v["properties"].get("ctx"), None);
        assert_eq!(v["required"], json!(["name"]));
    }

    #[test]
    fn async_main_is_detected() {
        let v = schema("async def main(x):\n    return x\n");
        assert_eq!(v["order"], json!(["x"]));
    }

    #[test]
    fn missing_main_reported() {
        assert!(matches!(
            extract_main_schema("def other():\n    pass\n"),
            MainSchema::NoMain
        ));
    }

    #[test]
    fn unparseable_reported() {
        assert!(matches!(
            extract_main_schema("def main( : invalid"),
            MainSchema::Unparseable
        ));
    }
}
