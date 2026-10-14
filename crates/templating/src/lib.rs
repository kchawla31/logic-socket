//! Variable rendering with layered environments.
//!
//! - Variables are `${name}`, `${user.id}`, or `${api-key}`. The `_` prefix is
//!   not required: `${_.name}` is accepted and means `${name}`.
//! - `{{ ... }}` is ordinary text. It is not a variable.
//! - Template tags (`{% uuid 'v4' %}`, `{% now 'iso-8601' %}`,
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
    /// self-referencing strings (`base_url: "${base_url}/v2"`) render
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

    /// Resolve `path`. A stored name that contains a period (`api.key`) wins
    /// over walking a shorter object path. Otherwise `a.b.0.c` walks objects
    /// and arrays. A leading `_.` is ignored.
    pub fn lookup(&self, path: &str) -> Option<&Value> {
        let path = path.strip_prefix("_.").unwrap_or(path);
        let (key, rest) = literal_key_prefix(path, &self.vars)?;
        walk(self.vars.get(key)?, rest)
    }

    /// Source layer of the stored key in `path`.
    pub fn source_of(&self, path: &str) -> Option<&str> {
        let path = path.strip_prefix("_.").unwrap_or(path);
        let key = match literal_key_prefix(path, &self.vars) {
            Some((key, _)) => key,
            None => path.split('.').next()?,
        };
        self.sources.get(key).map(String::as_str)
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

/// Self-reference check: the value references its own key, as in `${base_url}/v2`.
fn is_self_recursive(s: &str, key: &str) -> bool {
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        let after = rest[i + 2..].trim_start();
        let after = after.strip_prefix("_.").unwrap_or(after).trim_start();
        if let Some(tail) = after.strip_prefix(key) {
            let tail = tail.trim_start();
            if tail.starts_with('}')
                || tail.starts_with('|')
                || tail.starts_with('.')
                || tail.starts_with('[')
            {
                return true;
            }
        }
        rest = &rest[i + 2..];
    }
    false
}

