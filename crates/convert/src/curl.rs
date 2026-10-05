//! Parse a `curl` command line into a Request (paste-to-import in the URL bar).

use base64::Engine as _;
use irs_core::{Auth, Body, BodyParam, KeyValue, Request, mime};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CurlError {
    #[error("not a curl command")]
    NotCurl,
    #[error("unterminated quote in command")]
    Quote,
    #[error("no URL found in curl command")]
    NoUrl,
}

/// POSIX-ish shell word splitting: single/double quotes, backslash escapes,
/// `\`-newline continuations, and `$'...'` ANSI-C strings (common in browser "Copy as cURL").
pub fn split_words(s: &str) -> Result<Vec<String>, CurlError> {
    let mut words = vec![];
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\n') | Some('\r') => {}
                Some(n) => {
                    cur.push(n);
                    in_word = true;
                }
                None => {}
            },
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err(CurlError::Quote),
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => cur.push('\n'),
                            Some('t') => cur.push('\t'),
                            Some('r') => cur.push('\r'),
                            Some(x) => cur.push(x),
                            None => return Err(CurlError::Quote),
                        },
                        Some(x) => cur.push(x),
                        None => return Err(CurlError::Quote),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(x @ ('"' | '\\' | '$' | '`')) => cur.push(x),
                            Some('\n') => {}
                            Some(x) => {
                                cur.push('\\');
                                cur.push(x);
                            }
                            None => return Err(CurlError::Quote),
                        },
                        Some(x) => cur.push(x),
                        None => return Err(CurlError::Quote),
                    }
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

pub fn looks_like_curl(s: &str) -> bool {
    s.trim_start().starts_with("curl ")
}

