//! Nunjucks-compatible variable rendering with layered environments.
//!
//! - `{{ _.var }}` and `{{ var }}` both resolve against the merged context.
//! - template tags (`{% uuid 'v4' %}`, `{% now 'iso-8601' %}`,
//!   `{% base64 'encode', 'normal', 'x' %}`, `{% hash 'sha256', 'hex', 'x' %}`)
//!   are rewritten to function calls before rendering.
//! - Unresolved variables are reported by name.

pub mod faker;
mod tags;

use std::collections::{BTreeSet, HashMap};

use lsock_core::VarMap;
use minijinja::{Environment, UndefinedBehavior};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RenderError {
    #[error("unresolved variable{}: {}", if .0.len() == 1 { "" } else { "s" }, .0.join(", "))]
    Unresolved(Vec<String>),
    #[error("template error: {0}")]
    Template(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Return an error on any failure.
    Throw,
    /// Leave the input untouched on failure (used for environment pre-rendering).
    Keep,
}

/// One environment layer, lowest precedence first when passed to [`Context::build`].
#[derive(Debug, Clone)]
pub struct Layer {
    /// Human-readable source, e.g. "Base Environment", "Folder: Auth".
    pub name: String,
    pub vars: VarMap,
}

impl Layer {
    pub fn new(name: impl Into<String>, vars: VarMap) -> Self {
        Self {
            name: name.into(),
            vars,
        }
    }
}

/// The merged render context plus where each top-level key came from.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub vars: Map<String, Value>,
    pub sources: HashMap<String, String>,
}

impl Context {
    /// Merge layers, lowest precedence first:
    /// later layers override earlier ones, nested objects merge,
    /// self-referencing strings (`base_url: "{{ base_url }}/v2"`) render
    /// against the value from earlier layers, then up to 3 passes resolve
    /// cross-references between keys.
    pub fn build(renderer: &Renderer, layers: &[Layer]) -> Context {
        let mut ctx = Context::default();
        for layer in layers {
            for (key, val) in &layer.vars {
                if matches!(key.as_str(), "__proto__" | "constructor" | "prototype") {
                    continue;
                }
                let merged = match (val, ctx.vars.get(key)) {
                    (Value::String(s), _) if is_self_recursive(s, key) => Value::String(
                        renderer
                            .render_str(s, &ctx, Mode::Keep)
                            .unwrap_or_else(|_| s.clone()),
                    ),
                    (Value::Object(new), Some(Value::Object(old))) => {
                        let mut o = old.clone();
                        merge_objects(&mut o, new);
                        Value::Object(o)
                    }
                    _ => val.clone(),
                };
                ctx.vars.insert(key.clone(), merged);
                ctx.sources.insert(key.clone(), layer.name.clone());
            }
        }

        let keys: Vec<String> = ctx.vars.keys().cloned().collect();
        let mut settled: BTreeSet<String> = BTreeSet::new();
        for _ in 0..3 {
            for key in &keys {
                if settled.contains(key) {
                    continue;
                }
                let cur = ctx.vars[key].clone();
                let next = renderer
                    .render_value(&cur, &ctx, Mode::Keep)
                    .unwrap_or_else(|_| cur.clone());
                if next == cur {
                    settled.insert(key.clone());
                } else {
                    ctx.vars.insert(key.clone(), next);
                }
            }
        }
        ctx
    }

    /// Set a top-level variable (script-local or iteration data).
    pub fn set(&mut self, key: impl Into<String>, value: Value, source: &str) {
        let key = key.into();
        self.sources.insert(key.clone(), source.to_string());
        self.vars.insert(key, value);
    }

