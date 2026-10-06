//! Wire-level auth: HTTP Digest (RFC 7616/2617), OAuth 1.0a (RFC 5849), AWS SigV4.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use lsock_core::{AwsIamConfig, OAuth1Config};
use sha1::Sha1;
use sha2::{Digest as _, Sha256};

// ---------------------------------------------------------------- percent-encoding

/// RFC 3986 encoding (unreserved: ALPHA / DIGIT / "-" / "." / "_" / "~").
pub fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn pct_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={s}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- Digest

#[derive(Debug, Default, Clone, PartialEq)]
pub struct DigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    pub qop: Option<String>,
    pub algorithm: String,
}

/// Parse `WWW-Authenticate: Digest realm="...", nonce="...", ...`.
pub fn parse_digest(header: &str) -> Option<DigestChallenge> {
    let rest = header
        .trim()
        .strip_prefix("Digest")
        .or_else(|| header.trim().strip_prefix("digest"))?;
    let mut c = DigestChallenge {
        algorithm: "MD5".into(),
        ..Default::default()
    };
    let mut chars = rest.chars().peekable();
    loop {
        while matches!(chars.peek(), Some(' ' | ',')) {
            chars.next();
        }
        let key: String = std::iter::from_fn(|| chars.next_if(|c| *c != '=')).collect();
        if key.is_empty() || chars.next() != Some('=') {
            break;
        }
        let value: String = if chars.peek() == Some(&'"') {
            chars.next();
            let mut v = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => v.extend(chars.next()),
                    '"' => break,
                    c => v.push(c),
                }
            }
            v
        } else {
            std::iter::from_fn(|| chars.next_if(|c| *c != ','))
                .collect::<String>()
                .trim()
                .to_string()
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "realm" => c.realm = value,
            "nonce" => c.nonce = value,
            "opaque" => c.opaque = Some(value),
            "qop" => c.qop = Some(value),
            "algorithm" => c.algorithm = value,
            _ => {}
        }
    }
    (!c.nonce.is_empty()).then_some(c)
}

