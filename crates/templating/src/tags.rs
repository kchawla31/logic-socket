//! Insomnia template tags, implemented as minijinja functions.
//!
//! Insomnia writes tags as `{% name 'arg', 'arg' %}`. minijinja has no custom
//! block tags, so known tag names are rewritten to `{{ __tag_name('arg', 'arg') }}`.
//! Built-in statements (`if`, `for`, `set`, ...) pass through unchanged.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use minijinja::{Environment, Error, ErrorKind};
use sha2::Digest;

const TAGS: &[&str] = &["uuid", "now", "base64", "hash", "timestamp", "urlencode"];

const BUILTIN: &[&str] = &[
    "if",
    "elif",
    "else",
    "endif",
    "for",
    "endfor",
    "set",
    "endset",
    "raw",
    "endraw",
    "filter",
    "endfilter",
    "macro",
    "endmacro",
    "call",
    "endcall",
    "with",
    "endwith",
    "include",
    "import",
    "from",
    "block",
    "endblock",
    "extends",
    "autoescape",
    "endautoescape",
];

pub fn rewrite(input: &str) -> Result<String, String> {
    if !input.contains("{%") {
        return Ok(input.to_string());
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("{%") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("%}") else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let inner = after[..end]
            .trim()
            .trim_start_matches('-')
            .trim_end_matches('-')
            .trim();
        let (name, args) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
        if TAGS.contains(&name) {
            out.push_str(&format!("{{{{ __tag_{name}({}) }}}}", args.trim()));
        } else if BUILTIN.contains(&name) {
            out.push_str(&rest[start..start + 2 + end + 2]);
        } else {
            return Err(format!(
                "unknown tag '{name}' (supported: {})",
                TAGS.join(", ")
            ));
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidOperation, msg.into())
}

pub fn register(env: &mut Environment<'static>) {
    env.add_function(
        "__tag_uuid",
        |kind: Option<String>| -> Result<String, Error> {
            match kind.as_deref().unwrap_or("v4") {
                "v4" | "4" => Ok(uuid::Uuid::new_v4().to_string()),
                other => Err(invalid(format!(
                    "uuid: unsupported version '{other}' (use 'v4')"
                ))),
            }
        },
    );

    env.add_function("__tag_timestamp", || chrono::Utc::now().timestamp_millis());

    env.add_function(
        "__tag_now",
        |format: Option<String>, custom: Option<String>| -> Result<String, Error> {
            let now = chrono::Utc::now();
            Ok(match format.as_deref().unwrap_or("iso-8601") {
                "iso-8601" => now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "millis" => now.timestamp_millis().to_string(),
                "unix" => now.timestamp().to_string(),
                "custom" => now
                    .format(&moment_to_strftime(custom.as_deref().unwrap_or("")))
                    .to_string(),
                other => return Err(invalid(format!("now: unknown format '{other}'"))),
            })
        },
    );

    env.add_function(
        "__tag_base64",
        |action: String, kind: String, value: String| -> Result<String, Error> {
            let engine_encode = |v: &[u8]| match kind.as_str() {
                "url" => URL_SAFE_NO_PAD.encode(v),
                _ => STANDARD.encode(v),
            };
            match action.as_str() {
                "encode" => Ok(engine_encode(value.as_bytes())),
                "decode" => {
                    let bytes = match kind.as_str() {
                        "url" => URL_SAFE_NO_PAD.decode(value.trim_end_matches('=')),
                        _ => STANDARD.decode(value),
                    }
                    .map_err(|e| invalid(format!("base64: {e}")))?;
                    String::from_utf8(bytes).map_err(|e| invalid(format!("base64: {e}")))
                }
                other => Err(invalid(format!("base64: unknown action '{other}'"))),
            }
        },
    );

    env.add_function(
        "__tag_hash",
        |algo: String, encoding: String, value: String| -> Result<String, Error> {
            let bytes: Vec<u8> = match algo.as_str() {
                "md5" => md5::compute(value.as_bytes()).0.to_vec(),
                "sha1" => return Err(invalid("hash: sha1 not supported, use sha256/sha512/md5")),
                "sha256" => sha2::Sha256::digest(value.as_bytes()).to_vec(),
                "sha512" => sha2::Sha512::digest(value.as_bytes()).to_vec(),
                other => return Err(invalid(format!("hash: unknown algorithm '{other}'"))),
            };
            Ok(match encoding.as_str() {
                "base64" => STANDARD.encode(bytes),
                _ => bytes.iter().map(|b| format!("{b:02x}")).collect(),
            })
        },
    );

    env.add_function("__tag_urlencode", |value: String| -> String {
        url_encode(&value)
    });
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

/// Translate the common moment.js tokens Insomnia's `now` tag accepts.
fn moment_to_strftime(fmt: &str) -> String {
    let pairs = [
        ("YYYY", "%Y"),
        ("YY", "%y"),
        ("MM", "%m"),
        ("DD", "%d"),
        ("HH", "%H"),
        ("mm", "%M"),
        ("ss", "%S"),
        ("SSS", "%3f"),
        ("Z", "%:z"),
    ];
    let mut out = String::new();
    let mut i = 0;
    'outer: while i < fmt.len() {
        for (m, s) in pairs {
            if fmt[i..].starts_with(m) {
                out.push_str(s);
                i += m.len();
                continue 'outer;
            }
        }
        let c = fmt[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::{Context, Mode, Renderer};

    fn r(s: &str) -> String {
        Renderer::new()
            .render_str(s, &Context::default(), Mode::Throw)
            .unwrap()
    }

    #[test]
    fn tags_render() {
        assert_eq!(r("{% uuid 'v4' %}").len(), 36);
        assert_eq!(r("{% base64 'encode', 'normal', 'hello' %}"), "aGVsbG8=");
        assert_eq!(r("{% base64 'decode', 'normal', 'aGVsbG8=' %}"), "hello");
        assert_eq!(
            r("{% hash 'sha256', 'hex', 'abc' %}"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            r("{% hash 'md5', 'hex', 'abc' %}"),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert!(r("{% now 'iso-8601' %}").ends_with('Z'));
        assert_eq!(r("{% now 'custom', 'YYYY' %}").len(), 4);
        assert_eq!(r("{% urlencode 'a b&c' %}"), "a%20b%26c");
    }

    #[test]
    fn builtin_statements_pass_through_and_unknown_tags_error() {
        assert_eq!(r("{% if true %}yes{% endif %}"), "yes");
        let err = Renderer::new()
            .render_str("{% prompt 'x' %}", &Context::default(), Mode::Throw)
            .unwrap_err();
        assert!(err.to_string().contains("unknown tag 'prompt'"));
    }
}
