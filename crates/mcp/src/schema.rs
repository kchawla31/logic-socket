//! Turn a tool's JSON Schema into readable rows and validate arguments.
//!
//! Rows power both `lsock mcp tools` (terminal table) and the desktop
//! inspector's parameter table, so both read the schema the same way.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ParamRow {
    /// Dotted path, e.g. `assignee.login`, `labels[]`.
    pub path: String,
    pub name: String,
    pub depth: usize,
    /// Readable type, e.g. `string`, `integer`, `array<string>`, `string | null`, `object`.
    pub type_label: String,
    pub required: bool,
    pub default: Option<Value>,
    pub enum_values: Vec<Value>,
    pub description: Option<String>,
    /// Human-readable constraints: `1 ≤ value ≤ 100`, `format: email`, `pattern: ^a`.
    pub constraints: Vec<String>,
}

const MAX_DEPTH: usize = 8;

/// Flatten an object schema into rows. Nested object properties follow their
/// parent with `depth + 1`.
pub fn param_rows(schema: &Value) -> Vec<ParamRow> {
    let mut rows = vec![];
    let mut seen = HashSet::new();
    let root = resolve(schema, schema, &mut seen);
    walk_object(schema, &root, "", 0, &mut rows, &mut HashSet::new());
    rows
}

