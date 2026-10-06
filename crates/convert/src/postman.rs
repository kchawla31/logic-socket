//! Postman collections (v2.0 / v2.1, import + v2.1 export) and Postman environments.

use std::sync::LazyLock;

use indexmap::IndexMap;
use lsock_core::{
    Auth, AwsIamConfig, Body, BodyParam, Environment, Folder, KeyValue, OAuth1Config, OAuth2Config,
    Request, Workspace, WorkspaceScope, mime,
};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{
    ConvertError, Entry, Format, Imported, Item, Node, Result, WorkspaceBundle, scalar, str_of,
};

pub const SCHEMA_V21: &str = "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";

// ------------------------------------------------------------------ variables

static VAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").unwrap());

/// Postman `{{var}}` → our template syntax. Dynamic variables (`{{$guid}}`) become
/// faker tags; names that aren't identifiers use bracket notation.
pub fn vars_in(s: &str) -> String {
    if !s.contains("{{") {
        return s.to_string();
    }
    VAR.replace_all(s, |c: &regex::Captures| {
        let name = &c[1];
        if let Some(dynamic) = name.strip_prefix('$') {
            return match dynamic {
                "guid" | "randomUUID" => "{% uuid 'v4' %}".to_string(),
                "timestamp" => "{% now 'unix' %}".to_string(),
                "isoTimestamp" => "{% now 'iso-8601' %}".to_string(),
                other => format!("{{% faker '{other}' %}}"),
            };
        }
        if name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            format!("{{{{ {name} }}}}")
        } else {
            format!("{{{{ _['{}'] }}}}", name.replace('\'', "\\'"))
        }
    })
    .into_owned()
}

static OUR_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\{\{\s*(?:_\.([A-Za-z0-9_]+)|_\['([^']+)'\]|([A-Za-z0-9_]+))\s*\}\}"#).unwrap()
});
static OUR_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\{%\s*(\w+)\s*([^%]*?)\s*%\}"#).unwrap());

/// Our template syntax → Postman `{{var}}` (best effort; unknown tags are left as-is).
pub fn vars_out(s: &str) -> String {
    let s = OUR_VAR.replace_all(s, |c: &regex::Captures| {
        let n = c
            .get(1)
            .or(c.get(2))
            .or(c.get(3))
            .map(|m| m.as_str())
            .unwrap_or("");
        format!("{{{{{n}}}}}")
    });
    OUR_TAG
        .replace_all(&s, |c: &regex::Captures| {
            let args = c[2].trim().trim_matches(|ch| ch == '\'' || ch == '"');
            match (&c[1], args) {
                ("uuid", _) => "{{$guid}}".to_string(),
                ("now", "unix") => "{{$timestamp}}".to_string(),
                ("now", "iso-8601") | ("now", "") => "{{$isoTimestamp}}".to_string(),
                ("faker", name) => format!("{{{{${name}}}}}"),
                _ => c[0].to_string(),
            }
        })
        .into_owned()
}

// ------------------------------------------------------------------ scripts