    /// Resolve a dotted path (`a.b.0.c`), with an optional leading `_.`.
    pub fn lookup(&self, path: &str) -> Option<&Value> {
        let path = path.strip_prefix("_.").unwrap_or(path);
        let mut parts = path.split('.');
        let mut cur = self.vars.get(parts.next()?)?;
        for p in parts {
            cur = match cur {
                Value::Object(m) => m.get(p)?,
                Value::Array(a) => a.get(p.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(cur)
    }

    /// Source layer of the top-level key in `path`.
    pub fn source_of(&self, path: &str) -> Option<&str> {
        let path = path.strip_prefix("_.").unwrap_or(path);
        self.sources
            .get(path.split('.').next()?)
            .map(String::as_str)
    }

    fn to_template_value(&self) -> minijinja::Value {
        let mut root = self.vars.clone();
        root.insert("_".into(), Value::Object(self.vars.clone()));
        minijinja::Value::from_serialize(&root)
    }
}

fn merge_objects(dst: &mut Map<String, Value>, src: &Map<String, Value>) {
    for (k, v) in src {
        match (dst.get_mut(k), v) {
            (Some(Value::Object(d)), Value::Object(s)) => merge_objects(d, s),
            _ => {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Self-reference check: `{{ ?key[ |][^}]*}}` — the value references its own key.
fn is_self_recursive(s: &str, key: &str) -> bool {
    let mut rest = s;
    while let Some(i) = rest.find("{{") {
        let after = rest[i + 2..].trim_start_matches(' ');
        let after = after.strip_prefix("_.").unwrap_or(after);
        if let Some(tail) = after.strip_prefix(key)
            && (tail.starts_with(' ') || tail.starts_with('|') || tail.starts_with("}}"))
        {
            return true;
        }
        rest = &rest[i + 2..];
    }
    false
}

/// Description of one `{{ ... }}` reference found in a string, for UI highlighting.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VarRef {
    pub name: String,
    pub resolved: bool,
    pub value: Option<Value>,
    pub source: Option<String>,
}

pub struct Renderer {
    env: Environment<'static>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        env.set_formatter(|out, state, value| {
            if value.is_none() || value.is_undefined() {
                return Ok(());
            }
            if matches!(
                value.kind(),
                minijinja::value::ValueKind::Map | minijinja::value::ValueKind::Seq
            ) {
                // Objects render as JSON (more useful than Nunjucks' "[object Object]").
                let json = serde_json::to_string(&value).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                return out.write_str(&json).map_err(minijinja::Error::from);
            }
            minijinja::escape_formatter(out, state, value)
        });
        tags::register(&mut env);
        Self { env }
    }

    /// Render one string. Strings without template syntax are returned as-is.
    pub fn render_str(
        &self,
        input: &str,
        ctx: &Context,
        mode: Mode,
    ) -> Result<String, RenderError> {
        if !has_template_syntax(input) {
            return Ok(input.to_string());
        }
        match self.render_inner(input, ctx) {
            Ok(out) => {
                // A variable whose value is a tag (`{{ id }}` → `{% uuid %}`) renders twice,
                // but only when the original input had no tags of its own (see render.ts).
                if !input.contains("{%") && out.contains("{%") {
                    return self
                        .render_inner(&out, ctx)
                        .or_else(|e| keep_or(mode, input, e));
                }
                Ok(out)
            }
            Err(e) => keep_or(mode, input, e),
        }
    }

    fn render_inner(&self, input: &str, ctx: &Context) -> Result<String, RenderError> {
        let src = tags::rewrite(input).map_err(RenderError::Template)?;
        let tmpl = self
            .env
            .template_from_str(&src)
            .map_err(|e| RenderError::Template(fmt_err(&e)))?;
        let missing: Vec<String> = tmpl
            .undeclared_variables(true)
            .into_iter()
            .filter(|v| {
                !v.starts_with("__tag_") && self.env.globals().all(|(g, _)| g != v.as_str())
            })
            .filter(|v| v != "_" && ctx.lookup(v).is_none())
            .map(|v| v.strip_prefix("_.").unwrap_or(&v).to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !missing.is_empty() {
            return Err(RenderError::Unresolved(missing));
        }
        tmpl.render(ctx.to_template_value())
            .map_err(|e| RenderError::Template(fmt_err(&e)))
    }

    /// Render every string inside a JSON value. Objects with `"disabled": true` are skipped.
    pub fn render_value(&self, v: &Value, ctx: &Context, mode: Mode) -> Result<Value, RenderError> {
        Ok(match v {
            Value::String(s) => Value::String(self.render_str(s, ctx, mode)?),
            Value::Array(a) => Value::Array(
                a.iter()
                    .map(|x| self.render_value(x, ctx, mode))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(o) if o.get("disabled") == Some(&Value::Bool(true)) => v.clone(),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| Ok((k.clone(), self.render_value(x, ctx, mode)?)))
                    .collect::<Result<_, RenderError>>()?,
            ),
            _ => v.clone(),
        })
    }

    /// List the variable references in `input` with their resolution (for the UI).
    pub fn references(&self, input: &str, ctx: &Context) -> Vec<VarRef> {
        let Ok(src) = tags::rewrite(input) else {
            return vec![];
        };
        let Ok(tmpl) = self.env.template_from_str(&src) else {
            return vec![];
        };
        let mut names: Vec<String> = tmpl
            .undeclared_variables(true)
            .into_iter()
            .filter(|v| !v.starts_with("__tag_") && v != "_")
            .collect();
        names.sort();
        names
            .into_iter()
            .map(|n| {
                let value = ctx.lookup(&n).cloned();
                VarRef {
                    resolved: value.is_some(),
                    source: ctx.source_of(&n).map(str::to_string),
                    name: n.strip_prefix("_.").unwrap_or(&n).to_string(),
                    value,
                }
            })
            .collect()
    }
}

fn keep_or(mode: Mode, input: &str, e: RenderError) -> Result<String, RenderError> {
    match mode {
        Mode::Keep => Ok(input.to_string()),
        Mode::Throw => Err(e),
    }
}

fn fmt_err(e: &minijinja::Error) -> String {
    match e.detail() {
        Some(d) => format!("{}: {d}", e.kind()),
        None => e.kind().to_string(),
    }
}

pub fn has_template_syntax(s: &str) -> bool {
    (s.contains("{{") && s.contains("}}"))
        || (s.contains("{%") && s.contains("%}"))
        || (s.contains("{#") && s.contains("#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vars(v: Value) -> VarMap {
        serde_json::from_value(v).unwrap()
    }

    fn ctx(layers: Vec<(&str, Value)>) -> (Renderer, Context) {
        let r = Renderer::new();
        let layers: Vec<Layer> = layers
            .into_iter()
            .map(|(n, v)| Layer::new(n, vars(v)))
            .collect();
        let c = Context::build(&r, &layers);
        (r, c)
    }

    #[test]
    fn both_variable_syntaxes() {
        let (r, c) = ctx(vec![(
            "base",
            json!({"host": "api.io", "n": 3, "obj": {"a": [1, {"b": "deep"}]}}),
        )]);
        assert_eq!(
            r.render_str("https://{{ _.host }}/{{host}}?n={{ n }}", &c, Mode::Throw)
                .unwrap(),
            "https://api.io/api.io?n=3"
        );
        assert_eq!(
            r.render_str("{{ _.obj.a[1].b }}", &c, Mode::Throw).unwrap(),
            "deep"
        );
        // names with dashes use bracket notation
        let (r2, c2) = ctx(vec![("base", json!({"api-key": "k1"}))]);
        assert_eq!(
            r2.render_str("{{ _['api-key'] }}", &c2, Mode::Throw)
                .unwrap(),
            "k1"
        );
        assert_eq!(
            r.render_str("{{ obj }}", &c, Mode::Throw).unwrap(),
            r#"{"a":[1,{"b":"deep"}]}"#
        );
    }

    #[test]
    fn precedence_later_layers_win_and_track_source() {
        let (_, c) = ctx(vec![
            ("Base", json!({"a": "base", "b": "base"})),
            ("Staging", json!({"a": "sub"})),
            ("Folder: Users", json!({"b": "folder"})),
        ]);
        assert_eq!(c.lookup("a"), Some(&json!("sub")));
        assert_eq!(c.lookup("_.b"), Some(&json!("folder")));
        assert_eq!(c.source_of("a"), Some("Staging"));
    }

    #[test]
    fn nested_objects_merge() {
        let (_, c) = ctx(vec![
            ("Base", json!({"auth": {"user": "u", "pass": "p"}})),
            ("Sub", json!({"auth": {"pass": "x"}})),
        ]);
        assert_eq!(c.lookup("auth"), Some(&json!({"user": "u", "pass": "x"})));
    }

    #[test]
    fn self_recursive_values_extend_parent_value() {
        let (_, c) = ctx(vec![
            ("Base", json!({"base_url": "google.com"})),
            ("Sub", json!({"base_url": "{{ base_url }}/foo"})),
        ]);
        assert_eq!(c.lookup("base_url"), Some(&json!("google.com/foo")));
    }

    #[test]
    fn cross_references_resolve_across_layers() {
        let (_, c) = ctx(vec![
            ("Base", json!({"url": "https://{{ host }}/{{ _.ver }}"})),
            ("Sub", json!({"host": "staging.io", "ver": "v2"})),
        ]);
        assert_eq!(c.lookup("url"), Some(&json!("https://staging.io/v2")));
    }

    #[test]
    fn unresolved_variables_are_named() {
        let (r, c) = ctx(vec![("Base", json!({"a": 1}))]);
        let err = r
            .render_str("{{ a }} {{ _.missing }} {{ other.x }}", &c, Mode::Throw)
            .unwrap_err();
        assert_eq!(
            err,
            RenderError::Unresolved(vec!["missing".into(), "other.x".into()])
        );
        assert_eq!(
            r.render_str("{{ nope }}", &c, Mode::Keep).unwrap(),
            "{{ nope }}"
        );
    }

    #[test]
    fn null_renders_empty_and_plain_strings_untouched() {
        let (r, c) = ctx(vec![("Base", json!({"n": null}))]);
        assert_eq!(r.render_str("[{{ n }}]", &c, Mode::Throw).unwrap(), "[]");
        assert_eq!(
            r.render_str("no {braces} here", &c, Mode::Throw).unwrap(),
            "no {braces} here"
        );
    }

    #[test]
    fn render_value_skips_disabled_entries() {
        let (r, c) = ctx(vec![("Base", json!({"t": "tok"}))]);
        let v = json!([{"name": "A", "value": "{{ t }}"}, {"name": "B", "value": "{{ nope }}", "disabled": true}]);
        let out = r.render_value(&v, &c, Mode::Throw).unwrap();
        assert_eq!(out[0]["value"], "tok");
        assert_eq!(out[1]["value"], "{{ nope }}");
    }

    #[test]
    fn variable_holding_a_tag_is_rendered_twice() {
        let (r, c) = ctx(vec![(
            "Base",
            json!({"id": "{% base64 'encode', 'normal', 'hi' %}"}),
        )]);
        assert_eq!(r.render_str("{{ id }}", &c, Mode::Throw).unwrap(), "aGk=");
    }

    #[test]
    fn references_report_resolution_and_source() {
        let (r, c) = ctx(vec![("Base", json!({"host": "h"}))]);
        let refs = r.references("{{ _.host }}/{{ missing }}", &c);
        assert_eq!(refs.len(), 2);
        let host = refs.iter().find(|x| x.name == "host").unwrap();
        assert!(host.resolved);
        assert_eq!(host.source.as_deref(), Some("Base"));
        assert!(!refs.iter().find(|x| x.name == "missing").unwrap().resolved);
    }
}