pub fn parse(cmd: &str) -> Result<Request, CurlError> {
    let words = split_words(cmd.trim())?;
    let mut it = words.into_iter().peekable();
    if it.next().as_deref() != Some("curl") {
        return Err(CurlError::NotCurl);
    }
    let mut req = Request {
        name: String::new(),
        ..Default::default()
    };
    let mut url: Option<String> = None;
    let mut method: Option<String> = None;
    let mut data: Vec<String> = vec![];
    let mut form: Vec<BodyParam> = vec![];
    let mut get_mode = false;
    let mut head = false;

    while let Some(w) = it.next() {
        // --flag=value form
        let (flag, inline) = match w.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (w.clone(), None),
        };
        let mut val = || inline.clone().or_else(|| it.next()).unwrap_or_default();
        match flag.as_str() {
            "-X" | "--request" => method = Some(val().to_uppercase()),
            "-H" | "--header" => {
                let h = val();
                if let Some((k, v)) = h.split_once(':') {
                    req.headers.push(KeyValue::new(k.trim(), v.trim()));
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--json" => {
                if flag == "--json" {
                    req.headers.push(KeyValue::new("Content-Type", mime::JSON));
                    req.headers.push(KeyValue::new("Accept", mime::JSON));
                }
                data.push(val());
            }
            "--data-urlencode" => {
                let v = val();
                let enc = match v.split_once('=') {
                    Some((k, x)) => format!("{k}={}", url_encode(x)),
                    None => url_encode(&v),
                };
                data.push(enc);
            }
            "-F" | "--form" | "--form-string" => {
                let v = val();
                if let Some((k, x)) = v.split_once('=') {
                    let p = match x.strip_prefix('@') {
                        Some(path) if flag != "--form-string" => BodyParam {
                            name: k.into(),
                            kind: Some("file".into()),
                            file_name: Some(path.split(';').next().unwrap_or(path).into()),
                            ..Default::default()
                        },
                        _ => BodyParam {
                            name: k.into(),
                            value: x.into(),
                            ..Default::default()
                        },
                    };
                    form.push(p);
                }
            }
            "-u" | "--user" => {
                let v = val();
                let (u, p) = v.split_once(':').unwrap_or((&v, ""));
                req.authentication = Auth::Basic {
                    username: u.into(),
                    password: p.into(),
                    disabled: false,
                };
            }
            "-b" | "--cookie" => req.headers.push(KeyValue::new("Cookie", val())),
            "-A" | "--user-agent" => req.headers.push(KeyValue::new("User-Agent", val())),
            "-e" | "--referer" => req.headers.push(KeyValue::new("Referer", val())),
            "--url" => url = Some(val()),
            "-G" | "--get" => get_mode = true,
            "-I" | "--head" => head = true,
            // flags with a value we ignore
            "-o" | "--output" | "-m" | "--max-time" | "--connect-timeout" | "-w"
            | "--write-out" | "--proxy" | "-x" | "--retry" | "-c" | "--cookie-jar" => {
                val();
            }
            f if f.starts_with('-') => {} // boolean flags: -L, -k, -s, -v, --compressed, ...
            _ => {
                if url.is_none() {
                    url = Some(w);
                }
            }
        }
    }

    let url = url.ok_or(CurlError::NoUrl)?;
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b.to_string(), Some(q.to_string())),
        None => (url.clone(), None),
    };
    req.url = base;
    if let Some(q) = query {
        for pair in q.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            req.parameters
                .push(KeyValue::new(url_decode(k), url_decode(v)));
        }
    }

    let content_type = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.split(';').next().unwrap_or("").trim().to_string());

    if get_mode && !data.is_empty() {
        for d in data
            .drain(..)
            .flat_map(|d| d.split('&').map(str::to_string).collect::<Vec<_>>())
        {
            let (k, v) = d.split_once('=').unwrap_or((&d, ""));
            req.parameters
                .push(KeyValue::new(url_decode(k), url_decode(v)));
        }
    }
    if !form.is_empty() {
        req.body = Body {
            mime_type: Some(mime::MULTIPART.into()),
            params: form,
            ..Default::default()
        };
        req.headers
            .retain(|h| !h.name.eq_ignore_ascii_case("content-type"));
    } else if !data.is_empty() {
        let text = data.join("&");
        let mime_type = match content_type.as_deref() {
            Some(mime::FORM) | None if !text.trim_start().starts_with(['{', '[']) => {
                mime::FORM.to_string()
            }
            None => mime::JSON.to_string(),
            Some(ct) => ct.to_string(),
        };
        if mime_type == mime::FORM {
            let params = text
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    BodyParam {
                        name: url_decode(k),
                        value: url_decode(v),
                        ..Default::default()
                    }
                })
                .collect();
            req.body = Body {
                mime_type: Some(mime_type),
                params,
                ..Default::default()
            };
        } else {
            req.body = Body {
                mime_type: Some(mime_type),
                text: Some(text),
                ..Default::default()
            };
        }
        req.headers
            .retain(|h| !h.name.eq_ignore_ascii_case("content-type"));
    }

    // Authorization: Bearer / Basic headers become first-class auth.
    if let Some(pos) = req
        .headers
        .iter()
        .position(|h| h.name.eq_ignore_ascii_case("authorization"))
    {
        let v = req.headers[pos].value.clone();
        if let Some(t) = v.strip_prefix("Bearer ") {
            req.authentication = Auth::Bearer {
                token: t.trim().into(),
                prefix: None,
                disabled: false,
            };
            req.headers.remove(pos);
        } else if let Some(b) = v.strip_prefix("Basic ")
            && let Ok(dec) = base64::engine::general_purpose::STANDARD.decode(b.trim())
            && let Ok(s) = String::from_utf8(dec)
        {
            let (u, p) = s.split_once(':').unwrap_or((&s, ""));
            req.authentication = Auth::Basic {
                username: u.into(),
                password: p.into(),
                disabled: false,
            };
            req.headers.remove(pos);
        }
    }

    req.method = match (method, head) {
        (Some(m), _) => m,
        (None, true) => "HEAD".into(),
        (None, false) if !data.is_empty() || !req.body.params.is_empty() => "POST".into(),
        _ => "GET".into(),
    };
    req.name = format!("{} {}", req.method, short_name(&req.url));
    Ok(req)
}