/// Rules from Insomnia's `translate-postman-script.ts`, in the same order. Rust's regex
/// has no look-behind, so the "not preceded by `.`/`$`/`-`/quote/word" guard is a
/// captured prefix group (`${p}`) that is written back.
static SCRIPT_RULES: LazyLock<Vec<(Regex, String)>> = LazyLock::new(|| {
    const P: &str = r#"(?P<p>^|[^.$\-"'\w])"#;
    // `p` is capture group 1, so the rule's own groups start at 2
    let r = |pat: &str, rep: &str| {
        (
            Regex::new(&format!("(?m){P}{pat}")).unwrap(),
            rep.replace("${2}", "${3}").replace("${1}", "${2}"),
        )
    };
    vec![
        r(
            r"tests\[(.*?)\]\s*=\s*(.*?);",
            "${p}pm.test(${1}, function() { pm.expect(${2}).to.be.true; });",
        ),
        r(
            r"globals\.(\w+?)\s*=\s*(.*?);",
            "${p}pm.globals.set('${1}', ${2});",
        ),
        r(
            r"globals\[(.+?)\]\s*=\s*(.*?);",
            "${p}pm.globals.set(${1}, ${2});",
        ),
        r(r"globals\.(\w+)\b", "${p}pm.globals.get('${1}')"),
        r(r"globals\[(.+?)\]", "${p}pm.globals.get(${1})"),
        r(
            r"environment\.(\w+?)\s*=\s*(.*?);",
            "${p}pm.environment.set('${1}', ${2});",
        ),
        r(
            r"environment\[(.+?)\]\s*=\s*(.*?);",
            "${p}pm.environment.set(${1}, ${2});",
        ),
        r(r"environment\.(\w+)\b", "${p}pm.environment.get('${1}')"),
        r(r"environment\[(.+?)\]", "${p}pm.environment.get(${1})"),
        r(r"responseTime\b", "${p}pm.response.responseTime"),
        r(
            r"responseHeaders\.(\w+)\b",
            "${p}pm.response.headers.get('${1}')",
        ),
        r(
            r"responseHeaders\[(.+?)\]",
            "${p}pm.response.headers.get(${1})",
        ),
        r(r"responseCode\.code\b", "${p}pm.response.code"),
        r(r"responseBody\b", "${p}pm.response.text()"),
        r(
            r"postman\.getEnvironmentVariable\((.*?)\)",
            "${p}pm.environment.get(${1})",
        ),
        r(
            r"postman\.setEnvironmentVariable\s*\(\s*(.+?)\s*,\s*(.+?)\s*\)",
            "${p}pm.environment.set(${1}, ${2})",
        ),
        r(
            r"postman\.clearEnvironmentVariable\s*\(\s*(.+?)\s*\)",
            "${p}pm.environment.unset(${1})",
        ),
        r(
            r"postman\.clearEnvironmentVariable\s*\(\s*\)",
            "${p}pm.environment.clear()",
        ),
        r(
            r"postman\.getGlobalVariable\((.*?)\)",
            "${p}pm.globals.get(${1})",
        ),
        r(
            r"postman\.setGlobalVariable\s*\(\s*(.+?)\s*,\s*(.+?)\s*\)",
            "${p}pm.globals.set(${1}, ${2})",
        ),
        r(
            r"postman\.clearGlobalVariable\s*\(\s*(.+?)\s*\)",
            "${p}pm.globals.unset(${1})",
        ),
        r(
            r"postman\.clearGlobalVariable\s*\(\s*\)",
            "${p}pm.globals.clear()",
        ),
        r(
            r"postman\.setNextRequest\b",
            "${p}pm.execution.setNextRequest",
        ),
        r(
            r"postman\.getResponseCookie\((.*?)\)\.value",
            "${p}pm.cookies.get(${1})",
        ),
        r(
            r"postman\.getResponseHeader\((.*?)\)",
            "${p}pm.response.headers.get(${1})",
        ),
        r(r"pm\.", "${p}insomnia."),
    ]
});

/// Translate a Postman script (legacy globals and `pm.*`) to the `insomnia.*` API.
pub fn translate_script(src: &str) -> String {
    let mut s = src.to_string();
    for (re, rep) in SCRIPT_RULES.iter() {
        s = re.replace_all(&s, rep.as_str()).into_owned();
    }
    s
}

