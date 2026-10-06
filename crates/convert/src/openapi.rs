//! OpenAPI 3.x and Swagger 2.0 → a collection: one request per operation,
//! folders per tag, `base_url` and credential placeholders in the base environment.

use indexmap::IndexMap;
use lsock_core::{
    Auth, Body, BodyParam, Environment, Folder, KeyValue, OAuth2Config, Request, Workspace, mime,
};
use serde_json::{Map, Value, json};

use crate::{Entry, Format, Imported, Item, Node, Result, WorkspaceBundle, scalar, str_of};

const METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

struct Ctx<'a> {
    root: &'a Value,
    env: IndexMap<String, Value>,
    warnings: Vec<String>,
    v2: bool,
}

impl Ctx<'_> {
    /// Follow local `$ref`s (`#/components/...`, `#/definitions/...`).
    fn deref(&self, v: &Value) -> Value {
        let mut cur = v.clone();
        for _ in 0..16 {
            let Some(r) = cur.get("$ref").and_then(Value::as_str) else {
                return cur;
            };
            let Some(path) = r.strip_prefix("#/") else {
                return cur;
            };
            let mut node = self.root;
            for seg in path.split('/') {
                let seg = seg.replace("~1", "/").replace("~0", "~");
                match node.get(&seg) {
                    Some(n) => node = n,
                    None => return Value::Null,
                }
            }
            cur = node.clone();
        }
        cur
    }

    fn warn(&mut self, w: String) {
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    fn placeholder(&mut self, key: &str, default: &str) -> String {
        self.env
            .entry(key.to_string())
            .or_insert_with(|| json!(default));
        format!("{{{{ _.{key} }}}}")
    }

    /// An example value for a schema: explicit examples first, then a generated one.
    fn example(&self, schema: &Value, depth: usize) -> Value {
        let s = self.deref(schema);
        if depth > 6 {
            return Value::Null;
        }
        if let Some(e) = s.get("example") {
            return e.clone();
        }
        if let Some(e) = s.get("default") {
            return e.clone();
        }
        if let Some(e) = s
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
        {
            return e.clone();
        }
        if let Some(all) = s.get("allOf").and_then(Value::as_array) {
            let mut o = Map::new();
            for part in all {
                if let Value::Object(m) = self.example(part, depth + 1) {
                    o.extend(m);
                }
            }
            return Value::Object(o);
        }
        for k in ["oneOf", "anyOf"] {
            if let Some(first) = s.get(k).and_then(Value::as_array).and_then(|a| a.first()) {
                return self.example(first, depth + 1);
            }
        }
        let ty = match s.get("type") {
            Some(Value::String(t)) => t.clone(),
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .find(|t| *t != "null")
                .unwrap_or("string")
                .to_string(),
            _ if s.get("properties").is_some() => "object".into(),
            _ => String::new(),
        };
        match ty.as_str() {
            "object" => {
                let mut o = Map::new();
                if let Some(props) = s.get("properties").and_then(Value::as_object) {
                    for (k, p) in props {
                        if self.deref(p).get("readOnly").and_then(Value::as_bool) == Some(true) {
                            continue;
                        }
                        o.insert(k.clone(), self.example(p, depth + 1));
                    }
                }
                Value::Object(o)
            }
            "array" => json!([self.example(s.get("items").unwrap_or(&Value::Null), depth + 1)]),
            "integer" => json!(0),
            "number" => json!(0.0),
            "boolean" => json!(true),
            "string" => json!(match str_of(&s, "format").as_str() {
                "date-time" => "2024-01-01T00:00:00Z",
                "date" => "2024-01-01",
                "email" => "user@example.com",
                "uuid" => "00000000-0000-0000-0000-000000000000",
                "uri" | "url" => "https://example.com",
                "binary" | "byte" => "",
                _ => "string",
            }),
            _ => Value::Null,
        }
    }

    fn security_auth(&mut self, req: &[Value]) -> Auth {
        let schemes = if self.v2 {
            self.root.get("securityDefinitions")
        } else {
            self.root
                .get("components")
                .and_then(|c| c.get("securitySchemes"))
        };
        let Some(first) = req
            .iter()
            .find_map(|r| r.as_object().and_then(|o| o.keys().next().cloned()))
        else {
            return Auth::Inherit;
        };
        let Some(s) = schemes.and_then(|s| s.get(&first)).map(|s| self.deref(s)) else {
            return Auth::Inherit;
        };
        let ty = str_of(&s, "type");
        match (ty.as_str(), str_of(&s, "scheme").to_lowercase().as_str()) {
            ("basic", _) | ("http", "basic") => Auth::Basic {
                username: self.placeholder("http_username", "username"),
                password: self.placeholder("http_password", "password"),
                disabled: false,
            },
            ("http", "bearer") => Auth::Bearer {
                token: self.placeholder("bearer_token", ""),
                prefix: None,
                disabled: false,
            },
            ("http", "digest") => Auth::Digest {
                username: self.placeholder("http_username", "username"),
                password: self.placeholder("http_password", "password"),
                disabled: false,
            },
            ("apiKey", _) => {
                let var = format!(
                    "api_key_{}",
                    first
                        .to_lowercase()
                        .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
                );
                Auth::ApiKey {
                    key: str_of(&s, "name"),
                    value: self.placeholder(&var, ""),
                    add_to: Some(match str_of(&s, "in").as_str() {
                        "query" => "queryParams".into(),
                        "cookie" => "cookie".into(),
                        _ => "header".into(),
                    }),
                    disabled: false,
                }
            }
            ("oauth2", _) => {
                let scopes: Vec<String> = req
                    .iter()
                    .find_map(|r| r.get(&first))
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(scalar).collect())
                    .unwrap_or_default();
                let (grant, flow) = if self.v2 {
                    let g = match str_of(&s, "flow").as_str() {
                        "application" => "client_credentials",
                        "password" => "password",
                        _ => "authorization_code",
                    };
                    (g, s.clone())
                } else {
                    let flows = s.get("flows").cloned().unwrap_or(Value::Null);
                    if let Some(f) = flows.get("authorizationCode") {
                        ("authorization_code", f.clone())
                    } else if let Some(f) = flows.get("clientCredentials") {
                        ("client_credentials", f.clone())
                    } else if let Some(f) = flows.get("password") {
                        ("password", f.clone())
                    } else {
                        self.warn(format!("security scheme '{first}': OAuth 2 implicit flow isn't supported; requests use authorization code"));
                        (
                            "authorization_code",
                            flows.get("implicit").cloned().unwrap_or(Value::Null),
                        )
                    }
                };
                let mut c = OAuth2Config {
                    grant_type: grant.into(),
                    access_token_url: str_of(&flow, "tokenUrl"),
                    authorization_url: str_of(&flow, "authorizationUrl"),
                    client_id: self.placeholder("oauth2_client_id", ""),
                    client_secret: self.placeholder("oauth2_client_secret", ""),
                    scope: scopes.join(" "),
                    ..Default::default()
                };
                if grant == "password" {
                    c.username = self.placeholder("oauth2_username", "");
                    c.password = self.placeholder("oauth2_password", "");
                }
                Auth::OAuth2(c)
            }
            ("openIdConnect", _) => {
                self.warn(format!("security scheme '{first}': OpenID Connect discovery isn't automatic; fill in the OAuth 2 URLs"));
                Auth::OAuth2(OAuth2Config {
                    grant_type: "authorization_code".into(),
                    client_id: self.placeholder("oauth2_client_id", ""),
                    ..Default::default()
                })
            }
            _ => Auth::Inherit,
        }
    }
}