fn short_name(url: &str) -> String {
    let path = url.split("://").nth(1).unwrap_or(url);
    let path = path.split_once('/').map(|(_, p)| p).unwrap_or("");
    if path.is_empty() {
        "/".into()
    } else {
        format!("/{path}")
    }
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Import one or more `curl` commands (one per line, `\` continuations allowed)
/// into a workspace with one request each.
pub fn import_many(text: &str) -> crate::Result<crate::Imported> {
    let mut cmds: Vec<String> = vec![];
    for line in text.lines() {
        let continues = cmds
            .last()
            .is_some_and(|c: &String| c.trim_end().ends_with('\\'));
        if line.trim_start().starts_with("curl ") && !continues {
            cmds.push(line.to_string());
        } else if let Some(last) = cmds.last_mut() {
            last.push('\n');
            last.push_str(line);
        }
    }
    let mut warnings = vec![];
    let mut items = vec![];
    for (i, c) in cmds.iter().enumerate() {
        match parse(c) {
            Ok(mut r) => {
                if r.name.is_empty() {
                    r.name = request_name(&r.method, &r.url);
                }
                items.push(crate::Item::new(crate::Node::Request(r)));
            }
            Err(e) => warnings.push(format!("command {}: {e}", i + 1)),
        }
    }
    if items.is_empty() {
        return Err(crate::ConvertError::invalid(
            crate::Format::Curl,
            "no valid curl command found",
        ));
    }
    Ok(crate::Imported {
        format: crate::Format::Curl,
        workspaces: vec![crate::WorkspaceBundle {
            workspace: irs_core::Workspace {
                name: "cURL import".into(),
                ..Default::default()
            },
            items,
            ..Default::default()
        }],
        warnings,
    })
}

/// "GET /users/:id" style name from a method and URL.
pub fn request_name(method: &str, url: &str) -> String {
    let path = url::Url::parse(url)
        .map(|u| u.path().to_string())
        .unwrap_or_else(|_| url.split('?').next().unwrap_or(url).to_string());
    format!("{method} {}", if path.is_empty() { "/" } else { &path })
}

fn url_decode(s: &str) -> String {
    let bytes = s.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                16,
            )
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_copy_as_curl() {
        let cmd = r#"curl 'https://api.example.com/v1/items?page=2&q=a%20b' \
  -H 'accept: application/json' \
  -H 'authorization: Bearer tok123' \
  --data-raw $'{"name":"it\'s"}' \
  --compressed"#;
        let r = parse(cmd).unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.url, "https://api.example.com/v1/items");
        assert_eq!(
            r.parameters,
            vec![KeyValue::new("page", "2"), KeyValue::new("q", "a b")]
        );
        assert_eq!(r.headers, vec![KeyValue::new("accept", "application/json")]);
        assert_eq!(
            r.authentication,
            Auth::Bearer {
                token: "tok123".into(),
                prefix: None,
                disabled: false
            }
        );
        assert_eq!(r.body.mime_type.as_deref(), Some(mime::JSON));
        assert_eq!(r.body.text.as_deref(), Some(r#"{"name":"it's"}"#));
        assert_eq!(r.name, "POST /v1/items");
    }

    #[test]
    fn form_multipart_user_and_method() {
        let r =
            parse(r#"curl -X PUT -u alice:pw "http://x.io/up" -F "file=@/tmp/a.png" -F 'note=hi'"#)
                .unwrap();
        assert_eq!(r.method, "PUT");
        assert_eq!(
            r.authentication,
            Auth::Basic {
                username: "alice".into(),
                password: "pw".into(),
                disabled: false
            }
        );
        assert_eq!(r.body.mime_type.as_deref(), Some(mime::MULTIPART));
        assert_eq!(r.body.params[0].file_name.as_deref(), Some("/tmp/a.png"));
        assert_eq!(r.body.params[1].value, "hi");

        let r = parse("curl http://x.io/login -d user=a -d 'pass=b%26c'").unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.body.mime_type.as_deref(), Some(mime::FORM));
        assert_eq!(r.body.params[1].value, "b&c");

        let r = parse("curl -G http://x.io/s --data-urlencode 'q=a b'").unwrap();
        assert_eq!(
            (r.method.as_str(), r.parameters[0].value.as_str()),
            ("GET", "a b")
        );
    }

    #[test]
    fn errors() {
        assert_eq!(parse("wget http://x"), Err(CurlError::NotCurl));
        assert_eq!(parse("curl -H 'x: y'"), Err(CurlError::NoUrl));
        assert_eq!(parse("curl 'http://x"), Err(CurlError::Quote));
    }
}
