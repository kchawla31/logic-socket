//! "Generate code" snippets from a fully rendered request.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub name: String,
    pub value: String,
    /// Path of a file to upload instead of `value`.
    pub file: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CodeBody {
    #[default]
    None,
    Text {
        text: String,
    },
    Form {
        fields: Vec<(String, String)>,
    },
    Multipart {
        parts: Vec<Part>,
    },
    File {
        path: String,
    },
}

/// A request with templates, auth and query parameters already applied.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: CodeBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Curl,
    Httpie,
    JsFetch,
    PythonRequests,
    Go,
    RustReqwest,
}

impl Target {
    pub const ALL: [Target; 6] = [
        Target::Curl,
        Target::Httpie,
        Target::JsFetch,
        Target::PythonRequests,
        Target::Go,
        Target::RustReqwest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Target::Curl => "cURL",
            Target::Httpie => "HTTPie",
            Target::JsFetch => "JavaScript (fetch)",
            Target::PythonRequests => "Python (requests)",
            Target::Go => "Go (net/http)",
            Target::RustReqwest => "Rust (reqwest)",
        }
    }

    pub fn parse(s: &str) -> Option<Target> {
        Some(match s.to_ascii_lowercase().as_str() {
            "curl" => Target::Curl,
            "httpie" | "http" => Target::Httpie,
            "js" | "javascript" | "fetch" | "js-fetch" => Target::JsFetch,
            "python" | "py" | "requests" | "python-requests" => Target::PythonRequests,
            "go" | "golang" => Target::Go,
            "rust" | "reqwest" | "rust-reqwest" => Target::RustReqwest,
            _ => return None,
        })
    }
}