fn base_url(ctx: &mut Ctx) -> String {
    if ctx.v2 {
        let scheme = ctx
            .root
            .get("schemes")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .map(scalar)
            .unwrap_or_else(|| "https".into());
        let host = str_of(ctx.root, "host");
        let base = str_of(ctx.root, "basePath");
        return if host.is_empty() {
            base
        } else {
            format!("{scheme}://{host}{}", base.trim_end_matches('/'))
        };
    }
    let Some(server) = ctx
        .root
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
    else {
        return "http://localhost".into();
    };
    let mut url = str_of(&server, "url");
    if let Some(vars) = server.get("variables").and_then(Value::as_object) {
        for (k, v) in vars {
            let d = str_of(v, "default");
            url = url.replace(&format!("{{{k}}}"), &ctx.placeholder(k, &d));
        }
    }
    url.trim_end_matches('/').to_string()
}

fn params_of(ctx: &Ctx, path_item: &Value, op: &Value) -> Vec<Value> {
    // operation parameters override path-level ones with the same name+in
    let mut list: Vec<Value> = vec![];
    for p in path_item
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            op.get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
    {
        let p = ctx.deref(p);
        list.retain(|x| {
            !(str_of(x, "name") == str_of(&p, "name") && str_of(x, "in") == str_of(&p, "in"))
        });
        list.push(p);
    }
    list
}