/// `${name}` and `${user.id}`. A leading `_.` is ignored. `api-key` stays as written.
pub fn legacy_to_dollar(input: &str) -> String {
    if !input.contains("{{") {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if input[i..].starts_with("{%") || input[i..].starts_with("{#") {
            let closer = if input[i..].starts_with("{%") {
                "%}"
            } else {
                "#}"
            };
            if let Some(rel) = input[i + 2..].find(closer) {
                let end = i + 2 + rel + closer.len();
                out.push_str(&input[i..end]);
                i = end;
                continue;
            }
        }
        if input[i..].starts_with("{{")
            && let Some(rel) = input[i + 2..].find("}}")
        {
            let inner = &input[i + 2..i + 2 + rel];
            if !inner.contains('{') {
                out.push_str("${");
                out.push_str(&canonical_expr(inner));
                out.push('}');
                i = i + 2 + rel + 2;
                continue;
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Rewrite `{{ ... }}` variable text inside every string of a JSON value.
pub fn rewrite_json_values(v: &mut Value) -> bool {
    match v {
        Value::String(s) => {
            if !s.contains("{{") {
                return false;
            }
            let next = legacy_to_dollar(s);
            if next != *s {
                *s = next;
                true
            } else {
                false
            }
        }
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= rewrite_json_values(item);
            }
            changed
        }
        Value::Object(map) => {
            let mut changed = false;
            for item in map.values_mut() {
                changed |= rewrite_json_values(item);
            }
            changed
        }
        _ => false,
    }
}

fn canonical_expr(inner: &str) -> String {
    let t = inner.trim();
    let t = t.strip_prefix("_.").map(str::trim_start).unwrap_or(t);
    let (head, filters) = split_filter(t);
    let head = head.trim();
    let name = bracket_key(head).unwrap_or_else(|| head.to_string());
    if filters.is_empty() {
        name
    } else {
        format!("{name} | {}", filters.trim())
    }
}

/// Turn user text into MiniJinja. `${expr}` becomes a variable. `{{` stays literal.
fn to_minijinja(input: &str, ctx: &Context) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if input[i..].starts_with("{%") || input[i..].starts_with("{#") {
            let closer = if input[i..].starts_with("{%") {
                "%}"
            } else {
                "#}"
            };
            if let Some(rel) = input[i + 2..].find(closer) {
                let end = i + 2 + rel + closer.len();
                out.push_str(&input[i..end]);
                i = end;
                continue;
            }
        }
        if input[i..].starts_with("${")
            && let Some((expr, next)) = parse_dollar(input, i)
        {
            out.push_str("{{ ");
            out.push_str(&jinja_expr(&expr, ctx));
            out.push_str(" }}");
            i = next;
            continue;
        }
        if input[i..].starts_with("{{") {
            out.push_str("{% raw %}{{{% endraw %}");
            i += 2;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn parse_dollar(input: &str, start: usize) -> Option<(String, usize)> {
    let bytes = input.as_bytes();
    let mut j = start + 2;
    let mut depth = 1i32;
    let mut quote: Option<u8> = None;
    while j < bytes.len() {
        let c = bytes[j];
        if let Some(q) = quote {
            if c == b'\\' {
                j += 1 + usize::from(j + 1 < bytes.len());
                continue;
            }
            if c == q {
                quote = None;
            }
            j += 1;
            continue;
        }
        match c {
            b'\'' | b'"' => quote = Some(c),
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => {
                depth -= 1;
                if depth == 0 && c == b'}' {
                    return Some((input[start + 2..j].to_string(), j + 1));
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

fn jinja_expr(expr: &str, ctx: &Context) -> String {
    let t = expr.trim();
    let t = t.strip_prefix("_.").map(str::trim_start).unwrap_or(t);
    let (head, filters) = split_filter(t);
    let head = head.trim();
    let rendered = rendered_head(head, ctx);
    if filters.is_empty() {
        rendered
    } else {
        format!("{rendered} | {}", filters.trim())
    }
}

fn split_filter(s: &str) -> (&str, &str) {
    let bytes = s.as_bytes();
    let mut quote: Option<u8> = None;
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 1 + usize::from(i + 1 < bytes.len());
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' => quote = Some(c),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'|' if depth == 0 => return (&s[..i], &s[i + 1..]),
            _ => {}
        }
        i += 1;
    }
    (s, "")
}

fn bracket_key(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("_['")
        && let Some(name) = rest.strip_suffix("']")
    {
        return Some(name.replace("\\'", "'"));
    }
    if let Some(rest) = s.strip_prefix("_[\"")
        && let Some(name) = rest.strip_suffix("\"]")
    {
        return Some(name.to_string());
    }
    None
}

/// How `${head}` is written for MiniJinja.
///
/// A stored key whose name is not a plain identifier (`api.key`, `api-key`)
/// is read with `_['api.key']`. A plain name such as `user.id` stays an
/// attribute path so a missing field is still reported as `user.id`.
fn rendered_head(head: &str, ctx: &Context) -> String {
    if let Some(name) = bracket_key(head) {
        return format!("_['{}']", escape_sq(&name));
    }
    if loose_name(head) {
        return format!("_['{}']", escape_sq(head));
    }
    if let Some((key, rest)) = literal_key_prefix(head, &ctx.vars)
        && !plain_ident(key)
    {
        let base = format!("_['{}']", escape_sq(key));
        if rest.is_empty() {
            return base;
        }
        if rest.starts_with('[') {
            return format!("{base}{rest}");
        }
        return format!("{base}.{rest}");
    }
    head.to_string()
}

/// Longest stored key that is `head` or a dot-separated prefix of it.
/// `obj.a[1].b` can match the key `obj`, leaving the rest `a[1].b`.
fn literal_key_prefix<'a>(head: &'a str, vars: &Map<String, Value>) -> Option<(&'a str, &'a str)> {
    if vars.contains_key(head) {
        return Some((head, ""));
    }
    let bytes = head.as_bytes();
    let mut depth = 0i32;
    let mut cuts = Vec::new();
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'[' | b'(' => depth += 1,
            b']' | b')' => depth -= 1,
            b'.' if depth == 0 => cuts.push(i),
            _ => {}
        }
    }
    for &i in cuts.iter().rev() {
        let prefix = &head[..i];
        if !prefix.is_empty() && vars.contains_key(prefix) {
            return Some((prefix, &head[i + 1..]));
        }
    }
    None
}

fn walk<'a>(mut cur: &'a Value, rest: &str) -> Option<&'a Value> {
    if rest.is_empty() {
        return Some(cur);
    }
    for part in rest.split('.') {
        if part.is_empty() {
            return None;
        }
        cur = match cur {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn plain_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A single token that MiniJinja would not accept as an identifier (`api-key`).
fn loose_name(s: &str) -> bool {
    if s.is_empty() || s.contains(['.', '[', ']', '(', ')', ' ', '\t', '{', '}', '|', '\'', '"']) {
        return false;
    }
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let ident = (first.is_ascii_alphabetic() || first == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    !ident
}

fn escape_sq(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// True when a `${...}` block has no expression (`${}`, `${ }`, `${_.}`).
fn has_empty_dollar(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if input[i..].starts_with("{%") || input[i..].starts_with("{#") {
            let closer = if input[i..].starts_with("{%") {
                "%}"
            } else {
                "#}"
            };
            if let Some(rel) = input[i + 2..].find(closer) {
                i = i + 2 + rel + closer.len();
                continue;
            }
        }
        if input[i..].starts_with("${")
            && let Some((expr, next)) = parse_dollar(input, i)
        {
            if dollar_head_empty(&expr) {
                return true;
            }
            i = next;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        i += ch.len_utf8();
    }
    false
}

fn dollar_head_empty(expr: &str) -> bool {
    let t = expr.trim();
    let t = t.strip_prefix("_.").map(str::trim_start).unwrap_or(t);
    let (head, _) = split_filter(t);
    head.trim().is_empty()
}

fn dollar_names(input: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if input[i..].starts_with("{%") || input[i..].starts_with("{#") {
            let closer = if input[i..].starts_with("{%") {
                "%}"
            } else {
                "#}"
            };
            if let Some(rel) = input[i + 2..].find(closer) {
                i = i + 2 + rel + closer.len();
                continue;
            }
        }
        if input[i..].starts_with("${")
            && let Some((expr, next)) = parse_dollar(input, i)
        {
            let name = canonical_expr(&expr);
            let name = name.split('|').next().unwrap_or(&name).trim().to_string();
            if !name.is_empty() && name != "_" {
                names.push(name);
            }
            i = next;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        i += ch.len_utf8();
    }
    names
}

/// Description of one `${...}` reference found in a string, for UI highlighting.
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
        // `${}` is what you get while typing a variable. MiniJinja would see `{{  }}`
        // and report "unexpected end of variable block", which does not name the cause.
        if has_empty_dollar(input) {
            return keep_or(mode, input, RenderError::Template("empty variable".into()));
        }
        let prepared = to_minijinja(input, ctx);
        match self.render_inner(&prepared, ctx) {
            Ok(out) => {
                // A variable whose value is a tag (`${id}` → `{% uuid %}`) renders twice,
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
        let mut names = dollar_names(input);
        names.sort();
        names.dedup();
        names
            .into_iter()
            .map(|n| {
                let value = ctx.lookup(&n).cloned();
                VarRef {
                    resolved: value.is_some(),
                    source: ctx.source_of(&n).map(str::to_string),
                    name: n,
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
    (s.contains("${") && s.contains('}'))
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
    fn dollar_variables_resolve_without_underscore() {
        let (r, c) = ctx(vec![(
            "base",
            json!({"host": "api.io", "n": 3, "obj": {"a": [1, {"b": "deep"}]}}),
        )]);
        assert_eq!(
            r.render_str("https://${host}/${_.host}?n=${ n }", &c, Mode::Throw)
                .unwrap(),
            "https://api.io/api.io?n=3"
        );
        assert_eq!(
            r.render_str("${obj.a[1].b}", &c, Mode::Throw).unwrap(),
            "deep"
        );
        let (r2, c2) = ctx(vec![("base", json!({"api-key": "k1"}))]);
        assert_eq!(r2.render_str("${api-key}", &c2, Mode::Throw).unwrap(), "k1");
        assert_eq!(
            r.render_str("${obj}", &c, Mode::Throw).unwrap(),
            r#"{"a":[1,{"b":"deep"}]}"#
        );
        assert_eq!(
            r.render_str("keep {{ host }} literal", &c, Mode::Throw)
                .unwrap(),
            "keep {{ host }} literal"
        );
        assert_eq!(
            legacy_to_dollar("{{ _.host }}/{{ _['api-key'] }}/{% uuid 'v4' %}"),
            "${host}/${api-key}/{% uuid 'v4' %}"
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
            ("Sub", json!({"base_url": "${base_url}/foo"})),
        ]);
        assert_eq!(c.lookup("base_url"), Some(&json!("google.com/foo")));
    }

    #[test]
    fn cross_references_resolve_across_layers() {
        let (_, c) = ctx(vec![
            ("Base", json!({"url": "https://${host}/${ver}"})),
            ("Sub", json!({"host": "staging.io", "ver": "v2"})),
        ]);
        assert_eq!(c.lookup("url"), Some(&json!("https://staging.io/v2")));
    }

    #[test]
    fn unresolved_variables_are_named() {
        let (r, c) = ctx(vec![("Base", json!({"a": 1}))]);
        let err = r
            .render_str("${a} ${missing} ${other.x}", &c, Mode::Throw)
            .unwrap_err();
        assert_eq!(
            err,
            RenderError::Unresolved(vec!["missing".into(), "other.x".into()])
        );
        assert_eq!(r.render_str("${nope}", &c, Mode::Keep).unwrap(), "${nope}");
    }

    #[test]
    fn null_renders_empty_and_plain_strings_untouched() {
        let (r, c) = ctx(vec![("Base", json!({"n": null}))]);
        assert_eq!(r.render_str("[${n}]", &c, Mode::Throw).unwrap(), "[]");
        assert_eq!(
            r.render_str("no {braces} here", &c, Mode::Throw).unwrap(),
            "no {braces} here"
        );
    }

    #[test]
    fn render_value_skips_disabled_entries() {
        let (r, c) = ctx(vec![("Base", json!({"t": "tok"}))]);
        let v = json!([{"name": "A", "value": "${t}"}, {"name": "B", "value": "${nope}", "disabled": true}]);
        let out = r.render_value(&v, &c, Mode::Throw).unwrap();
        assert_eq!(out[0]["value"], "tok");
        assert_eq!(out[1]["value"], "${nope}");
    }

    #[test]
    fn variable_holding_a_tag_is_rendered_twice() {
        let (r, c) = ctx(vec![(
            "Base",
            json!({"id": "{% base64 'encode', 'normal', 'hi' %}"}),
        )]);
        assert_eq!(r.render_str("${id}", &c, Mode::Throw).unwrap(), "aGk=");
    }

    #[test]
    fn empty_dollar_is_an_empty_variable_not_a_syntax_error() {
        let (r, c) = ctx(vec![("Base", json!({}))]);
        for sample in ["${}", "${ }", "${_.}", "pre ${} post"] {
            let err = r.render_str(sample, &c, Mode::Throw).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("empty variable"), "{sample}: {msg}");
            assert!(!msg.contains("unexpected end"), "{sample}: {msg}");
            assert_eq!(r.render_str(sample, &c, Mode::Keep).unwrap(), sample);
        }
        let (r, c) = ctx(vec![("Base", json!({"host": "h"}))]);
        assert_eq!(r.render_str("ok ${host}", &c, Mode::Throw).unwrap(), "ok h");
    }

    #[test]
    fn references_report_resolution_and_source() {
        let (r, c) = ctx(vec![("Base", json!({"host": "h"}))]);
        let refs = r.references("${host}/${missing}", &c);
        assert_eq!(refs.len(), 2);
        let host = refs.iter().find(|x| x.name == "host").unwrap();
        assert!(host.resolved);
        assert_eq!(host.source.as_deref(), Some("Base"));
        assert!(!refs.iter().find(|x| x.name == "missing").unwrap().resolved);
    }

    #[test]
    fn variable_names_with_periods_are_literal_keys() {
        let (r, c) = ctx(vec![(
            "Base",
            json!({
                "api.key": "secret",
                "user": {"id": "1"},
                "a.b": {"c": "x"},
                "api": {"key": "nested"},
            }),
        )]);
        assert_eq!(
            r.render_str("${api.key}", &c, Mode::Throw).unwrap(),
            "secret"
        );
        assert_eq!(r.render_str("${ user.id }", &c, Mode::Throw).unwrap(), "1");
        assert_eq!(r.render_str("${a.b.c}", &c, Mode::Throw).unwrap(), "x");
        assert_eq!(c.lookup("api.key"), Some(&json!("secret")));
        assert_eq!(c.lookup("user.id"), Some(&json!("1")));
        assert_eq!(c.source_of("api.key"), Some("Base"));
        assert_eq!(c.source_of("user.id"), Some("Base"));
        let err = r.render_str("${missing.key}", &c, Mode::Throw).unwrap_err();
        assert_eq!(err, RenderError::Unresolved(vec!["missing.key".into()]));
    }

    #[test]
    fn dotted_name_extends_the_same_key_from_an_earlier_layer() {
        let (_, c) = ctx(vec![
            ("Base", json!({"api.key": "https://ex.test"})),
            ("Sub", json!({"api.key": "${api.key}/v2"})),
        ]);
        assert_eq!(c.lookup("api.key"), Some(&json!("https://ex.test/v2")));
    }

    #[test]
    fn a_dotted_name_in_a_json_body_is_substituted() {
        let (r, c) = ctx(vec![(
            "Base",
            json!({"api.key": "secret", "user": {"id": 7}}),
        )]);
        let body = "{\n  \"token\": \"${api.key}\",\n  \"id\": ${user.id}\n}";
        assert_eq!(
            r.render_str(body, &c, Mode::Throw).unwrap(),
            "{\n  \"token\": \"secret\",\n  \"id\": 7\n}"
        );
    }
}