fn walk_object(
    root: &Value,
    obj: &Value,
    prefix: &str,
    depth: usize,
    rows: &mut Vec<ParamRow>,
    seen: &mut HashSet<String>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let required: HashSet<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let Some(props) = obj.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, prop) in props {
        let prop = resolve(root, prop, seen);
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        push_row(
            root,
            &prop,
            name,
            &path,
            required.contains(name.as_str()),
            depth,
            rows,
            seen,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_row(
    root: &Value,
    prop: &Value,
    name: &str,
    path: &str,
    required: bool,
    depth: usize,
    rows: &mut Vec<ParamRow>,
    seen: &mut HashSet<String>,
) {
    rows.push(ParamRow {
        path: path.to_string(),
        name: name.to_string(),
        depth,
        type_label: type_label(root, prop, &mut seen.clone()),
        required,
        default: prop.get("default").cloned(),
        enum_values: enum_values(prop),
        description: prop
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        constraints: constraints(prop),
    });
    if is_object(prop) {
        walk_object(root, prop, path, depth + 1, rows, seen);
    } else if let Some(items) = prop.get("items") {
        let items = resolve(root, items, seen);
        if is_object(&items) {
            walk_object(root, &items, &format!("{path}[]"), depth + 1, rows, seen);
        }
    }
}

fn is_object(v: &Value) -> bool {
    v.get("properties").is_some() || v.get("type").and_then(Value::as_str) == Some("object")
}

/// Resolve a local `$ref` (`#/$defs/X`, `#/definitions/X`), guarding cycles.
fn resolve(root: &Value, v: &Value, seen: &mut HashSet<String>) -> Value {
    let Some(r) = v.get("$ref").and_then(Value::as_str) else {
        return v.clone();
    };
    if !seen.insert(r.to_string()) {
        return serde_json::json!({ "type": "object", "description": format!("(recursive {r})") });
    }
    let target = r.strip_prefix('#').and_then(|p| root.pointer(p)).cloned();
    match target {
        Some(t) => {
            // Sibling keywords next to $ref (e.g. description) override the target's.
            let mut merged = t.as_object().cloned().unwrap_or_default();
            if let Some(o) = v.as_object() {
                for (k, val) in o.iter().filter(|(k, _)| *k != "$ref") {
                    merged.insert(k.clone(), val.clone());
                }
            }
            resolve(root, &Value::Object(merged), seen)
        }
        None => serde_json::json!({ "description": format!("unresolved {r}") }),
    }
}

pub fn type_label(root: &Value, v: &Value, seen: &mut HashSet<String>) -> String {
    let v = resolve(root, v, seen);
    for key in ["anyOf", "oneOf"] {
        if let Some(alts) = v.get(key).and_then(Value::as_array) {
            let mut labels: Vec<String> = alts
                .iter()
                .map(|a| type_label(root, a, &mut seen.clone()))
                .collect();
            labels.dedup();
            return labels.join(" | ");
        }
    }
    if let Some(all) = v.get("allOf").and_then(Value::as_array)
        && all.len() == 1
    {
        return type_label(root, &all[0], seen);
    }
    let base = match v.get("type") {
        Some(Value::String(t)) => single_type(root, &v, t, seen),
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .map(|t| single_type(root, &v, t, &mut seen.clone()))
            .collect::<Vec<_>>()
            .join(" | "),
        _ if v.get("properties").is_some() => "object".into(),
        _ if v.get("enum").is_some() => "enum".into(),
        _ if v.get("const").is_some() => format!("const {}", v["const"]),
        _ => "any".into(),
    };
    if v.get("nullable") == Some(&Value::Bool(true)) {
        format!("{base} | null")
    } else {
        base
    }
}

fn single_type(root: &Value, v: &Value, t: &str, seen: &mut HashSet<String>) -> String {
    match t {
        "array" => match v.get("items") {
            Some(items) => format!("array<{}>", type_label(root, items, seen)),
            None => "array".into(),
        },
        "string" => match v.get("format").and_then(Value::as_str) {
            Some(f) => format!("string ({f})"),
            None => "string".into(),
        },
        other => other.to_string(),
    }
}

fn enum_values(v: &Value) -> Vec<Value> {
    if let Some(e) = v.get("enum").and_then(Value::as_array) {
        return e.clone();
    }
    // oneOf of consts is a common "labelled enum" pattern
    v.get("oneOf")
        .and_then(Value::as_array)
        .map(|alts| {
            alts.iter()
                .filter_map(|a| a.get("const").cloned())
                .collect()
        })
        .unwrap_or_default()
}

fn constraints(v: &Value) -> Vec<String> {
    let num = |k: &str| v.get(k).filter(|x| x.is_number()).map(|x| x.to_string());
    let mut out = vec![];
    match (
        num("minimum").or(num("exclusiveMinimum")),
        num("maximum").or(num("exclusiveMaximum")),
    ) {
        (Some(a), Some(b)) => out.push(format!("{a} ≤ value ≤ {b}")),
        (Some(a), None) => out.push(format!("≥ {a}")),
        (None, Some(b)) => out.push(format!("≤ {b}")),
        _ => {}
    }
    match (num("minLength"), num("maxLength")) {
        (Some(a), Some(b)) => out.push(format!("length {a}–{b}")),
        (Some(a), None) => out.push(format!("min length {a}")),
        (None, Some(b)) => out.push(format!("max length {b}")),
        _ => {}
    }
    match (num("minItems"), num("maxItems")) {
        (Some(a), Some(b)) => out.push(format!("{a}–{b} items")),
        (Some(a), None) => out.push(format!("≥ {a} items")),
        (None, Some(b)) => out.push(format!("≤ {b} items")),
        _ => {}
    }
    if let Some(p) = v.get("pattern").and_then(Value::as_str) {
        out.push(format!("pattern: {p}"));
    }
    if v.get("uniqueItems") == Some(&Value::Bool(true)) {
        out.push("unique items".into());
    }
    if let Some(items) = v.get("items") {
        let e = enum_values(items);
        if !e.is_empty() {
            let opts: Vec<String> = e
                .iter()
                .map(|x| match x {
                    Value::String(s) => format!("\"{s}\""),
                    o => o.to_string(),
                })
                .collect();
            out.push(format!("each one of: {}", opts.join(", ")));
        }
    }
    out
}

/// Lightweight validation of tool arguments against an input schema.
/// Returns human-readable problems; empty means valid.
pub fn validate(schema: &Value, args: &Value) -> Vec<String> {
    let mut errs = vec![];
    check(schema, schema, args, "", &mut errs, 0);
    errs
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_ok(expected: &str, v: &Value) -> bool {
    let actual = json_type(v);
    expected == actual || (expected == "number" && actual == "integer")
}

fn check(
    root: &Value,
    schema: &Value,
    v: &Value,
    path: &str,
    errs: &mut Vec<String>,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let schema = resolve(root, schema, &mut HashSet::new());
    let at = if path.is_empty() {
        "arguments".to_string()
    } else {
        format!("'{path}'")
    };

    for key in ["anyOf", "oneOf"] {
        if let Some(alts) = schema.get(key).and_then(Value::as_array) {
            let ok = alts.iter().any(|a| {
                let mut e = vec![];
                check(root, a, v, path, &mut e, depth + 1);
                e.is_empty()
            });
            if !ok {
                errs.push(format!(
                    "{at} does not match any allowed shape ({})",
                    type_label(root, &schema, &mut HashSet::new())
                ));
            }
            return;
        }
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    let nullable = schema.get("nullable") == Some(&Value::Bool(true));
    if !types.is_empty() && !types.iter().any(|t| type_ok(t, v)) && !(nullable && v.is_null()) {
        errs.push(format!(
            "{at} should be {} but is {}",
            types.join(" | "),
            json_type(v)
        ));
        return;
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        let opts: Vec<String> = e.iter().map(Value::to_string).collect();
        errs.push(format!("{at} must be one of {}", opts.join(", ")));
    }
    if let Some(n) = v.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
            && n < min
        {
            errs.push(format!("{at} must be ≥ {min}"));
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
            && n > max
        {
            errs.push(format!("{at} must be ≤ {max}"));
        }
    }
    if let Some(s) = v.as_str() {
        let len = s.chars().count() as u64;
        if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
            && len < min
        {
            errs.push(format!("{at} must be at least {min} characters"));
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
            && len > max
        {
            errs.push(format!("{at} must be at most {max} characters"));
        }
    }
    if let (Some(obj), Some(props)) = (
        v.as_object(),
        schema.get("properties").and_then(Value::as_object),
    ) {
        check_object(root, &schema, obj, props, path, errs, depth);
    } else if let (Some(obj), None) = (v.as_object(), schema.get("properties")) {
        check_object(root, &schema, obj, &Map::new(), path, errs, depth);
    }
    if let (Some(arr), Some(items)) = (v.as_array(), schema.get("items")) {
        for (i, item) in arr.iter().enumerate() {
            check(root, items, item, &format!("{path}[{i}]"), errs, depth + 1);
        }
    }
}

fn check_object(
    root: &Value,
    schema: &Value,
    obj: &Map<String, Value>,
    props: &Map<String, Value>,
    path: &str,
    errs: &mut Vec<String>,
    depth: usize,
) {
    let join = |k: &str| {
        if path.is_empty() {
            k.to_string()
        } else {
            format!("{path}.{k}")
        }
    };
    for r in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !obj.contains_key(r) {
            errs.push(format!("'{}' is required", join(r)));
        }
    }
    for (k, val) in obj {
        match props.get(k) {
            Some(p) => check(root, p, val, &join(k), errs, depth + 1),
            None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                errs.push(format!("'{}' is not an allowed property", join(k)))
            }
            None => {}
        }
    }
}

/// Build an example argument object from a schema (defaults, first enum
/// value, or a type placeholder). Used to pre-fill the call form.
pub fn example_args(schema: &Value) -> Value {
    example(schema, schema, 0, &mut HashSet::new())
}

fn example(root: &Value, v: &Value, depth: usize, seen: &mut HashSet<String>) -> Value {
    let v = resolve(root, v, seen);
    if let Some(d) = v.get("default") {
        return d.clone();
    }
    if let Some(e) = enum_values(&v).into_iter().next() {
        return e;
    }
    if let Some(alt) = v
        .get("anyOf")
        .or(v.get("oneOf"))
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x["type"] != "null"))
    {
        return example(root, alt, depth, seen);
    }
    let t = match v.get("type") {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or("null")
            .to_string(),
        _ if v.get("properties").is_some() => "object".into(),
        _ => "string".into(),
    };
    match t.as_str() {
        "object" if depth < MAX_DEPTH => {
            let mut m = Map::new();
            let required: HashSet<&str> = v
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if let Some(props) = v.get("properties").and_then(Value::as_object) {
                for (k, p) in props {
                    if required.is_empty() || required.contains(k.as_str()) {
                        m.insert(k.clone(), example(root, p, depth + 1, &mut seen.clone()));
                    }
                }
            }
            Value::Object(m)
        }
        "array" => Value::Array(vec![]),
        "integer" => v.get("minimum").cloned().unwrap_or(Value::from(0)),
        "number" => v.get("minimum").cloned().unwrap_or(Value::from(0)),
        "boolean" => Value::Bool(false),
        "null" => Value::Null,
        _ => Value::String(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "required": ["title", "assignee"],
            "properties": {
                "title": {"type": "string", "description": "Issue title", "minLength": 1, "maxLength": 80},
                "priority": {"type": "string", "enum": ["low", "high"], "default": "low"},
                "labels": {"type": "array", "items": {"type": "string"}, "uniqueItems": true},
                "assignee": {"$ref": "#/$defs/user", "description": "Who owns it"},
                "due": {"type": ["string", "null"], "format": "date"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100},
                "steps": {"type": "array", "items": {"type": "object", "properties": {"n": {"type": "integer"}}}}
            },
            "$defs": {"user": {"type": "object", "required": ["login"], "properties": {
                "login": {"type": "string"}, "id": {"anyOf": [{"type": "integer"}, {"type": "null"}]}
            }}}
        })
    }

    #[test]
    fn rows_are_readable() {
        let rows = param_rows(&schema());
        let get = |p: &str| {
            rows.iter()
                .find(|r| r.path == p)
                .unwrap_or_else(|| panic!("missing {p}"))
        };
        assert_eq!(
            rows.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            [
                "title",
                "priority",
                "labels",
                "assignee",
                "assignee.login",
                "assignee.id",
                "due",
                "limit",
                "steps",
                "steps[].n"
            ]
        );
        assert!(get("title").required);
        assert_eq!(get("title").constraints, ["length 1–80"]);
        assert_eq!(get("priority").enum_values, [json!("low"), json!("high")]);
        assert_eq!(get("priority").default, Some(json!("low")));
        assert_eq!(get("labels").type_label, "array<string>");
        assert_eq!(get("assignee").type_label, "object");
        assert_eq!(get("assignee").description.as_deref(), Some("Who owns it"));
        assert_eq!(
            (get("assignee.login").depth, get("assignee.login").required),
            (1, true)
        );
        assert_eq!(get("assignee.id").type_label, "integer | null");
        assert_eq!(get("due").type_label, "string (date) | null");
        assert_eq!(get("limit").constraints, ["1 ≤ value ≤ 100"]);
        let kinds = json!({"type": "array", "items": {"type": "string", "enum": ["a", "b"]}});
        assert_eq!(constraints(&kinds), ["each one of: \"a\", \"b\""]);
    }

    #[test]
    fn recursive_refs_terminate() {
        let s = json!({"type": "object", "properties": {"node": {"$ref": "#/$defs/n"}},
            "$defs": {"n": {"type": "object", "properties": {"child": {"$ref": "#/$defs/n"}}}}});
        let rows = param_rows(&s);
        assert!(rows.len() < 20);
        assert_eq!(rows[0].path, "node");
    }

    #[test]
    fn validation_messages() {
        let s = schema();
        assert!(validate(&s, &json!({"title": "x", "assignee": {"login": "a"}})).is_empty());
        let errs = validate(
            &s,
            &json!({"title": "", "priority": "urgent", "limit": 500, "assignee": {}, "labels": [1]}),
        );
        assert!(
            errs.contains(&"'title' must be at least 1 characters".to_string()),
            "{errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| e.starts_with("'priority' must be one of"))
        );
        assert!(errs.contains(&"'limit' must be ≤ 100".to_string()));
        assert!(errs.contains(&"'assignee.login' is required".to_string()));
        assert!(errs.contains(&"'labels[0]' should be string but is integer".to_string()));
        assert!(
            validate(
                &s,
                &json!({"title": "x", "assignee": {"login": "a", "id": "nope"}})
            )[0]
            .contains("assignee.id")
        );
    }

    #[test]
    fn additional_properties_false_rejects_unknown_keys() {
        let s = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"a": {"type": "string"}}
        });
        let errs = validate(&s, &json!({"a": "ok", "b": 1}));
        assert!(
            errs.iter().any(|e| e.contains("'b'") && e.contains("not an allowed")),
            "{errs:?}"
        );
        assert!(validate(&s, &json!({"a": "ok"})).is_empty());
    }

    /// Numeric `exclusiveMinimum` / `exclusiveMaximum` (JSON Schema draft 6+)
    /// are endpoints the value must not equal. The inspector prints them with
    /// `≤`, and `validate` only reads `minimum` / `maximum`.
    #[test]
    #[ignore = "BUG-003"]
    fn exclusive_numeric_bounds_are_exclusive() {
        let s = json!({
            "type": "object",
            "properties": {
                "n": {"type": "number", "exclusiveMinimum": 0, "exclusiveMaximum": 10}
            }
        });
        let shown = param_rows(&s)
            .into_iter()
            .find(|r| r.path == "n")
            .map(|r| r.constraints)
            .unwrap_or_default();
        let at_min = validate(&s, &json!({"n": 0}));
        let at_max = validate(&s, &json!({"n": 10}));
        let inside = validate(&s, &json!({"n": 5}));
        assert!(
            shown.iter().any(|c| c.contains('<') || c.contains('>'))
                && at_min.iter().any(|e| e.contains('n'))
                && at_max.iter().any(|e| e.contains('n'))
                && inside.is_empty(),
            "shown={shown:?} at_min={at_min:?} at_max={at_max:?} inside={inside:?}"
        );
    }

    /// `oneOf` is exactly one match. `anyOf` / `oneOf` do not skip `required`.
    #[test]
    #[ignore = "BUG-012"]
    fn one_of_is_exclusive_and_sibling_keywords_still_apply() {
        let both = validate(
            &json!({"oneOf":[{"type":"string"},{"minLength":1}]}),
            &json!("ab"),
        );
        let missing = validate(
            &json!({
                "type": "object",
                "required": ["a"],
                "anyOf": [{"properties": {"a": {"type": "string"}}}]
            }),
            &json!({}),
        );
        assert!(
            !both.is_empty() && !missing.is_empty(),
            "both={both:?} missing={missing:?}"
        );
    }

    /// Constraints the parameter table prints must be the same rules `validate`
    /// enforces. `pattern`, `minItems`, and `uniqueItems` are displayed and then
    /// accepted.
    #[test]
    #[ignore = "BUG-004"]
    fn displayed_pattern_items_and_uniqueness_are_enforced() {
        let s = json!({
            "type": "object",
            "properties": {
                "repo": {"type": "string", "pattern": "^[\\w-]+/[\\w-]+$"},
                "labels": {
                    "type": "array",
                    "minItems": 2,
                    "uniqueItems": true,
                    "items": {"type": "string"}
                }
            }
        });
        let rows = param_rows(&s);
        let repo_c = rows.iter().find(|r| r.path == "repo").map(|r| r.constraints.clone());
        let labels_c = rows.iter().find(|r| r.path == "labels").map(|r| r.constraints.clone());
        let bad_pattern = validate(&s, &json!({"repo": "not a repo", "labels": ["a", "b"]}));
        let dupes = validate(&s, &json!({"repo": "acme/api", "labels": ["a", "a"]}));
        let too_few = validate(&s, &json!({"repo": "acme/api", "labels": ["a"]}));
        assert!(
            repo_c.as_ref().is_some_and(|c| c.iter().any(|x| x.contains("pattern")))
                && labels_c.as_ref().is_some_and(|c| c.iter().any(|x| x.contains("unique")))
                && labels_c.as_ref().is_some_and(|c| c.iter().any(|x| x.contains("items")))
                && bad_pattern.iter().any(|e| e.contains("repo"))
                && dupes.iter().any(|e| e.contains("labels"))
                && too_few.iter().any(|e| e.contains("labels")),
            "repo_c={repo_c:?} labels_c={labels_c:?} bad_pattern={bad_pattern:?} dupes={dupes:?} too_few={too_few:?}"
        );
    }

    #[test]
    fn example_args_fill_required_with_defaults() {
        assert_eq!(
            example_args(&schema()),
            json!({"title": "", "assignee": {"login": ""}})
        );
        let s = json!({"type": "object", "properties": {"u": {"type": "string", "enum": ["c", "f"]}, "n": {"type": "integer", "minimum": 1}}});
        assert_eq!(example_args(&s), json!({"u": "c", "n": 1}));
    }
}
