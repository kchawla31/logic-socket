//! Minimal RFC 6265 cookie jar operating directly on `lsock_core::Cookie`.

use lsock_core::Cookie;
use url::Url;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Parse one `Set-Cookie` header value received from `url`.
pub fn parse_set_cookie(header: &str, url: &Url) -> Option<Cookie> {
    let mut parts = header.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let mut c = Cookie {
        name: name.to_string(),
        value: value.trim().trim_matches('"').to_string(),
        domain: host.clone(),
        path: default_path(url),
        host_only: true,
        ..Default::default()
    };
    let mut max_age: Option<i64> = None;
    for attr in parts {
        let (k, v) = attr
            .split_once('=')
            .map(|(k, v)| (k.trim(), v.trim()))
            .unwrap_or((attr.trim(), ""));
        match k.to_ascii_lowercase().as_str() {
            "domain" if !v.is_empty() => {
                let d = v.trim_start_matches('.').to_ascii_lowercase();
                if !domain_match(&host, &d) {
                    return None; // a server may not set cookies for unrelated domains
                }
                c.domain = d;
                c.host_only = false;
            }
            "path" if v.starts_with('/') => c.path = v.to_string(),
            "expires" => {
                if let Ok(t) = chrono::DateTime::parse_from_rfc2822(&v.replace('-', " ")) {
                    c.expires = Some(t.timestamp_millis());
                }
            }
            "max-age" => max_age = v.parse().ok(),
            "secure" => c.secure = true,
            "httponly" => c.http_only = true,
            _ => {}
        }
    }
    if let Some(secs) = max_age {
        c.expires = Some(now_ms() + secs * 1000);
    }
    Some(c)
}

fn default_path(url: &Url) -> String {
    let p = url.path();
    match p.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => p[..i].to_string(),
    }
}

fn domain_match(host: &str, domain: &str) -> bool {
    host == domain || (host.ends_with(domain) && host[..host.len() - domain.len()].ends_with('.'))
}

fn path_match(req: &str, cookie: &str) -> bool {
    req == cookie
        || (req.starts_with(cookie)
            && (cookie.ends_with('/') || req[cookie.len()..].starts_with('/')))
}

fn is_expired(c: &Cookie) -> bool {
    c.expires.is_some_and(|e| e <= now_ms())
}

/// Insert or replace a cookie (same name/domain/path); expired cookies delete.
pub fn store(jar: &mut Vec<Cookie>, cookie: Cookie) {
    jar.retain(|c| !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path));
    if !is_expired(&cookie) {
        jar.push(cookie);
    }
}

/// Value for the `Cookie` request header, or `None` if no cookies apply.
pub fn header_for(jar: &[Cookie], url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    let secure = url.scheme() == "https";
    let mut matching: Vec<&Cookie> = jar
        .iter()
        .filter(|c| !is_expired(c))
        .filter(|c| {
            if c.host_only {
                host == c.domain
            } else {
                domain_match(&host, &c.domain)
            }
        })
        .filter(|c| path_match(url.path(), &c.path))
        .filter(|c| !c.secure || secure)
        .collect();
    if matching.is_empty() {
        return None;
    }
    matching.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
    Some(
        matching
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn parse_and_match() {
        let url = u("https://api.example.com/v1/login");
        let c = parse_set_cookie(
            "sid=abc; Path=/; Domain=.example.com; Secure; HttpOnly",
            &url,
        )
        .unwrap();
        assert_eq!(
            (c.domain.as_str(), c.host_only, c.secure, c.http_only),
            ("example.com", false, true, true)
        );
        let mut jar = vec![];
        store(&mut jar, c);
        store(&mut jar, parse_set_cookie("v=1", &url).unwrap()); // host-only, path /v1
        assert_eq!(
            header_for(&jar, &u("https://api.example.com/v1/x")).unwrap(),
            "v=1; sid=abc"
        );
        assert_eq!(
            header_for(&jar, &u("https://www.example.com/")).unwrap(),
            "sid=abc"
        );
        assert!(header_for(&jar, &u("http://api.example.com/v1/x")).unwrap() == "v=1"); // secure excluded
        assert!(header_for(&jar, &u("https://other.com/")).is_none());
    }

    #[test]
    fn foreign_domain_rejected_and_expiry_deletes() {
        let url = u("https://a.com/");
        assert!(parse_set_cookie("x=1; Domain=b.com", &url).is_none());
        let mut jar = vec![];
        store(&mut jar, parse_set_cookie("x=1", &url).unwrap());
        store(&mut jar, parse_set_cookie("x=; Max-Age=0", &url).unwrap());
        assert!(jar.is_empty());
        let c = parse_set_cookie("y=1; Expires=Wed, 21 Oct 2099 07:28:00 GMT", &url).unwrap();
        assert!(c.expires.unwrap() > now_ms());
    }
}
