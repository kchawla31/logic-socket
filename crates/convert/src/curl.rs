//! Parse a `curl` command line into a Request (paste-to-import in the URL bar).

use base64::Engine as _;
use lsock_core::{Auth, Body, BodyParam, KeyValue, Request, mime};

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

/// True when `s` is a curl command someone would paste: `curl`, `CURL`, `curl.exe`,
/// a shell prompt (`$`, `%`, `❯`, `user@host $`), markdown fence, or BOM. A following
/// space or quote is required so `curl:` and `curling` are not commands.
pub fn looks_like_curl(s: &str) -> bool {
    curl_token_at_start(&prepare(s))
}

/// Drop paste chrome that is not part of the command.
///
/// One leading `#` is left in place. File detection already skips comment lines, and a
/// commented-out command should not start a new import. [`parse`] strips that `#` itself.
fn prepare(s: &str) -> String {
    let mut t = s.trim_start_matches('\u{feff}').trim().to_string();
    if let Some(inner) = strip_markdown_fence(&t) {
        t = inner;
    }
    t = join_windows_continuations(&t);
    strip_shell_prompt(t.trim())
}

fn strip_markdown_fence(s: &str) -> Option<String> {
    let rest = s.trim().strip_prefix("```")?;
    let nl = rest.find('\n')?;
    let body = &rest[nl + 1..];
    let end = body.rfind("```")?;
    Some(body[..end].trim().to_string())
}

/// Join cmd.exe `^` end-of-line continuations. An odd number of trailing carets means
/// the last one escapes the newline; that caret and the newline are removed.
fn join_windows_continuations(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let (line_with_nl, next) = match rest.find('\n') {
            Some(i) => (&rest[..=i], &rest[i + 1..]),
            None => (rest, ""),
        };
        rest = next;
        let has_nl = line_with_nl.ends_with('\n');
        let body = line_with_nl.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_end();
        let carets = trimmed.chars().rev().take_while(|c| *c == '^').count();
        if carets % 2 == 1 && has_nl {
            out.push_str(&trimmed[..trimmed.len() - '^'.len_utf8()]);
            continue;
        }
        out.push_str(body);
        if has_nl {
            out.push('\n');
        }
    }
    out
}

fn strip_shell_prompt(s: &str) -> String {
    let t = s.trim_start();
    if curl_token_at_start(t) {
        return t.to_string();
    }
    let first_len = t.find(['\n', '\r']).unwrap_or(t.len());
    if let Some(idx) = find_curl_token(&t[..first_len]) {
        let prefix = &t[..idx];
        if prefix_is_prompt(prefix) {
            return t[idx..].to_string();
        }
    }
    t.to_string()
}

/// A copied terminal line (`kapil@mac ~ %`, `(venv) $`, `❯`) in front of the command.
/// `#` is not a prompt here; a commented-out curl must not start a file import.
fn prefix_is_prompt(prefix: &str) -> bool {
    let p = prefix.trim();
    !p.is_empty()
        && p.len() <= 120
        && !p.contains("://")
        && p.chars()
            .any(|c| matches!(c, '$' | '%' | '>' | '❯' | '➜' | 'λ' | '»'))
}

fn find_curl_token(s: &str) -> Option<usize> {
    let mut prev: Option<char> = None;
    for (idx, ch) in s.char_indices() {
        let at_boundary = prev.is_none_or(|p| !is_curl_token_char(p));
        if at_boundary && ch.eq_ignore_ascii_case(&'c') && curl_token_at_start(&s[idx..]) {
            return Some(idx);
        }
        prev = Some(ch);
    }
    None
}

fn is_curl_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/')
}

fn strip_hash_prompt(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix('#') {
        let rest = rest.trim_start();
        if curl_token_at_start(rest) {
            return rest;
        }
    }
    t
}

fn curl_token_at_start(s: &str) -> bool {
    let s = s.trim_start();
    let rest = if has_ascii_prefix_ignore_case(s, "curl.exe") {
        &s["curl.exe".len()..]
    } else if has_ascii_prefix_ignore_case(s, "curl") {
        &s["curl".len()..]
    } else {
        return false;
    };
    matches!(rest.chars().next(), Some(c) if c.is_whitespace() || c == '\'' || c == '"')
}

