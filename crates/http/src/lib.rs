//! HTTP execution: build a wire request from a (rendered) `irs_core::Request`,
//! send it with manual redirect handling, and capture timings + a timeline.

pub mod cookies;
pub mod sign;

use std::time::{Duration, Instant};

use base64::Engine as _;
use irs_core::{Auth, Body, Cookie, KeyValue, Request, TimelineEntry, Timings, mime};
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("invalid URL '{0}': {1}")]
    InvalidUrl(String, String),
    #[error("invalid method '{0}'")]
    InvalidMethod(String),
    #[error("could not read file '{0}': {1}")]
    File(String, String),
    #[error("request timed out after {0} ms")]
    Timeout(u64),
    #[error("too many redirects (max {0})")]
    TooManyRedirects(usize),
    #[error("{0}")]
    Network(String),
}

#[derive(Debug, Clone)]
pub struct Options {
    pub timeout: Duration,
    pub follow_redirects: bool,
    pub max_redirects: usize,
    pub validate_certificates: bool,
    pub send_cookies: bool,
    pub store_cookies: bool,
    pub user_agent: Option<String>,
    /// Cap on body bytes copied into the timeline.
    pub timeline_body_limit: usize,
    pub proxy: Option<String>,
    pub no_proxy: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            follow_redirects: true,
            max_redirects: 10,
            validate_certificates: true,
            send_cookies: true,
            store_cookies: true,
            user_agent: Some(concat!("insomnia-rs/", env!("CARGO_PKG_VERSION")).to_string()),
            timeline_body_limit: 10 * 1024,
            proxy: None,
            no_proxy: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpResponse {
    pub method: String,
    /// Final URL after redirects.
    pub url: String,
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    pub headers: Vec<KeyValue>,
    #[serde(skip)]
    pub body: Vec<u8>,
    pub content_type: String,
    pub timings: Timings,
    pub timeline: Vec<TimelineEntry>,
    pub redirects: usize,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Body ready for the wire.
#[derive(Debug, Clone)]
enum WireBody {
    None,
    Bytes {
        content_type: Option<String>,
        data: Vec<u8>,
    },
    Multipart(Vec<irs_core::BodyParam>),
}

/// Build the final URL: path params, query params, auth-in-query, scheme default.
pub fn build_url(req: &Request, encode: bool) -> Result<Url, HttpError> {
    let mut raw = req.url.trim().to_string();
    for p in &req.path_parameters {
        if !p.value.is_empty() {
            let needle = format!("/:{}", p.name);
            let enc = url_encode_component(&p.value);
            // replace only whole segments
            let mut out = String::new();
            let mut rest = raw.as_str();
            while let Some(i) = rest.find(&needle) {
                let end = i + needle.len();
                let boundary = rest[end..]
                    .chars()
                    .next()
                    .is_none_or(|c| matches!(c, '/' | '?' | '#'));
                out.push_str(&rest[..i]);
                if boundary {
                    out.push('/');
                    out.push_str(&enc);
                } else {
                    out.push_str(&needle);
                }
                rest = &rest[end..];
            }
            out.push_str(rest);
            raw = out;
        }
    }
    if !raw.contains("://") {
        raw = format!("http://{raw}");
    }
    let mut url =
        Url::parse(&raw).map_err(|e| HttpError::InvalidUrl(req.url.clone(), e.to_string()))?;
    let extra: Vec<&KeyValue> = req
        .parameters
        .iter()
        .filter(|p| !p.disabled && !p.name.is_empty())
        .collect();
    if !extra.is_empty() {
        if encode {
            let mut qp = url.query_pairs_mut();
            for p in extra {
                qp.append_pair(&p.name, &p.value);
            }
        } else {
            let mut q = url.query().map(str::to_string).unwrap_or_default();
            for p in extra {
                if !q.is_empty() {
                    q.push('&');
                }
                q.push_str(&format!("{}={}", p.name, p.value));
            }
            url.set_query(Some(&q));
        }
    }
    Ok(url)
}

fn url_encode_component(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// Headers to send (enabled, non-empty names) plus auth headers. Returns any
/// auth query parameters / cookies separately.
fn apply_auth(
    auth: &Auth,
    headers: &mut Vec<(String, String)>,
    url: &mut Url,
    cookie_extra: &mut Vec<String>,
) {
    let has =
        |hs: &Vec<(String, String)>, n: &str| hs.iter().any(|(k, _)| k.eq_ignore_ascii_case(n));
    match auth {
        Auth::Basic {
            username,
            password,
            disabled: false,
        } if !has(headers, "authorization") => {
            let token =
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
            headers.push(("Authorization".into(), format!("Basic {token}")));
        }
        Auth::Bearer {
            token,
            prefix,
            disabled: false,
        } if !has(headers, "authorization") && !token.is_empty() => {
            let prefix = prefix
                .as_deref()
                .filter(|p| !p.is_empty())
                .unwrap_or("Bearer");
            headers.push(("Authorization".into(), format!("{prefix} {token}")));
        }
        Auth::ApiKey {
            key,
            value,
            add_to,
            disabled: false,
        } if !key.is_empty() => match add_to.as_deref() {
            Some("queryParams") => {
                url.query_pairs_mut().append_pair(key, value);
            }
            Some("cookie") => cookie_extra.push(format!("{key}={value}")),
            _ => headers.push((key.clone(), value.clone())),
        },
        _ => {}
    }
}

fn wire_body(body: &Body) -> Result<WireBody, HttpError> {
    let mime_type = body.mime_type.clone().filter(|m| !m.is_empty());
    let read = |path: &str| {
        std::fs::read(path).map_err(|e| HttpError::File(path.to_string(), e.to_string()))
    };
    Ok(match mime_type.as_deref() {
        None => match &body.text {
            Some(t) if !t.is_empty() => WireBody::Bytes {
                content_type: None,
                data: t.clone().into_bytes(),
            },
            _ => WireBody::None,
        },
        Some(mime::FORM) => {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for p in body.params.iter().filter(|p| !p.disabled) {
                ser.append_pair(&p.name, &p.value);
            }
            WireBody::Bytes {
                content_type: Some(mime::FORM.into()),
                data: ser.finish().into_bytes(),
            }
        }
        Some(mime::MULTIPART) => WireBody::Multipart(
            body.params
                .iter()
                .filter(|p| !p.disabled)
                .cloned()
                .collect(),
        ),
        Some(mime::FILE) => match &body.file_name {
            Some(f) if !f.is_empty() => WireBody::Bytes {
                content_type: Some(mime::FILE.into()),
                data: read(f)?,
            },
            _ => WireBody::None,
        },
        Some(mime::GRAPHQL) => WireBody::Bytes {
            content_type: Some(mime::JSON.into()),
            data: body.text.clone().unwrap_or_default().into_bytes(),
        },
        Some(other) => WireBody::Bytes {
            content_type: Some(other.to_string()),
            data: body.text.clone().unwrap_or_default().into_bytes(),
        },
    })
}

struct Timeline {
    start: Instant,
    entries: Vec<TimelineEntry>,
}

impl Timeline {
    fn push(&mut self, kind: &str, text: impl Into<String>) {
        self.entries.push(TimelineEntry {
            kind: kind.into(),
            text: text.into(),
            at_ms: self.start.elapsed().as_secs_f64() * 1000.0,
        });
    }
}

fn http_version(v: reqwest::Version) -> &'static str {
    match v {
        reqwest::Version::HTTP_09 => "HTTP/0.9",
        reqwest::Version::HTTP_10 => "HTTP/1.0",
        reqwest::Version::HTTP_2 => "HTTP/2",
        reqwest::Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/1.1",
    }
}

/// Send a fully rendered request. `jar` is read for outgoing cookies and
/// updated with any `Set-Cookie` headers (including on redirect hops).
pub async fn send(
    req: &Request,
    auth: &Auth,
    opts: &Options,
    jar: &mut Vec<Cookie>,
) -> Result<HttpResponse, HttpError> {
    let start = Instant::now();
    let mut tl = Timeline {
        start,
        entries: vec![],
    };
    let method = reqwest::Method::from_bytes(req.method.trim().to_uppercase().as_bytes())
        .map_err(|_| HttpError::InvalidMethod(req.method.clone()))?;
    let mut url = build_url(req, req.settings.encode_url)?;

    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .filter(|h| !h.disabled && !h.name.trim().is_empty())
        .map(|h| (h.name.trim().to_string(), h.value.clone()))
        .collect();
    let mut auth_cookies = vec![];
    apply_auth(auth, &mut headers, &mut url, &mut auth_cookies);
    if let Some(ua) = &opts.user_agent
        && !req.settings.disable_user_agent
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
    {
        headers.push(("User-Agent".into(), ua.clone()));
    }
    let body = wire_body(&req.body)?;

    let mut builder = reqwest::Client::builder();
    if let Some(p) = opts
        .proxy
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let proxy = reqwest::Proxy::all(p)
            .map_err(|e| HttpError::InvalidUrl(p.to_string(), format!("proxy: {e}")))?
            .no_proxy(
                opts.no_proxy
                    .as_deref()
                    .and_then(reqwest::NoProxy::from_string),
            );
        builder = builder.proxy(proxy);
        tl.push("info", format!("Using proxy {p}"));
    }
    let client = builder
        .redirect(reqwest::redirect::Policy::none())
        .danger_accept_invalid_certs(!opts.validate_certificates)
        .timeout(opts.timeout)
        .build()
        .map_err(|e| HttpError::Network(e.to_string()))?;

    let mut method = method;
    let mut body = body;
    let mut redirects = 0usize;
    // Digest: (challenge, nonce count) once the server has challenged us.
    let mut digest: Option<(sign::DigestChallenge, u32)> = None;
    let mut digest_attempted = false;
    tl.push("info", format!("Preparing request to {url}"));
    tl.push(
        "info",
        format!("Current time is {}", chrono::Utc::now().to_rfc3339()),
    );

    loop {
        let mut rb = client.request(method.clone(), url.clone());
        let mut sent_headers = headers.clone();
        let mut cookie_parts = auth_cookies.clone();
        if opts.send_cookies
            && let Some(c) = cookies::header_for(jar, &url)
        {
            cookie_parts.push(c);
        }
        if !cookie_parts.is_empty()
            && !sent_headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("cookie"))
        {
            sent_headers.push(("Cookie".into(), cookie_parts.join("; ")));
        }
        match &body {
            WireBody::None => {}
            WireBody::Bytes { content_type, data } => {
                if let Some(ct) = content_type
                    && !sent_headers
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                {
                    sent_headers.push(("Content-Type".into(), ct.clone()));
                }
                rb = rb.body(data.clone());
            }
            WireBody::Multipart(params) => {
                let mut form = reqwest::multipart::Form::new();
                for p in params {
                    if p.kind.as_deref() == Some("file") {
                        let path = p.file_name.clone().unwrap_or_default();
                        let data = std::fs::read(&path)
                            .map_err(|e| HttpError::File(path.clone(), e.to_string()))?;
                        let fname = std::path::Path::new(&path)
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned());
                        let mut part = reqwest::multipart::Part::bytes(data);
                        if let Some(f) = fname {
                            part = part.file_name(f);
                        }
                        form = form.part(p.name.clone(), part);
                    } else {
                        form = form.text(p.name.clone(), p.value.clone());
                    }
                }
                // let reqwest set the boundary content-type
                sent_headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
                rb = rb.multipart(form);
            }
        }
        for (k, v) in &sent_headers {
            rb = rb.header(k, v);
        }
        let mut wire = rb.build().map_err(|e| HttpError::Network(e.to_string()))?;
        apply_wire_auth(auth, &mut wire, &mut digest, &mut tl)?;