fn param_value(ctx: &Ctx, p: &Value) -> String {
    if let Some(e) = p.get("example") {
        return scalar(e);
    }
    let schema = if ctx.v2 {
        p.clone()
    } else {
        p.get("schema").cloned().unwrap_or(Value::Null)
    };
    match ctx.example(&schema, 0) {
        Value::Null => String::new(),
        Value::String(s) if s == "string" => String::new(),
        v => scalar(&v),
    }
}

fn body_from_schema(ctx: &Ctx, content_type: &str, media: &Value, schema: &Value) -> Body {
    if content_type == mime::FORM || content_type == mime::MULTIPART {
        let s = ctx.deref(schema);
        let params = s
            .get("properties")
            .and_then(Value::as_object)
            .map(|props| {
                props
                    .iter()
                    .map(|(k, p)| {
                        let p = ctx.deref(p);
                        let file = str_of(&p, "format") == "binary" || str_of(&p, "type") == "file";
                        BodyParam {
                            name: k.clone(),
                            value: if file {
                                String::new()
                            } else {
                                match ctx.example(&p, 1) {
                                    Value::String(s) if s == "string" => String::new(),
                                    v => scalar(&v),
                                }
                            },
                            kind: (content_type == mime::MULTIPART)
                                .then(|| if file { "file".into() } else { "text".into() }),
                            ..Default::default()
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Body {
            mime_type: Some(content_type.into()),
            params,
            ..Default::default()
        };
    }
    let example = media
        .get("example")
        .cloned()
        .or_else(|| {
            media
                .get("examples")
                .and_then(Value::as_object)
                .and_then(|e| e.values().next())
                .map(|e| ctx.deref(e).get("value").cloned().unwrap_or(Value::Null))
        })
        .unwrap_or_else(|| ctx.example(schema, 0));
    let text = if content_type.contains("json") {
        serde_json::to_string_pretty(&example).unwrap_or_default()
    } else {
        scalar(&example)
    };
    Body {
        mime_type: Some(content_type.into()),
        text: Some(text),
        ..Default::default()
    }
}

fn operation(ctx: &mut Ctx, path: &str, method: &str, path_item: &Value, op: &Value) -> Request {
    let name = [str_of(op, "summary"), str_of(op, "operationId")]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{} {path}", method.to_uppercase()));
    // `{id}` → `:id` so it shows up as an editable path parameter
    let mut url_path = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let n: String = chars.by_ref().take_while(|c| *c != '}').collect();
            url_path.push(':');
            url_path.push_str(&n);
        } else {
            url_path.push(c);
        }
    }
    let mut r = Request {
        name,
        description: [str_of(op, "description"), str_of(op, "summary")]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default(),
        method: method.to_uppercase(),
        url: format!("{{{{ _.base_url }}}}{url_path}"),
        ..Default::default()
    };
    for p in params_of(ctx, path_item, op) {
        let n = str_of(&p, "name");
        let required = p.get("required").and_then(Value::as_bool).unwrap_or(false);
        match str_of(&p, "in").as_str() {
            "path" => r
                .path_parameters
                .push(KeyValue::new(n, param_value(ctx, &p))),
            "query" => r.parameters.push(KeyValue {
                name: n,
                value: param_value(ctx, &p),
                description: Some(str_of(&p, "description")).filter(|d| !d.is_empty()),
                disabled: !required,
                ..Default::default()
            }),
            "header" => r.headers.push(KeyValue {
                name: n,
                value: param_value(ctx, &p),
                disabled: !required,
                ..Default::default()
            }),
            "body" if ctx.v2 => {
                let ct = op
                    .get("consumes")
                    .or(ctx.root.get("consumes"))
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .map(scalar)
                    .unwrap_or_else(|| mime::JSON.into());
                r.body = body_from_schema(
                    ctx,
                    &ct,
                    &Value::Null,
                    p.get("schema").unwrap_or(&Value::Null),
                );
            }
            "formData" if ctx.v2 => {
                let multipart = p.get("type").and_then(Value::as_str) == Some("file")
                    || op
                        .get("consumes")
                        .and_then(Value::as_array)
                        .is_some_and(|a| a.iter().any(|c| c.as_str() == Some(mime::MULTIPART)));
                let mt = if multipart {
                    mime::MULTIPART
                } else {
                    mime::FORM
                };
                r.body.mime_type = Some(mt.into());
                let file = str_of(&p, "type") == "file";
                r.body.params.push(BodyParam {
                    name: n,
                    value: if file {
                        String::new()
                    } else {
                        param_value(ctx, &p)
                    },
                    kind: multipart.then(|| if file { "file".into() } else { "text".into() }),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }
    if !ctx.v2
        && let Some(rb) = op.get("requestBody").map(|b| ctx.deref(b))
        && let Some(content) = rb.get("content").and_then(Value::as_object)
    {
        let pick = [mime::JSON, mime::FORM, mime::MULTIPART]
            .iter()
            .find_map(|m| content.get_key_value(*m))
            .or_else(|| content.iter().find(|(k, _)| k.contains("json")))
            .or_else(|| content.iter().next());
        if let Some((ct, media)) = pick {
            let ct = if ct == "*/*" { mime::JSON } else { ct.as_str() };
            let schema = media.get("schema").cloned().unwrap_or(Value::Null);
            r.body = body_from_schema(ctx, ct, media, &schema);
        }
    }
    if let Some(ct) = r.body.mime_type.clone().filter(|m| m != mime::MULTIPART)
        && !r
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("content-type"))
    {
        r.headers.insert(0, KeyValue::new("Content-Type", ct));
    }
    let security = op
        .get("security")
        .or(ctx.root.get("security"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    r.authentication = if op
        .get("security")
        .is_some_and(|s| s.as_array().is_some_and(Vec::is_empty))
    {
        Auth::None
    } else {
        ctx.security_auth(&security)
    };
    r
}

pub fn import(v: &Value, text: &str, format: Format) -> Result<Imported> {
    let mut ctx = Ctx {
        root: v,
        env: IndexMap::new(),
        warnings: vec![],
        v2: format == Format::Swagger2,
    };
    let base = base_url(&mut ctx);
    let mut env = IndexMap::new();
    env.insert("base_url".to_string(), json!(base));
    let info = v.get("info").cloned().unwrap_or(Value::Null);
    let mut root: Vec<Item> = vec![];
    let mut folders: IndexMap<String, Vec<Item>> = IndexMap::new();
    if let Some(paths) = v.get("paths").and_then(Value::as_object) {
        for (path, item) in paths {
            let item = ctx.deref(item);
            for m in METHODS {
                let Some(op) = item.get(*m) else { continue };
                let r = operation(&mut ctx, path, m, &item, op);
                let it = Item::new(Node::Request(r));
                match op
                    .get("tags")
                    .and_then(Value::as_array)
                    .and_then(|t| t.first())
                    .map(scalar)
                {
                    Some(tag) => folders.entry(tag).or_default().push(it),
                    None => root.push(it),
                }
            }
        }
    }
    let tag_desc = |t: &str| -> String {
        v.get("tags")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|x| str_of(x, "name") == t))
            .map(|x| str_of(x, "description"))
            .unwrap_or_default()
    };
    let mut items: Vec<Item> = folders
        .into_iter()
        .map(|(tag, children)| {
            Item::new(Node::Folder(
                Folder {
                    description: tag_desc(&tag),
                    name: tag,
                    ..Default::default()
                },
                children,
            ))
        })
        .collect();
    items.extend(root);
    env.extend(ctx.env);
    let title = Some(str_of(&info, "title"))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Imported API".into());
    let version = str_of(&info, "version");
    let b = WorkspaceBundle {
        workspace: Workspace {
            name: if version.is_empty() {
                title
            } else {
                format!("{title} {version}")
            },
            description: str_of(&info, "description"),
            ..Default::default()
        },
        base_env: Some(Entry::new(Environment {
            name: "Base Environment".into(),
            data: env,
            ..Default::default()
        })),
        items,
        spec: Some(text.to_string()),
        ..Default::default()
    };
    Ok(Imported {
        format,
        workspaces: vec![b],
        warnings: ctx.warnings,
    })
}