fn hex_hash(alg: &str, s: &str) -> String {
    if alg.to_ascii_uppercase().starts_with("SHA-256") {
        Sha256::digest(s.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    } else {
        format!("{:x}", md5::compute(s.as_bytes()))
    }
}

/// Build the `Authorization: Digest ...` value for one request.
pub fn digest_authorization(
    c: &DigestChallenge,
    user: &str,
    pass: &str,
    method: &str,
    uri: &str,
    cnonce: &str,
    nc: u32,
) -> String {
    let alg = c.algorithm.clone();
    let mut ha1 = hex_hash(&alg, &format!("{user}:{}:{pass}", c.realm));
    if alg.to_ascii_lowercase().ends_with("-sess") {
        ha1 = hex_hash(&alg, &format!("{ha1}:{}:{cnonce}", c.nonce));
    }
    let ha2 = hex_hash(&alg, &format!("{method}:{uri}"));
    let qop = c
        .qop
        .as_deref()
        .and_then(|q| q.split(',').map(str::trim).find(|q| *q == "auth"));
    let nc = format!("{nc:08x}");
    let response = match qop {
        Some(q) => hex_hash(&alg, &format!("{ha1}:{}:{nc}:{cnonce}:{q}:{ha2}", c.nonce)),
        None => hex_hash(&alg, &format!("{ha1}:{}:{ha2}", c.nonce)),
    };
    let mut h = format!(
        r#"Digest username="{user}", realm="{}", nonce="{}", uri="{uri}", algorithm={alg}, response="{response}""#,
        c.realm, c.nonce
    );
    if let Some(q) = qop {
        h.push_str(&format!(r#", qop={q}, nc={nc}, cnonce="{cnonce}""#));
    }
    if let Some(o) = &c.opaque {
        h.push_str(&format!(r#", opaque="{o}""#));
    }
    h
}

// ---------------------------------------------------------------- OAuth 1.0a

/// Returns the `Authorization: OAuth ...` header value.
/// `form_params` are the decoded `application/x-www-form-urlencoded` body params (if any);
/// `raw_body` is used for `oauth_body_hash` when the body is not a form.
pub fn oauth1_authorization(
    cfg: &OAuth1Config,
    method: &str,
    url: &url::Url,
    form_params: &[(String, String)],
    raw_body: Option<&[u8]>,
) -> String {
    let nonce = if cfg.nonce.is_empty() {
        uuid::Uuid::new_v4().simple().to_string()
    } else {
        cfg.nonce.clone()
    };
    let timestamp = if cfg.timestamp.is_empty() {
        chrono::Utc::now().timestamp().to_string()
    } else {
        cfg.timestamp.clone()
    };
    let method_name = if cfg.signature_method.is_empty() {
        "HMAC-SHA1".to_string()
    } else {
        cfg.signature_method.to_ascii_uppercase()
    };
    let mut oauth: Vec<(String, String)> = vec![
        ("oauth_consumer_key".into(), cfg.consumer_key.clone()),
        ("oauth_nonce".into(), nonce),
        ("oauth_signature_method".into(), method_name.clone()),
        ("oauth_timestamp".into(), timestamp),
        ("oauth_version".into(), "1.0".into()),
    ];
    if !cfg.token_key.is_empty() {
        oauth.push(("oauth_token".into(), cfg.token_key.clone()));
    }
    if !cfg.callback.is_empty() {
        oauth.push(("oauth_callback".into(), cfg.callback.clone()));
    }
    if !cfg.verifier.is_empty() {
        oauth.push(("oauth_verifier".into(), cfg.verifier.clone()));
    }
    if cfg.include_body_hash
        && form_params.is_empty()
        && let Some(b) = raw_body
    {
        let h = if method_name == "HMAC-SHA256" {
            STANDARD.encode(Sha256::digest(b))
        } else {
            STANDARD.encode(Sha1::digest(b))
        };
        oauth.push(("oauth_body_hash".into(), h));
    }

    let mut all: Vec<(String, String)> = oauth.clone();
    for (k, v) in url.query_pairs() {
        all.push((k.into_owned(), v.into_owned()));
    }
    all.extend(form_params.iter().cloned());
    let mut enc: Vec<(String, String)> = all.iter().map(|(k, v)| (pct(k), pct(v))).collect();
    enc.sort();
    let param_str = enc
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut base_url = url.clone();
    base_url.set_query(None);
    base_url.set_fragment(None);
    let base = format!(
        "{}&{}&{}",
        method.to_ascii_uppercase(),
        pct(base_url.as_str()),
        pct(&param_str)
    );
    let key = format!("{}&{}", pct(&cfg.consumer_secret), pct(&cfg.token_secret));
    let signature = match method_name.as_str() {
        "PLAINTEXT" => key,
        "HMAC-SHA256" => {
            let mut m = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("any key length");
            m.update(base.as_bytes());
            STANDARD.encode(m.finalize().into_bytes())
        }
        _ => {
            let mut m = Hmac::<Sha1>::new_from_slice(key.as_bytes()).expect("any key length");
            m.update(base.as_bytes());
            STANDARD.encode(m.finalize().into_bytes())
        }
    };
    oauth.push(("oauth_signature".into(), signature));
    oauth.sort();
    let mut parts = vec![];
    if !cfg.realm.is_empty() {
        parts.push(format!(r#"realm="{}""#, cfg.realm));
    }
    parts.extend(
        oauth
            .iter()
            .map(|(k, v)| format!(r#"{}="{}""#, pct(k), pct(v))),
    );
    format!("OAuth {}", parts.join(", "))
}

/// Parse an urlencoded body into decoded pairs.
pub fn form_pairs(body: &[u8]) -> Vec<(String, String)> {
    let s = String::from_utf8_lossy(body);
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (pct_decode(k), pct_decode(v))
        })
        .collect()
}

// ---------------------------------------------------------------- AWS SigV4

/// Headers to add for AWS Signature V4. `body` is `None` for streamed bodies (unsigned payload).
pub fn aws_sigv4_headers(
    cfg: &AwsIamConfig,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
    time: std::time::SystemTime,
) -> Result<Vec<(String, String)>, String> {
    use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
    use aws_sigv4::sign::v4;
    let session = Some(cfg.session_token.clone()).filter(|s| !s.is_empty());
    let identity = aws_credential_types::Credentials::new(
        &cfg.access_key_id,
        &cfg.secret_access_key,
        session,
        None,
        "logic-socket",
    )
    .into();
    let region = if cfg.region.is_empty() {
        "us-east-1"
    } else {
        cfg.region.as_str()
    };
    let service = if cfg.service.is_empty() {
        "execute-api"
    } else {
        cfg.service.as_str()
    };
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(service)
        .time(time)
        .settings(SigningSettings::default())
        .build()
        .map_err(|e| e.to_string())?
        .into();
    let body = match body {
        Some(b) => SignableBody::Bytes(b),
        None => SignableBody::UnsignedPayload,
    };
    let req = SignableRequest::new(
        method,
        url,
        headers.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        body,
    )
    .map_err(|e| e.to_string())?;
    let (instructions, _sig) = sign(req, &params).map_err(|e| e.to_string())?.into_parts();
    Ok(instructions
        .headers()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_rfc2617_example() {
        let c = parse_digest(r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#).unwrap();
        assert_eq!(c.realm, "testrealm@host.com");
        let h = digest_authorization(
            &c,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            "0a4f113b",
            1,
        );
        assert!(
            h.contains(r#"response="6629fae49393a05397450978507c4ef1""#),
            "{h}"
        );
        assert!(
            h.contains("qop=auth, nc=00000001")
                && h.contains(r#"opaque="5ccc069c403ebaf9f0171e9517f40e41""#)
        );
        let sha = DigestChallenge {
            algorithm: "SHA-256".into(),
            ..c
        };
        let sha_resp = digest_authorization(&sha, "u", "p", "GET", "/", "c", 1);
        let value = sha_resp
            .split("response=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(value.len(), 64, "SHA-256 response is 64 hex chars");
    }

    #[test]
    fn oauth1_matches_twitter_documented_example() {
        let cfg = OAuth1Config {
            consumer_key: "xvz1evFS4wEEPTGEFPHBog".into(),
            consumer_secret: "kAcSOqF21Fu85e7zjz7ZN2U4ZRhfV3WpwPAoE3Z7kBw".into(),
            token_key: "370773112-GmHxMAgYyLbNEtIKZeRNFsMKPR9EyMZeS9weJAEb".into(),
            token_secret: "LswwdoUaIvS8ltyTt5jkRh4J50vUPVVHtR2YPi5kE".into(),
            nonce: "kYjzVBB8Y0ZFabxSWbWovY3uYSQ2pTgmZeNu2VS4cg".into(),
            timestamp: "1318622958".into(),
            ..Default::default()
        };
        let url = url::Url::parse(
            "https://api.twitter.com/1.1/statuses/update.json?include_entities=true",
        )
        .unwrap();
        let body = form_pairs(
            b"status=Hello%20Ladies%20%2b%20Gentlemen%2c%20a%20signed%20OAuth%20request%21",
        );
        assert_eq!(
            body[0].1,
            "Hello Ladies + Gentlemen, a signed OAuth request!"
        );
        let h = oauth1_authorization(&cfg, "POST", &url, &body, None);
        assert!(
            h.contains(r#"oauth_signature="hCtSmYh%2BiHYCEqBWrE7C7hYmtUk%3D""#),
            "{h}"
        );
        assert!(h.starts_with("OAuth oauth_consumer_key="));
    }

    #[test]
    fn sigv4_adds_authorization_and_date() {
        let cfg = AwsIamConfig {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            region: "us-east-1".into(),
            service: "service".into(),
            ..Default::default()
        };
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_440_938_160); // 2015-08-30T12:36:00Z
        let h = aws_sigv4_headers(
            &cfg,
            "GET",
            "https://example.amazonaws.com/",
            &[("host".into(), "example.amazonaws.com".into())],
            Some(b""),
            t,
        )
        .unwrap();
        let get = |n: &str| {
            h.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(n))
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get("x-amz-date"), "20150830T123600Z");
        let auth = get("authorization");
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders="), "{auth}");
        assert!(auth.contains("Signature="));
    }
}