        tl.push(
            "header-out",
            format!("{} {} HTTP/1.1", wire.method(), request_target(wire.url())),
        );
        tl.push("header-out", format!("Host: {}", host_header(wire.url())));
        for (k, v) in wire.headers() {
            tl.push(
                "header-out",
                format!("{}: {}", k, v.to_str().unwrap_or("<binary>")),
            );
        }
        if let Some(b) = wire.body().and_then(|b| b.as_bytes()) {
            let shown = &b[..b.len().min(opts.timeline_body_limit)];
            tl.push("data-out", String::from_utf8_lossy(shown).into_owned());
        }

        let sent_at = Instant::now();
        let resp = client.execute(wire).await.map_err(|e| {
            if e.is_timeout() {
                HttpError::Timeout(opts.timeout.as_millis() as u64)
            } else {
                HttpError::Network(error_chain(&e))
            }
        })?;
        let ttfb = sent_at.elapsed();
        let status = resp.status();
        let version = http_version(resp.version());
        tl.push(
            "header-in",
            format!(
                "{version} {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            ),
        );
        let resp_headers: Vec<KeyValue> = resp
            .headers()
            .iter()
            .map(|(k, v)| KeyValue::new(k.as_str(), String::from_utf8_lossy(v.as_bytes())))
            .collect();
        for h in &resp_headers {
            tl.push("header-in", format!("{}: {}", h.name, h.value));
        }
        if opts.store_cookies {
            for h in resp_headers
                .iter()
                .filter(|h| h.name.eq_ignore_ascii_case("set-cookie"))
            {
                if let Some(c) = cookies::parse_set_cookie(&h.value, &url) {
                    tl.push(
                        "info",
                        format!("Stored cookie \"{}\" for domain {}", c.name, c.domain),
                    );
                    cookies::store(jar, c);
                }
            }
        }

        if status == reqwest::StatusCode::UNAUTHORIZED
            && !digest_attempted
            && let Auth::Digest {
                disabled: false, ..
            } = auth
            && let Some(c) = resp
                .headers()
                .get_all(reqwest::header::WWW_AUTHENTICATE)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .find_map(sign::parse_digest)
        {
            tl.push(
                "info",
                format!(
                    "Digest challenge (realm \"{}\", {}); retrying with credentials",
                    c.realm, c.algorithm
                ),
            );
            digest = Some((c, 0));
            digest_attempted = true;
            continue;
        }

        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if status.is_redirection() && opts.follow_redirects && location.is_some() {
            if redirects >= opts.max_redirects {
                return Err(HttpError::TooManyRedirects(opts.max_redirects));
            }
            let next = url
                .join(location.as_deref().unwrap_or_default())
                .map_err(|e| {
                    HttpError::InvalidUrl(location.clone().unwrap_or_default(), e.to_string())
                })?;
            redirects += 1;
            tl.push("info", format!("Following redirect #{redirects} to {next}"));
            // 301/302/303 switch to GET without a body (browser/curl behavior); 307/308 keep both.
            if matches!(status.as_u16(), 301..=303) && method != reqwest::Method::HEAD {
                method = reqwest::Method::GET;
                body = WireBody::None;
                headers.retain(|(k, _)| {
                    !k.eq_ignore_ascii_case("content-type")
                        && !k.eq_ignore_ascii_case("content-length")
                });
            }
            if next.host_str() != url.host_str() {
                headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
            }
            url = next;
            continue;
        }

        let dl_start = Instant::now();
        let bytes = resp.bytes().await.map_err(|e| {
            if e.is_timeout() {
                HttpError::Timeout(opts.timeout.as_millis() as u64)
            } else {
                HttpError::Network(error_chain(&e))
            }
        })?;
        let download = dl_start.elapsed();
        tl.push("data-in", format!("Received {} B", bytes.len()));
        let content_type = resp_headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.clone())
            .unwrap_or_default();
        let total = start.elapsed();
        return Ok(HttpResponse {
            method: method.to_string(),
            url: url.to_string(),
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or("").to_string(),
            http_version: version.to_string(),
            headers: resp_headers,
            body: bytes.to_vec(),
            content_type,
            timings: Timings {
                dns_ms: None,
                connect_ms: None,
                tls_ms: None,
                ttfb_ms: ms(ttfb),
                download_ms: ms(download),
                total_ms: ms(total),
            },
            timeline: tl.entries,
            redirects,
        });
    }
}