/// POSIX single-quote a word.
pub fn sh(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// A double-quoted string literal valid in JS, Python, Go and Rust (JSON escaping).
fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

pub fn generate(r: &CodeRequest, t: Target) -> String {
    match t {
        Target::Curl => curl(r),
        Target::Httpie => httpie(r),
        Target::JsFetch => fetch(r),
        Target::PythonRequests => python(r),
        Target::Go => go(r),
        Target::RustReqwest => rust(r),
    }
}

fn curl(r: &CodeRequest) -> String {
    let mut parts = vec![format!(
        "curl --request {} \\\n  --url {}",
        r.method,
        sh(&r.url)
    )];
    for (k, v) in &r.headers {
        parts.push(format!("  --header {}", sh(&format!("{k}: {v}"))));
    }
    match &r.body {
        CodeBody::None => {}
        // --data-raw: plain --data would read a body starting with '@' as a file.
        CodeBody::Text { text } => parts.push(format!("  --data-raw {}", sh(text))),
        CodeBody::Form { fields } => {
            for (k, v) in fields {
                parts.push(format!("  --data-urlencode {}", sh(&format!("{k}={v}"))));
            }
        }
        CodeBody::Multipart { parts: ps } => {
            for p in ps {
                let v = match &p.file {
                    Some(f) => format!("{}=@{f}", p.name),
                    None => format!("{}={}", p.name, p.value),
                };
                parts.push(format!("  --form {}", sh(&v)));
            }
        }
        CodeBody::File { path } => {
            parts.push(format!("  --data-binary {}", sh(&format!("@{path}"))))
        }
    }
    parts.join(" \\\n")
}

fn httpie(r: &CodeRequest) -> String {
    let mut out = String::new();
    let mut args = vec![];
    match &r.body {
        CodeBody::Text { text } => out.push_str(&format!("printf %s {} | ", sh(text))),
        CodeBody::File { path } => out.push_str(&format!("cat {} | ", sh(path))),
        CodeBody::Form { fields } => {
            args.push("--form".to_string());
            args.extend(fields.iter().map(|(k, v)| sh(&format!("{k}={v}"))));
        }
        CodeBody::Multipart { parts } => {
            args.push("--multipart".to_string());
            args.extend(parts.iter().map(|p| match &p.file {
                Some(f) => sh(&format!("{}@{f}", p.name)),
                None => sh(&format!("{}={}", p.name, p.value)),
            }));
        }
        CodeBody::None => {}
    }
    out.push_str(&format!("http {} {}", r.method, sh(&r.url)));
    for (k, v) in &r.headers {
        out.push_str(&format!(" \\\n  {}", sh(&format!("{k}:{v}"))));
    }
    for a in args {
        out.push_str(&format!(" \\\n  {a}"));
    }
    out
}

fn fetch(r: &CodeRequest) -> String {
    let mut pre = String::new();
    let mut opts = vec![format!("  method: {}", q(&r.method))];
    let headers: Vec<String> = r
        .headers
        .iter()
        .map(|(k, v)| format!("    {}: {}", q(k), q(v)))
        .collect();
    if !headers.is_empty() {
        opts.push(format!("  headers: {{\n{}\n  }}", headers.join(",\n")));
    }
    match &r.body {
        CodeBody::None => {}
        CodeBody::Text { text } => opts.push(format!("  body: {}", q(text))),
        CodeBody::Form { fields } => {
            pre.push_str("const body = new URLSearchParams();\n");
            for (k, v) in fields {
                pre.push_str(&format!("body.append({}, {});\n", q(k), q(v)));
            }
            opts.push("  body".into());
        }
        CodeBody::Multipart { parts } => {
            pre.push_str("const body = new FormData();\n");
            for p in parts {
                match &p.file {
                    Some(f) => pre.push_str(&format!(
                        "body.append({}, fileInput.files[0]); // {}\n",
                        q(&p.name),
                        f
                    )),
                    None => {
                        pre.push_str(&format!("body.append({}, {});\n", q(&p.name), q(&p.value)))
                    }
                }
            }
            opts.push("  body".into());
        }
        CodeBody::File { path } => {
            pre.push_str(&format!("const body = fileInput.files[0]; // {path}\n"));
            opts.push("  body".into());
        }
    }
    format!(
        "{pre}const response = await fetch({}, {{\n{}\n}});\nconsole.log(response.status, await response.text());\n",
        q(&r.url),
        opts.join(",\n")
    )
}

fn python(r: &CodeRequest) -> String {
    let mut s = String::from("import requests\n\n");
    let mut args = vec![q(&r.method), q(&r.url)];
    if !r.headers.is_empty() {
        s.push_str("headers = {\n");
        for (k, v) in &r.headers {
            s.push_str(&format!("    {}: {},\n", q(k), q(v)));
        }
        s.push_str("}\n");
        args.push("headers=headers".into());
    }
    match &r.body {
        CodeBody::None => {}
        CodeBody::Text { text } => {
            s.push_str(&format!("data = {}\n", q(text)));
            args.push("data=data.encode()".into());
        }
        CodeBody::Form { fields } => {
            s.push_str(&format!(
                "data = [{}]\n",
                fields
                    .iter()
                    .map(|(k, v)| format!("({}, {})", q(k), q(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            args.push("data=data".into());
        }
        CodeBody::Multipart { parts } => {
            s.push_str("files = [\n");
            for p in parts {
                match &p.file {
                    Some(f) => {
                        s.push_str(&format!("    ({}, open({}, \"rb\")),\n", q(&p.name), q(f)))
                    }
                    None => {
                        s.push_str(&format!("    ({}, (None, {})),\n", q(&p.name), q(&p.value)))
                    }
                }
            }
            s.push_str("]\n");
            args.push("files=files".into());
        }
        CodeBody::File { path } => {
            s.push_str(&format!("data = open({}, \"rb\")\n", q(path)));
            args.push("data=data".into());
        }
    }
    s.push_str(&format!(
        "\nresponse = requests.request({})\nprint(response.status_code, response.text)\n",
        args.join(", ")
    ));
    s
}

fn go(r: &CodeRequest) -> String {
    let (imports, body_setup, body_arg) = match &r.body {
        CodeBody::None => (vec![], String::new(), "nil".to_string()),
        CodeBody::Text { text } => (
            vec!["\"strings\""],
            String::new(),
            format!("strings.NewReader({})", q(text)),
        ),
        CodeBody::Form { fields } => (
            vec!["\"net/url\"", "\"strings\""],
            format!(
                "\tform := url.Values{{}}\n{}",
                fields
                    .iter()
                    .map(|(k, v)| format!("\tform.Add({}, {})\n", q(k), q(v)))
                    .collect::<String>()
            ),
            "strings.NewReader(form.Encode())".to_string(),
        ),
        CodeBody::File { path } => (
            vec!["\"os\""],
            format!(
                "\tbody, err := os.Open({})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer body.Close()\n",
                q(path)
            ),
            "body".to_string(),
        ),
        CodeBody::Multipart { parts } => {
            let mut setup =
                String::from("\tbody := &bytes.Buffer{}\n\tw := multipart.NewWriter(body)\n");
            let mut imps = vec!["\"bytes\"", "\"mime/multipart\""];
            for p in parts {
                match &p.file {
                    Some(f) => {
                        imps.extend(["\"os\"", "\"path/filepath\""]);
                        setup.push_str(&format!(
                            "\tif f, err := os.Open({f}); err == nil {{\n\t\tpart, _ := w.CreateFormFile({n}, filepath.Base({f}))\n\t\tio.Copy(part, f)\n\t\tf.Close()\n\t}}\n",
                            f = q(f),
                            n = q(&p.name)
                        ));
                    }
                    None => setup.push_str(&format!(
                        "\tw.WriteField({}, {})\n",
                        q(&p.name),
                        q(&p.value)
                    )),
                }
            }
            setup.push_str("\tw.Close()\n");
            (imps, setup, "body".to_string())
        }
    };
    let mut imp = vec!["\"fmt\"", "\"io\"", "\"net/http\""];
    imp.extend(imports);
    imp.sort();
    imp.dedup();
    let mut headers: String = r
        .headers
        .iter()
        .map(|(k, v)| format!("\treq.Header.Add({}, {})\n", q(k), q(v)))
        .collect();
    if matches!(r.body, CodeBody::Multipart { .. }) {
        headers.push_str("\treq.Header.Set(\"Content-Type\", w.FormDataContentType())\n");
    }
    format!(
        "package main\n\nimport (\n{}\n)\n\nfunc main() {{\n{body_setup}\treq, err := http.NewRequest({}, {}, {body_arg})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n{headers}\tres, err := http.DefaultClient.Do(req)\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer res.Body.Close()\n\tdata, _ := io.ReadAll(res.Body)\n\tfmt.Println(res.Status, string(data))\n}}\n",
        imp.iter()
            .map(|i| format!("\t{i}"))
            .collect::<Vec<_>>()
            .join("\n"),
        q(&r.method),
        q(&r.url)
    )
}

fn rust(r: &CodeRequest) -> String {
    let mut s = String::from(
        "// [dependencies] reqwest = { version = \"0.13\", features = [\"form\", \"multipart\"] }, tokio = { version = \"1\", features = [\"full\"] }\n",
    );
    s.push_str("#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n    let client = reqwest::Client::new();\n");
    let mut chain = format!(
        "    let response = client\n        .request(reqwest::Method::from_bytes(b{})?, {})\n",
        q(&r.method),
        q(&r.url)
    );
    for (k, v) in &r.headers {
        chain.push_str(&format!("        .header({}, {})\n", q(k), q(v)));
    }
    match &r.body {
        CodeBody::None => {}
        CodeBody::Text { text } => chain.push_str(&format!("        .body({})\n", q(text))),
        CodeBody::Form { fields } => chain.push_str(&format!(
            "        .form(&[{}])\n",
            fields
                .iter()
                .map(|(k, v)| format!("({}, {})", q(k), q(v)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        CodeBody::Multipart { parts } => {
            s.push_str("    let form = reqwest::multipart::Form::new()");
            for p in parts {
                match &p.file {
                    Some(f) => {
                        s.push_str(&format!("\n        .file({}, {}).await?", q(&p.name), q(f)))
                    }
                    None => {
                        s.push_str(&format!("\n        .text({}, {})", q(&p.name), q(&p.value)))
                    }
                }
            }
            s.push_str(";\n");
            chain.push_str("        .multipart(form)\n");
        }
        CodeBody::File { path } => {
            chain.push_str(&format!("        .body(std::fs::read({})?)\n", q(path)))
        }
    }
    chain.push_str("        .send()\n        .await?;\n");
    s.push_str(&chain);
    s.push_str(
        "    println!(\"{} {}\", response.status(), response.text().await?);\n    Ok(())\n}\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// curl reads `--data @file`. A body that starts with `@` is still body text.
    #[test]
    fn curl_text_body_starting_with_at_is_not_a_file() {
        let s = generate(
            &CodeRequest {
                method: "POST".into(),
                url: "https://ex.test/a".into(),
                body: CodeBody::Text {
                    text: "@not-a-file".into(),
                },
                ..Default::default()
            },
            Target::Curl,
        );
        assert!(
            s.contains("--data-raw @not-a-file") || s.contains("--data-raw '@not-a-file'"),
            "{s}"
        );
        assert!(!s.contains("--data @not-a-file"), "{s}");
    }
}