static TO_PM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)(?P<p>^|[^.$\-"'\w])insomnia\."#).unwrap());

pub fn script_to_postman(src: &str) -> String {
    TO_PM.replace_all(src, "${p}pm.").into_owned()
}

fn script_of(events: &[Value], listen: &str) -> Option<String> {
    let e = events.iter().find(|e| str_of(e, "listen") == listen)?;
    let exec = e.get("script")?.get("exec")?;
    let src = match exec {
        Value::Array(lines) => lines.iter().map(scalar).collect::<Vec<_>>().join("\n"),
        other => scalar(other),
    };
    (!src.trim().is_empty()).then(|| translate_script(&src))
}

// ------------------------------------------------------------------ import

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn description(v: &Value) -> String {
    match v.get("description") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => o.get("content").map(scalar).unwrap_or_default(),
        _ => String::new(),
    }
}

/// v2.1 stores auth params as `[{key, value}]`, v2.0 as an object.
fn auth_params(auth: &Value, ty: &str) -> IndexMap<String, Value> {
    match auth.get(ty) {
        Some(Value::Array(list)) => list
            .iter()
            .map(|p| {
                (
                    str_of(p, "key"),
                    p.get("value").cloned().unwrap_or(Value::Null),
                )
            })
            .collect(),
        Some(Value::Object(o)) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => IndexMap::new(),
    }
}

fn auth_in(auth: Option<&Value>, warnings: &mut Vec<String>, at: &str) -> Auth {
    let Some(auth) = auth.filter(|a| a.is_object()) else {
        return Auth::Inherit;
    };
    let ty = str_of(auth, "type");
    let p = auth_params(auth, &ty);
    // some values (oauth2 audience/resource) are objects keyed by an internal id
    let g = |k: &str| {
        let v = match p.get(k) {
            Some(Value::Object(o)) => o.values().next().map(scalar),
            other => other.map(scalar),
        };
        vars_in(&v.unwrap_or_default())
    };
    let flag = |k: &str| {
        p.get(k)
            .is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true"))
    };
    match ty.as_str() {
        "noauth" => Auth::None,
        "inherit" | "" => Auth::Inherit,
        "basic" => Auth::Basic {
            username: g("username"),
            password: g("password"),
            disabled: false,
        },
        "bearer" => Auth::Bearer {
            token: g("token"),
            prefix: None,
            disabled: false,
        },
        "digest" => Auth::Digest {
            username: g("username"),
            password: g("password"),
            disabled: false,
        },
        "apikey" => Auth::ApiKey {
            key: g("key"),
            value: g("value"),
            add_to: Some(if g("in") == "query" {
                "queryParams".into()
            } else {
                "header".into()
            }),
            disabled: false,
        },
        "awsv4" => Auth::Iam(AwsIamConfig {
            access_key_id: g("accessKey"),
            secret_access_key: g("secretKey"),
            session_token: g("sessionToken"),
            region: g("region"),
            service: g("service"),
            disabled: false,
        }),
        "oauth1" => Auth::OAuth1(OAuth1Config {
            consumer_key: g("consumerKey"),
            consumer_secret: g("consumerSecret"),
            token_key: g("token"),
            token_secret: g("tokenSecret"),
            signature_method: Some(g("signatureMethod"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "HMAC-SHA1".into()),
            realm: g("realm"),
            callback: g("callback"),
            verifier: g("verifier"),
            nonce: g("nonce"),
            timestamp: g("timestamp"),
            include_body_hash: flag("includeBodyHash"),
            disabled: false,
        }),
        "oauth2" => {
            let grant = match g("grant_type").as_str() {
                "authorization_code_with_pkce" | "authorization_code" | "" => "authorization_code",
                "password_credentials" => "password",
                "client_credentials" => "client_credentials",
                "implicit" => {
                    warnings.push(format!("{at}: OAuth 2 implicit grant isn't supported; switched to authorization code"));
                    "authorization_code"
                }
                other => {
                    warnings.push(format!("{at}: unknown OAuth 2 grant '{other}'"));
                    "authorization_code"
                }
            };
            let d = OAuth2Config::default();
            Auth::OAuth2(OAuth2Config {
                grant_type: grant.into(),
                access_token_url: g("accessTokenUrl"),
                authorization_url: g("authUrl"),
                client_id: g("clientId"),
                client_secret: g("clientSecret"),
                scope: g("scope"),
                audience: g("audience"),
                resource: g("resource"),
                username: g("username"),
                password: g("password"),
                redirect_url: Some(g("redirect_uri"))
                    .filter(|s| {
                        s.starts_with("http://localhost") || s.starts_with("http://127.0.0.1")
                    })
                    .unwrap_or(d.redirect_url),
                use_pkce: g("grant_type") == "authorization_code_with_pkce",
                credentials_in_body: g("client_authentication") == "body",
                token_prefix: g("headerPrefix"),
                disabled: false,
            })
        }
        other => {
            warnings.push(format!(
                "{at}: {other} auth isn't supported yet; set to No auth"
            ));
            Auth::None
        }
    }
}

fn kv_list(list: &[Value], v20: bool) -> Vec<KeyValue> {
    list.iter()
        .filter(|q| q.get("key").is_some() || q.get("value").is_some())
        .map(|q| KeyValue {
            name: vars_in(&str_of(q, "key")),
            value: vars_in(&q.get("value").map(scalar).unwrap_or_default()),
            description: Some(description(q)).filter(|d| !d.is_empty()),
            disabled: if v20 && q.get("enabled").is_some() {
                q.get("enabled").and_then(Value::as_bool) == Some(false)
            } else {
                q.get("disabled").and_then(Value::as_bool).unwrap_or(false)
            },
            ..Default::default()
        })
        .collect()
}

fn url_in(url: Option<&Value>, v20: bool) -> (String, Vec<KeyValue>, Vec<KeyValue>) {
    match url {
        Some(Value::String(s)) => (vars_in(s), vec![], vec![]),
        Some(u @ Value::Object(_)) => {
            let mut raw = str_of(u, "raw");
            if raw.is_empty() {
                // rebuild from parts
                let host = arr(u, "host")
                    .iter()
                    .map(scalar)
                    .collect::<Vec<_>>()
                    .join(".");
                let path = arr(u, "path")
                    .iter()
                    .map(scalar)
                    .collect::<Vec<_>>()
                    .join("/");
                let proto = str_of(u, "protocol");
                raw = format!(
                    "{}{host}{}{}",
                    if proto.is_empty() {
                        String::new()
                    } else {
                        format!("{proto}://")
                    },
                    if path.is_empty() { "" } else { "/" },
                    path
                );
            }
            let query = arr(u, "query");
            if !query.is_empty()
                && let Some(i) = raw.find('?')
            {
                raw.truncate(i);
            }
            let path_params = arr(u, "variable")
                .iter()
                .map(|v| {
                    KeyValue::new(
                        vars_in(&str_of(v, "key")),
                        vars_in(&v.get("value").map(scalar).unwrap_or_default()),
                    )
                })
                .collect();
            (vars_in(&raw), kv_list(query, v20), path_params)
        }
        _ => (String::new(), vec![], vec![]),
    }
}

fn body_in(b: Option<&Value>, v20: bool) -> Body {
    let Some(b) =
        b.filter(|b| b.is_object() && b.get("disabled").and_then(Value::as_bool) != Some(true))
    else {
        return Body::default();
    };
    match str_of(b, "mode").as_str() {
        "raw" => {
            let raw = str_of(b, "raw");
            if raw.is_empty() {
                return Body::default();
            }
            let lang = b
                .get("options")
                .and_then(|o| o.get("raw"))
                .map(|r| str_of(r, "language"))
                .unwrap_or_default();
            let mime = match lang.as_str() {
                "json" => "application/json",
                "xml" => "application/xml",
                "html" => "text/html",
                "javascript" => "application/javascript",
                _ => "text/plain",
            };
            Body {
                mime_type: Some(mime.into()),
                text: Some(vars_in(&raw)),
                ..Default::default()
            }
        }
        "urlencoded" => Body {
            mime_type: Some(mime::FORM.into()),
            params: kv_list(arr(b, "urlencoded"), v20)
                .into_iter()
                .map(|kv| BodyParam {
                    name: kv.name,
                    value: kv.value,
                    disabled: kv.disabled,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        },
        "formdata" => Body {
            mime_type: Some(mime::MULTIPART.into()),
            params: arr(b, "formdata")
                .iter()
                .map(|p| {
                    let file = str_of(p, "type") == "file";
                    let disabled = if v20 && p.get("enabled").is_some() {
                        p.get("enabled").and_then(Value::as_bool) == Some(false)
                    } else {
                        p.get("disabled").and_then(Value::as_bool).unwrap_or(false)
                    };
                    BodyParam {
                        name: vars_in(&str_of(p, "key")),
                        value: if file {
                            String::new()
                        } else {
                            vars_in(&p.get("value").map(scalar).unwrap_or_default())
                        },
                        disabled,
                        kind: Some(if file { "file" } else { "text" }.into()),
                        file_name: if file {
                            match p.get("src") {
                                Some(Value::Array(a)) => a.first().map(scalar),
                                Some(v) => Some(scalar(v)).filter(|s| !s.is_empty()),
                                None => None,
                            }
                        } else {
                            None
                        },
                        ..Default::default()
                    }
                })
                .collect(),
            ..Default::default()
        },
        "file" => Body {
            mime_type: Some(mime::FILE.into()),
            file_name: b
                .get("file")
                .map(|f| str_of(f, "src"))
                .filter(|s| !s.is_empty()),
            ..Default::default()
        },
        "graphql" => {
            let g = b.get("graphql").cloned().unwrap_or(Value::Null);
            let vars = match g.get("variables") {
                Some(Value::String(s)) if !s.trim().is_empty() => {
                    serde_json::from_str(s).unwrap_or(Value::String(s.clone()))
                }
                Some(Value::Object(o)) => Value::Object(o.clone()),
                _ => json!({}),
            };
            let text = serde_json::to_string_pretty(
                &json!({ "query": str_of(&g, "query"), "variables": vars }),
            )
            .unwrap_or_default();
            Body {
                mime_type: Some(mime::GRAPHQL.into()),
                text: Some(vars_in(&text)),
                ..Default::default()
            }
        }
        _ => Body::default(),
    }
}

fn items_in(list: &[Value], v20: bool, warnings: &mut Vec<String>) -> Vec<Item> {
    let mut out = vec![];
    for it in list {
        let name = str_of(it, "name");
        let at = format!("'{name}'");
        let events = arr(it, "event");
        if it.get("item").is_some() || it.get("request").is_none() {
            let f = Folder {
                name,
                description: description(it),
                authentication: auth_in(it.get("auth"), warnings, &at),
                pre_request_script: script_of(events, "prerequest"),
                after_response_script: script_of(events, "test"),
                ..Default::default()
            };
            out.push(Item::new(Node::Folder(
                f,
                items_in(arr(it, "item"), v20, warnings),
            )));
            continue;
        }
        let req = it.get("request").cloned().unwrap_or(Value::Null);
        // a request may be just a URL string
        let req = if let Value::String(u) = &req {
            json!({"url": u, "method": "GET"})
        } else {
            req
        };
        let (url, parameters, path_parameters) = url_in(req.get("url"), v20);
        let mut headers = kv_list(arr(&req, "header"), v20);
        let body = body_in(req.get("body"), v20);
        if let Some(m) = &body.mime_type
            && !headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("content-type"))
            && m != mime::MULTIPART
            && m != mime::FORM
        {
            let ct = if m == mime::GRAPHQL {
                mime::JSON
            } else {
                m.as_str()
            };
            headers.push(KeyValue::new("Content-Type", ct));
        }
        let r = Request {
            name: if name.is_empty() {
                crate::curl::request_name(&str_of(&req, "method"), &url)
            } else {
                name
            },
            description: if description(it).is_empty() {
                description(&req)
            } else {
                description(it)
            },
            method: req
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .to_uppercase(),
            url,
            parameters,
            path_parameters,
            headers,
            body,
            authentication: auth_in(req.get("auth"), warnings, &at),
            pre_request_script: script_of(events, "prerequest"),
            after_response_script: script_of(events, "test"),
            ..Default::default()
        };
        out.push(Item::new(Node::Request(r)));
    }
    out
}

pub fn import_collection(v: &Value) -> Result<Imported> {
    let info = v
        .get("info")
        .ok_or_else(|| ConvertError::invalid(Format::PostmanCollection, "missing info"))?;
    let v20 = str_of(info, "schema").contains("v2.0.0");
    let mut warnings = vec![];
    let name = Some(str_of(info, "name"))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Postman collection".into());
    let mut items = items_in(arr(v, "item"), v20, &mut warnings);
    let auth = auth_in(v.get("auth"), &mut warnings, "collection");
    let events = arr(v, "event");
    let pre = script_of(events, "prerequest");
    let post = script_of(events, "test");
    // collection-level auth/scripts need a folder to live on (like Insomnia does)
    if !auth.is_inherit() || pre.is_some() || post.is_some() {
        let f = Folder {
            name: name.clone(),
            description: description(info),
            authentication: auth,
            pre_request_script: pre,
            after_response_script: post,
            ..Default::default()
        };
        items = vec![Item::new(Node::Folder(f, items))];
    }
    let data: IndexMap<String, Value> = arr(v, "variable")
        .iter()
        .filter(|x| x.get("key").is_some())
        .map(|x| {
            (
                str_of(x, "key"),
                Value::String(vars_in(&x.get("value").map(scalar).unwrap_or_default())),
            )
        })
        .collect();
    let b = WorkspaceBundle {
        workspace: Workspace {
            name,
            description: description(info),
            ..Default::default()
        },
        base_env: Some(Entry::new(Environment {
            name: "Base Environment".into(),
            data,
            ..Default::default()
        })),
        items,
        ..Default::default()
    };
    Ok(Imported {
        format: Format::PostmanCollection,
        workspaces: vec![b],
        warnings,
    })
}

/// A Postman environment becomes a global-environment workspace.
pub fn import_environment(v: &Value) -> Result<Imported> {
    let name = Some(str_of(v, "name"))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Postman Environment".into());
    let mut data = IndexMap::new();
    let mut secret_keys = vec![];
    for x in arr(v, "values") {
        if x.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let k = str_of(x, "key");
        if str_of(x, "type") == "secret" {
            secret_keys.push(k.clone());
        }
        data.insert(
            k,
            Value::String(vars_in(&x.get("value").map(scalar).unwrap_or_default())),
        );
    }
    let b = WorkspaceBundle {
        workspace: Workspace {
            name: name.clone(),
            scope: WorkspaceScope::Environment,
            ..Default::default()
        },
        base_env: Some(Entry::new(Environment {
            name: "Base Environment".into(),
            ..Default::default()
        })),
        sub_envs: vec![Entry::new(Environment {
            name,
            data,
            secret_keys,
            ..Default::default()
        })],
        ..Default::default()
    };
    Ok(Imported {
        format: Format::PostmanEnvironment,
        workspaces: vec![b],
        warnings: vec![],
    })
}

// ------------------------------------------------------------------ export

fn kv_out(list: &[KeyValue]) -> Vec<Value> {
    list.iter()
        .map(|h| {
            let mut o = Map::new();
            o.insert("key".into(), json!(vars_out(&h.name)));
            o.insert("value".into(), json!(vars_out(&h.value)));
            if h.disabled {
                o.insert("disabled".into(), json!(true));
            }
            if let Some(d) = &h.description {
                o.insert("description".into(), json!(d));
            }
            Value::Object(o)
        })
        .collect()
}

fn params(pairs: &[(&str, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(k, v)| json!({"key": k, "value": vars_out(v), "type": "string"}))
            .collect(),
    )
}

fn auth_out(a: &Auth) -> Option<Value> {
    Some(match a {
        Auth::Inherit => return None,
        Auth::None => json!({"type": "noauth"}),
        Auth::Basic {
            username, password, ..
        } => {
            json!({"type": "basic", "basic": params(&[("username", username.clone()), ("password", password.clone())])})
        }
        Auth::Bearer { token, .. } => {
            json!({"type": "bearer", "bearer": params(&[("token", token.clone())])})
        }
        Auth::Digest {
            username, password, ..
        } => {
            json!({"type": "digest", "digest": params(&[("username", username.clone()), ("password", password.clone())])})
        }
        Auth::ApiKey {
            key, value, add_to, ..
        } => json!({"type": "apikey", "apikey": params(&[
            ("key", key.clone()), ("value", value.clone()),
            ("in", if add_to.as_deref() == Some("queryParams") { "query".into() } else { "header".into() }),
        ])}),
        Auth::Iam(c) => json!({"type": "awsv4", "awsv4": params(&[
            ("accessKey", c.access_key_id.clone()), ("secretKey", c.secret_access_key.clone()),
            ("sessionToken", c.session_token.clone()), ("region", c.region.clone()), ("service", c.service.clone()),
        ])}),
        Auth::OAuth1(c) => json!({"type": "oauth1", "oauth1": params(&[
            ("consumerKey", c.consumer_key.clone()), ("consumerSecret", c.consumer_secret.clone()),
            ("token", c.token_key.clone()), ("tokenSecret", c.token_secret.clone()),
            ("signatureMethod", c.signature_method.clone()), ("realm", c.realm.clone()),
            ("callback", c.callback.clone()), ("verifier", c.verifier.clone()),
        ])}),
        Auth::OAuth2(c) => json!({"type": "oauth2", "oauth2": params(&[
            ("grant_type", match c.grant_type.as_str() {
                "password" => "password_credentials".into(),
                "authorization_code" if c.use_pkce => "authorization_code_with_pkce".into(),
                g => g.to_string(),
            }),
            ("accessTokenUrl", c.access_token_url.clone()), ("authUrl", c.authorization_url.clone()),
            ("clientId", c.client_id.clone()), ("clientSecret", c.client_secret.clone()),
            ("scope", c.scope.clone()), ("username", c.username.clone()), ("password", c.password.clone()),
            ("redirect_uri", c.redirect_url.clone()),
            ("client_authentication", if c.credentials_in_body { "body".into() } else { "header".into() }),
        ])}),
        Auth::Netrc { .. } => return None,
    })
}

fn body_out(b: &Body) -> Option<Value> {
    let m = b.mime_type.as_deref()?;
    Some(match m {
        mime::FORM => json!({"mode": "urlencoded", "urlencoded": b.params.iter().map(|p| json!({
            "key": vars_out(&p.name), "value": vars_out(&p.value), "disabled": p.disabled,
        })).collect::<Vec<_>>()}),
        mime::MULTIPART => {
            json!({"mode": "formdata", "formdata": b.params.iter().map(|p| if p.kind.as_deref() == Some("file") {
            json!({"key": vars_out(&p.name), "type": "file", "src": p.file_name, "disabled": p.disabled})
        } else {
            json!({"key": vars_out(&p.name), "value": vars_out(&p.value), "type": "text", "disabled": p.disabled})
        }).collect::<Vec<_>>()})
        }
        mime::FILE => json!({"mode": "file", "file": {"src": b.file_name}}),
        mime::GRAPHQL => {
            let g: Value =
                serde_json::from_str(b.text.as_deref().unwrap_or("{}")).unwrap_or(json!({}));
            json!({"mode": "graphql", "graphql": {
                "query": vars_out(&str_of(&g, "query")),
                "variables": g.get("variables").map(|v| serde_json::to_string_pretty(v).unwrap_or_default()).unwrap_or_default(),
            }})
        }
        other => {
            let lang = if other.contains("json") {
                "json"
            } else if other.contains("xml") {
                "xml"
            } else if other.contains("html") {
                "html"
            } else if other.contains("javascript") {
                "javascript"
            } else {
                "text"
            };
            json!({"mode": "raw", "raw": vars_out(b.text.as_deref().unwrap_or("")), "options": {"raw": {"language": lang}}})
        }
    })
}

fn events_out(pre: &Option<String>, post: &Option<String>) -> Vec<Value> {
    let mut ev = vec![];
    for (listen, src) in [("prerequest", pre), ("test", post)] {
        if let Some(s) = src.as_ref().filter(|s| !s.trim().is_empty()) {
            ev.push(json!({"listen": listen, "script": {"type": "text/javascript", "exec": script_to_postman(s).lines().collect::<Vec<_>>()}}));
        }
    }
    ev
}

fn url_out(r: &Request) -> Value {
    let raw = vars_out(&r.url);
    let mut full = raw.clone();
    let enabled: Vec<&KeyValue> = r.parameters.iter().filter(|p| !p.disabled).collect();
    if !enabled.is_empty() {
        full.push(if full.contains('?') { '&' } else { '?' });
        full.push_str(
            &enabled
                .iter()
                .map(|p| format!("{}={}", vars_out(&p.name), vars_out(&p.value)))
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    let (proto, rest) = raw
        .split_once("://")
        .map(|(p, r)| (Some(p), r))
        .unwrap_or((None, raw.as_str()));
    let rest = rest.split('?').next().unwrap_or(rest);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut o = Map::new();
    o.insert("raw".into(), json!(full));
    if let Some(p) = proto {
        o.insert("protocol".into(), json!(p));
    }
    o.insert("host".into(), json!(host.split('.').collect::<Vec<_>>()));
    if !path.is_empty() {
        o.insert("path".into(), json!(path.split('/').collect::<Vec<_>>()));
    }
    if !r.parameters.is_empty() {
        o.insert("query".into(), json!(kv_out(&r.parameters)));
    }
    if !r.path_parameters.is_empty() {
        o.insert("variable".into(), json!(kv_out(&r.path_parameters)));
    }
    Value::Object(o)
}

fn items_out(items: &[Item], skipped: &mut usize) -> Vec<Value> {
    let mut out = vec![];
    for it in items {
        match &it.node {
            Node::Folder(f, children) => {
                let mut o = json!({"name": f.name, "item": items_out(children, skipped)});
                let m = o.as_object_mut().unwrap();
                if !f.description.is_empty() {
                    m.insert("description".into(), json!(f.description));
                }
                if let Some(a) = auth_out(&f.authentication) {
                    m.insert("auth".into(), a);
                }
                let ev = events_out(&f.pre_request_script, &f.after_response_script);
                if !ev.is_empty() {
                    m.insert("event".into(), json!(ev));
                }
                out.push(o);
            }
            Node::Request(r) => {
                let mut req =
                    json!({"method": r.method, "header": kv_out(&r.headers), "url": url_out(r)});
                let m = req.as_object_mut().unwrap();
                if let Some(b) = body_out(&r.body) {
                    m.insert("body".into(), b);
                }
                if let Some(a) = auth_out(&r.authentication) {
                    m.insert("auth".into(), a);
                }
                if !r.description.is_empty() {
                    m.insert("description".into(), json!(r.description));
                }
                let mut o = json!({"name": r.name, "request": req});
                let ev = events_out(&r.pre_request_script, &r.after_response_script);
                if !ev.is_empty() {
                    o.as_object_mut().unwrap().insert("event".into(), json!(ev));
                }
                out.push(o);
            }
            _ => *skipped += 1,
        }
    }
    out
}

/// Export a workspace as a Postman v2.1 collection. Returns the JSON and how many
/// items had no Postman equivalent (gRPC, realtime, MCP, AI requests).
pub fn export_collection(b: &WorkspaceBundle) -> (String, usize) {
    let mut skipped = 0;
    let item = items_out(&b.items, &mut skipped);
    let mut variable = vec![];
    if let Some(base) = &b.base_env {
        for (k, v) in &base.body.data {
            variable.push(json!({"key": k, "value": vars_out(&scalar(v))}));
        }
    }
    let v = json!({
        "info": {
            "name": b.workspace.name,
            "description": b.workspace.description,
            "schema": SCHEMA_V21,
        },
        "item": item,
        "variable": variable,
    });
    (
        serde_json::to_string_pretty(&v).unwrap_or_default(),
        skipped,
    )
}

/// Export one environment as a Postman environment file.
pub fn export_environment(e: &Environment) -> String {
    let values: Vec<Value> = e
        .data
        .iter()
        .map(|(k, v)| {
            json!({
                "key": k,
                "value": vars_out(&scalar(v)),
                "type": if e.secret_keys.contains(k) { "secret" } else { "default" },
                "enabled": true,
            })
        })
        .collect();
    serde_json::to_string_pretty(
        &json!({"name": e.name, "values": values, "_postman_variable_scope": "environment"}),
    )
    .unwrap_or_default()
}