/// Auth that signs the final wire request (OAuth 1, AWS SigV4, Digest).
fn apply_wire_auth(
    auth: &Auth,
    wire: &mut reqwest::Request,
    digest: &mut Option<(sign::DigestChallenge, u32)>,
    tl: &mut Timeline,
) -> Result<(), HttpError> {
    let set = |wire: &mut reqwest::Request, name: &str, value: &str| -> Result<(), HttpError> {
        let n = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| HttpError::Network(e.to_string()))?;
        let v = reqwest::header::HeaderValue::from_str(value)
            .map_err(|e| HttpError::Network(format!("header {name}: {e}")))?;
        wire.headers_mut().insert(n, v);
        Ok(())
    };
    match auth {
        Auth::Digest {
            username,
            password,
            disabled: false,
        } => {
            if let Some((c, nc)) = digest {
                *nc += 1;
                let cnonce = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
                let h = sign::digest_authorization(
                    c,
                    username,
                    password,
                    wire.method().as_str(),
                    &request_target(wire.url()),
                    &cnonce,
                    *nc,
                );
                set(wire, "authorization", &h)?;
            }
        }
        Auth::OAuth1(cfg) if !cfg.disabled => {
            let body = wire.body().and_then(|b| b.as_bytes()).map(|b| b.to_vec());
            let is_form = wire
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with(irs_core::mime::FORM));
            let form = if is_form {
                body.as_deref().map(sign::form_pairs).unwrap_or_default()
            } else {
                vec![]
            };
            let h = sign::oauth1_authorization(
                cfg,
                wire.method().as_str(),
                wire.url(),
                &form,
                body.as_deref(),
            );
            set(wire, "authorization", &h)?;
            tl.push(
                "info",
                format!(
                    "Signed with OAuth 1.0a ({})",
                    if cfg.signature_method.is_empty() {
                        "HMAC-SHA1"
                    } else {
                        &cfg.signature_method
                    }
                ),
            );
        }
        Auth::Iam(cfg) if !cfg.disabled => {
            let mut headers: Vec<(String, String)> = wire
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();
            if !headers.iter().any(|(k, _)| k == "host") {
                headers.push(("host".into(), host_header(wire.url())));
            }
            let body = wire
                .body()
                .and_then(|b| b.as_bytes())
                .map(|b| b.to_vec())
                .or(if wire.body().is_none() {
                    Some(vec![])
                } else {
                    None
                });
            let signed = sign::aws_sigv4_headers(
                cfg,
                wire.method().as_str(),
                wire.url().as_str(),
                &headers,
                body.as_deref(),
                std::time::SystemTime::now(),
            )
            .map_err(|e| HttpError::Network(format!("AWS SigV4 signing failed: {e}")))?;
            for (k, v) in signed {
                set(wire, &k, &v)?;
            }
            tl.push(
                "info",
                format!("Signed with AWS SigV4 ({}/{})", cfg.region, cfg.service),
            );
        }
        _ => {}
    }
    Ok(())
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1000.0 * 100.0).round() / 100.0
}

fn request_target(u: &Url) -> String {
    match u.query() {
        Some(q) => format!("{}?{q}", u.path()),
        None => u.path().to_string(),
    }
}

fn host_header(u: &Url) -> String {
    match u.port() {
        Some(p) => format!("{}:{p}", u.host_str().unwrap_or("")),
        None => u.host_str().unwrap_or("").to_string(),
    }
}

fn error_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        let m = c.to_string();
        if !s.contains(&m) {
            s.push_str(": ");
            s.push_str(&m);
        }
        cur = c.source();
    }
    s
}

#[cfg(test)]
mod tests;