fn has_ascii_prefix_ignore_case(s: &str, prefix: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn is_curl_argv0(w: &str) -> bool {
    w.eq_ignore_ascii_case("curl") || w.eq_ignore_ascii_case("curl.exe")
}

/// `curl'https://…'` is one word after [`split_words`] removes the quotes.
/// Split that into the argv0 and the URL (or a glued flag).
fn peel_glued_argv0(mut words: Vec<String>) -> Vec<String> {
    let split = {
        let Some(first) = words.first() else {
            return words;
        };
        if is_curl_argv0(first) {
            return words;
        }
        let (name, operand) = if has_ascii_prefix_ignore_case(first, "curl.exe") {
            ("curl.exe", &first["curl.exe".len()..])
        } else if has_ascii_prefix_ignore_case(first, "curl") {
            ("curl", &first["curl".len()..])
        } else {
            return words;
        };
        if !looks_like_glued_operand(operand) {
            return words;
        }
        (name.to_string(), operand.to_string())
    };
    let mut out = vec![split.0, split.1];
    out.extend(words.drain(1..));
    out
}

fn looks_like_glued_operand(rest: &str) -> bool {
    let Some(c) = rest.chars().next() else {
        return false;
    };
    if c == '-' || c == '/' || c == '{' {
        return true;
    }
    c.is_ascii_alphanumeric()
        && (rest.contains("://") || rest.contains('/') || rest.contains(':') || rest.contains('.'))
}

fn short_takes_value(c: char) -> bool {
    matches!(
        c,
        'X' | 'H'
            | 'd'
            | 'u'
            | 'F'
            | 'b'
            | 'A'
            | 'e'
            | 'o'
            | 'm'
            | 'w'
            | 'x'
            | 'c'
            | 'T'
            | 'E'
            | 'K'
            | 'P'
            | 'Q'
            | 'r'
            | 't'
            | 'y'
            | 'z'
    )
}

/// `-XPUT` and `-d{"a":1}` are one word after [`split_words`]. Split the value off, and
/// turn boolean clusters (`-sSL`, `-ks`) into one flag per letter.
fn expand_short_options(words: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    let mut words = words.into_iter();
    if let Some(argv0) = words.next() {
        out.push(argv0);
    }
    for w in words {
        if !w.starts_with('-') || w.starts_with("--") || w == "-" {
            out.push(w);
            continue;
        }
        let body: Vec<char> = w[1..].chars().collect();
        if body.len() <= 1 || !body[0].is_ascii_alphabetic() {
            out.push(w);
            continue;
        }
        let mut i = 0;
        while i < body.len() {
            let ch = body[i];
            if !ch.is_ascii_alphabetic() {
                out.push(body[i..].iter().collect());
                break;
            }
            out.push(format!("-{ch}"));
            if short_takes_value(ch) {
                let rest: String = body[i + 1..].iter().collect();
                if !rest.is_empty() {
                    out.push(rest);
                }
                break;
            }
            i += 1;
        }
    }
    out
}

pub fn parse(cmd: &str) -> Result<Request, CurlError> {
    let prepared = prepare(cmd);
    let words = split_words(strip_hash_prompt(&prepared))?;
    let words = expand_short_options(peel_glued_argv0(words));
    let mut it = words.into_iter().peekable();
    if !it.next().is_some_and(|w| is_curl_argv0(&w)) {
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
            | "--write-out" | "--proxy" | "-x" | "--retry" | "-c" | "--cookie-jar" | "-T"
            | "--upload-file" | "-E" | "--cert" | "-K" | "--config" | "-P" | "--ftp-port"
            | "-Q" | "--quote" | "-r" | "--range" | "-t" | "--telnet-option" | "-y"
            | "--speed-time" | "-z" | "--time-cond" => {
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
    let text = prepare(text);
    let mut cmds: Vec<String> = vec![];
    for line in text.lines() {
        let continues = cmds
            .last()
            .is_some_and(|c: &String| c.trim_end().ends_with('\\'));
        if looks_like_curl(line) && !continues {
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
            workspace: lsock_core::Workspace {
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

    /// Short options may carry the value in the same word (`-XPUT`, `-d'…'`).
    #[test]
    fn short_options_may_attach_their_value() {
        let put = parse("curl -XPUT https://ex.test/a").unwrap();
        let post = parse(r#"curl -d'{"a":1}' https://ex.test/a"#).unwrap();
        let header = parse("curl -H'K: v' https://ex.test/a").unwrap();
        let auth = parse("curl -uuser:pw https://ex.test/a").unwrap();
        assert_eq!(put.method, "PUT", "method {}", put.method);
        assert_eq!(post.method, "POST", "method {}", post.method);
        assert_eq!(post.body.text.as_deref(), Some(r#"{"a":1}"#), "{post:?}");
        assert!(
            header
                .headers
                .iter()
                .any(|h| h.name == "K" && h.value == "v"),
            "{header:?}"
        );
        assert!(
            matches!(auth.authentication, Auth::Basic { ref username, ref password, .. } if username == "user" && password == "pw"),
            "{auth:?}"
        );
    }

    #[test]
    fn errors() {
        assert_eq!(parse("wget http://x"), Err(CurlError::NotCurl));
        assert_eq!(parse("curl -H 'x: y'"), Err(CurlError::NoUrl));
        assert_eq!(parse("curl 'http://x"), Err(CurlError::Quote));
    }

    #[test]
    fn pasted_prompts_case_and_curl_exe() {
        let prompted = parse("$ curl https://ex.test/a \\\n  -H 'Accept: text/plain'").unwrap();
        assert_eq!(prompted.url, "https://ex.test/a");
        assert_eq!(
            prompted.headers,
            vec![KeyValue::new("Accept", "text/plain")]
        );
        assert_eq!(
            parse("> curl https://ex.test/b").unwrap().url,
            "https://ex.test/b"
        );
        assert_eq!(
            parse("% curl https://ex.test/c").unwrap().url,
            "https://ex.test/c"
        );
        assert_eq!(
            parse("# curl https://ex.test/d").unwrap().url,
            "https://ex.test/d"
        );
        assert_eq!(
            parse("kapil@mac ~ % curl https://ex.test/host")
                .unwrap()
                .url,
            "https://ex.test/host"
        );
        assert_eq!(
            parse("❯ curl https://ex.test/star").unwrap().url,
            "https://ex.test/star"
        );
        assert_eq!(
            parse("(venv) $ curl https://ex.test/venv").unwrap().url,
            "https://ex.test/venv"
        );

        let put = parse("curl.exe -XPUT https://ex.test/a").unwrap();
        assert_eq!(
            (put.method.as_str(), put.url.as_str()),
            ("PUT", "https://ex.test/a")
        );
        assert_eq!(parse("CURL https://ex.test/a").unwrap().method, "GET");
        assert_eq!(
            parse("curl'https://ex.test/q'").unwrap().url,
            "https://ex.test/q"
        );
        assert_eq!(
            parse("curl\"https://ex.test/q\"").unwrap().url,
            "https://ex.test/q"
        );
        assert_eq!(
            parse("\u{feff}$ curl https://ex.test/bom").unwrap().url,
            "https://ex.test/bom"
        );
        let fenced = parse("```bash\ncurl -XPUT https://ex.test/fence\n```").unwrap();
        assert_eq!(fenced.method, "PUT");
        assert_eq!(fenced.url, "https://ex.test/fence");

        let quiet = parse("curl -sSL https://ex.test/a").unwrap();
        assert_eq!(
            (quiet.method.as_str(), quiet.url.as_str()),
            ("GET", "https://ex.test/a")
        );

        assert!(!looks_like_curl("wget https://ex.test"));
        assert!(!looks_like_curl("https://curl.se"));
        assert!(!looks_like_curl("curling https://ex.test"));
        assert!(!looks_like_curl("curl: https://ex.test"));
        assert!(!looks_like_curl("curl"));
        assert!(!looks_like_curl("# curl https://ex.test"));
        assert!(!looks_like_curl("https://example.com/curl https://ex.test"));
        assert!(looks_like_curl("$ curl https://ex.test"));
        assert!(looks_like_curl("kapil@mac ~ % curl https://ex.test"));
        assert!(looks_like_curl("CURL.EXE -XPUT https://ex.test"));
    }

    #[test]
    fn windows_caret_continuation() {
        let cmd =
            "curl -X POST ^\r\n  https://ex.test/a ^\r\n  -H \"Accept: application/json\"\r\n";
        let r = parse(cmd).unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.url, "https://ex.test/a");
        assert_eq!(r.headers, vec![KeyValue::new("Accept", "application/json")]);
        // A caret that is not escaping a newline stays in the value.
        let kept = parse("curl -H \"X-A: a^b\" https://ex.test/a").unwrap();
        assert_eq!(kept.headers, vec![KeyValue::new("X-A", "a^b")]);
    }

    #[test]
    fn import_many_accepts_prompt_and_curl_exe_lines() {
        let text = "$ curl https://ex.test/a\nCURL.exe -XPUT https://ex.test/b\n";
        let imported = import_many(text).unwrap();
        let reqs: Vec<_> = imported.workspaces[0]
            .items
            .iter()
            .filter_map(|item| match &item.node {
                crate::Node::Request(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].url, "https://ex.test/a");
        assert_eq!(reqs[1].method, "PUT");
        assert_eq!(reqs[1].url, "https://ex.test/b");
    }
}
