use super::*;
use axum::{
    Router,
    body::Bytes,
    extract::{Multipart, Path},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Redirect},
    routing::{any, get, post},
};
use irs_core::{BodyParam, RequestSettings};
use serde_json::{Value, json};

async fn echo(method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let h: serde_json::Map<String, Value> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v.to_str().unwrap_or(""))))
        .collect();
    axum::Json(json!({
        "method": method.as_str(),
        "path": uri.path(),
        "query": uri.query(),
        "headers": h,
        "body": String::from_utf8_lossy(&body),
    }))
}

async fn multipart(mut mp: Multipart) -> impl IntoResponse {
    let mut out = serde_json::Map::new();
    while let Some(f) = mp.next_field().await.unwrap() {
        let name = f.name().unwrap().to_string();
        let file = f.file_name().map(str::to_string);
        let text = f.text().await.unwrap();
        out.insert(name, json!({"file": file, "text": text}));
    }
    axum::Json(Value::Object(out))
}

pub(crate) async fn server() -> String {
    let app = Router::new()
        .route("/echo", any(echo))
        .route("/echo/{*rest}", any(echo))
        .route("/multipart", post(multipart))
        .route(
            "/login",
            post(|| async {
                (
                    [("set-cookie", "sid=s3cr3t; Path=/"), ("location", "/echo")],
                    StatusCode::FOUND,
                )
            }),
        )
        .route("/loop", get(|| async { Redirect::temporary("/loop") }))
        .route("/keep", post(|| async { Redirect::temporary("/echo") }))
        .route(
            "/digest",
            get(|headers: HeaderMap| async move {
                let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                if !auth.starts_with("Digest ") {
                    return (
                        StatusCode::UNAUTHORIZED,
                        [("www-authenticate", r#"Digest realm="irs", qop="auth", nonce="abc123", opaque="xyz", algorithm=MD5"#)],
                        String::new(),
                    );
                }
                // verify like a server would
                let get = |k: &str| auth.split(&format!("{k}=\"")).nth(1).and_then(|r| r.split('"').next()).unwrap_or("").to_string();
                let nc = auth.split("nc=").nth(1).and_then(|r| r.split(',').next()).unwrap_or("").to_string();
                let ha1 = format!("{:x}", md5::compute("ada:irs:lovelace"));
                let ha2 = format!("{:x}", md5::compute(format!("GET:{}", get("uri"))));
                let expected = format!("{:x}", md5::compute(format!("{ha1}:abc123:{nc}:{}:auth:{ha2}", get("cnonce"))));
                if get("response") == expected {
                    (StatusCode::OK, [("www-authenticate", "")], "welcome ada".to_string())
                } else {
                    (StatusCode::UNAUTHORIZED, [("www-authenticate", "")], "bad".to_string())
                }
            }),
        )
        .route(
            "/status/{code}",
            get(|Path(c): Path<u16>| async move { StatusCode::from_u16(c).unwrap() }),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                "late"
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn req(method: &str, url: String) -> Request {
    Request {
        method: method.into(),
        url,
        ..Default::default()
    }
}

fn json_of(r: &HttpResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

#[tokio::test]
async fn get_with_query_path_params_and_headers() {
    let base = server().await;
    let mut r = req("get", format!("{base}/echo/users/:id"));
    r.path_parameters = vec![KeyValue::new("id", "a b")];
    r.parameters = vec![
        KeyValue::new("q", "x&y"),
        KeyValue {
            disabled: true,
            ..KeyValue::new("off", "1")
        },
    ];
    r.headers = vec![
        KeyValue::new("X-Test", "1"),
        KeyValue {
            disabled: true,
            ..KeyValue::new("X-Off", "1")
        },
    ];
    let res = send(&r, &Auth::None, &Options::default(), &mut vec![])
        .await
        .unwrap();
    let j = json_of(&res);
    assert_eq!(res.status, 200);
    assert_eq!(j["method"], "GET");
    assert_eq!(j["path"], "/echo/users/a%20b");
    assert_eq!(j["query"], "q=x%26y");
    assert_eq!(j["headers"]["x-test"], "1");
    assert!(j["headers"].get("x-off").is_none());
    assert!(
        j["headers"]["user-agent"]
            .as_str()
            .unwrap()
            .starts_with("insomnia-rs/")
    );
    assert!(res.timings.total_ms > 0.0);
    assert!(
        res.timeline
            .iter()
            .any(|t| t.kind == "header-out" && t.text.starts_with("GET /echo/users/a%20b?q=x%26y"))
    );
}

#[tokio::test]
async fn json_form_and_raw_bodies() {
    let base = server().await;
    let mut r = req("POST", format!("{base}/echo"));
    r.body = Body {
        mime_type: Some(mime::JSON.into()),
        text: Some(r#"{"a":1}"#.into()),
        ..Default::default()
    };
    let j = json_of(
        &send(&r, &Auth::None, &Options::default(), &mut vec![])
            .await
            .unwrap(),
    );
    assert_eq!(
        (j["body"].as_str(), j["headers"]["content-type"].as_str()),
        (Some(r#"{"a":1}"#), Some(mime::JSON))
    );

    r.body = Body {
        mime_type: Some(mime::FORM.into()),
        params: vec![
            BodyParam {
                name: "a".into(),
                value: "1 2".into(),
                ..Default::default()
            },
            BodyParam {
                name: "b".into(),
                value: "x".into(),
                disabled: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let j = json_of(
        &send(&r, &Auth::None, &Options::default(), &mut vec![])
            .await
            .unwrap(),
    );
    assert_eq!(j["body"], "a=1+2");

    r.body = Body {
        mime_type: Some("text/xml".into()),
        text: Some("<a/>".into()),
        ..Default::default()
    };
    r.headers = vec![KeyValue::new("content-type", "application/custom")];
    let j = json_of(
        &send(&r, &Auth::None, &Options::default(), &mut vec![])
            .await
            .unwrap(),
    );
    assert_eq!(j["headers"]["content-type"], "application/custom"); // explicit header wins
}

#[tokio::test]
async fn multipart_with_file() {
    let base = server().await;
    let dir = std::env::temp_dir().join(format!("irs-mp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.txt");
    std::fs::write(&file, "file-content").unwrap();
    let mut r = req("POST", format!("{base}/multipart"));
    r.body = Body {
        mime_type: Some(mime::MULTIPART.into()),
        params: vec![
            BodyParam {
                name: "field".into(),
                value: "v".into(),
                ..Default::default()
            },
            BodyParam {
                name: "upload".into(),
                kind: Some("file".into()),
                file_name: Some(file.to_string_lossy().into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let j = json_of(
        &send(&r, &Auth::None, &Options::default(), &mut vec![])
            .await
            .unwrap(),
    );
    assert_eq!(j["field"]["text"], "v");
    assert_eq!(
        j["upload"],
        json!({"file": "hello.txt", "text": "file-content"})
    );
}

#[tokio::test]
async fn auth_kinds() {
    let base = server().await;
    let r = req("GET", format!("{base}/echo"));
    let get = |a: Auth| {
        let r = r.clone();
        async move {
            json_of(
                &send(&r, &a, &Options::default(), &mut vec![])
                    .await
                    .unwrap(),
            )
        }
    };
    let j = get(Auth::Basic {
        username: "u".into(),
        password: "p".into(),
        disabled: false,
    })
    .await;
    assert_eq!(j["headers"]["authorization"], "Basic dTpw");
    let j = get(Auth::Bearer {
        token: "t".into(),
        prefix: None,
        disabled: false,
    })
    .await;
    assert_eq!(j["headers"]["authorization"], "Bearer t");
    let j = get(Auth::ApiKey {
        key: "k".into(),
        value: "v".into(),
        add_to: Some("queryParams".into()),
        disabled: false,
    })
    .await;
    assert_eq!(j["query"], "k=v");
    let j = get(Auth::ApiKey {
        key: "X-Api".into(),
        value: "v".into(),
        add_to: None,
        disabled: false,
    })
    .await;
    assert_eq!(j["headers"]["x-api"], "v");
    let j = get(Auth::Bearer {
        token: "t".into(),
        prefix: None,
        disabled: true,
    })
    .await;
    assert!(j["headers"].get("authorization").is_none());
}

#[tokio::test]
async fn redirect_stores_cookie_from_intermediate_hop_and_switches_to_get() {
    let base = server().await;
    let mut jar = vec![];
    let mut r = req("POST", format!("{base}/login"));
    r.body = Body {
        mime_type: Some(mime::JSON.into()),
        text: Some("{}".into()),
        ..Default::default()
    };
    let res = send(&r, &Auth::None, &Options::default(), &mut jar)
        .await
        .unwrap();
    let j = json_of(&res);
    assert_eq!(res.redirects, 1);
    assert_eq!(j["method"], "GET");
    assert_eq!(j["headers"]["cookie"], "sid=s3cr3t");
    assert_eq!(j["body"], "");
    assert_eq!(jar.len(), 1);

    // 307 keeps method + body
    let mut r = req("POST", format!("{base}/keep"));
    r.body = Body {
        mime_type: Some(mime::JSON.into()),
        text: Some("{\"k\":1}".into()),
        ..Default::default()
    };
    let j = json_of(
        &send(&r, &Auth::None, &Options::default(), &mut vec![])
            .await
            .unwrap(),
    );
    assert_eq!(
        (j["method"].as_str(), j["body"].as_str()),
        (Some("POST"), Some("{\"k\":1}"))
    );
}

#[tokio::test]
async fn redirect_off_loop_limit_and_cookie_settings() {
    let base = server().await;
    let opts = Options {
        follow_redirects: false,
        ..Default::default()
    };
    let res = send(
        &req("POST", format!("{base}/login")),
        &Auth::None,
        &opts,
        &mut vec![],
    )
    .await
    .unwrap();
    assert_eq!((res.status, res.header("location")), (302, Some("/echo")));

    let opts = Options {
        max_redirects: 3,
        ..Default::default()
    };
    let err = send(
        &req("GET", format!("{base}/loop")),
        &Auth::None,
        &opts,
        &mut vec![],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, HttpError::TooManyRedirects(3)));

    let mut jar = vec![];
    let opts = Options {
        store_cookies: false,
        ..Default::default()
    };
    send(
        &req("POST", format!("{base}/login")),
        &Auth::None,
        &opts,
        &mut jar,
    )
    .await
    .unwrap();
    assert!(jar.is_empty());
}

#[tokio::test]
async fn timeout_and_connection_errors() {
    let base = server().await;
    let opts = Options {
        timeout: Duration::from_millis(200),
        ..Default::default()
    };
    let err = send(
        &req("GET", format!("{base}/slow")),
        &Auth::None,
        &opts,
        &mut vec![],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, HttpError::Timeout(200)), "{err:?}");

    let err = send(
        &req("GET", "http://127.0.0.1:1/".into()),
        &Auth::None,
        &Options::default(),
        &mut vec![],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, HttpError::Network(_)));
    let err = send(
        &req("GET", "http://[::bad".into()),
        &Auth::None,
        &Options::default(),
        &mut vec![],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, HttpError::InvalidUrl(..)));
}

#[tokio::test]
async fn non_2xx_is_a_response_not_an_error_and_scheme_defaults() {
    let base = server().await;
    let res = send(
        &req("GET", format!("{base}/status/404")),
        &Auth::None,
        &Options::default(),
        &mut vec![],
    )
    .await
    .unwrap();
    assert_eq!((res.status, res.status_text.as_str()), (404, "Not Found"));
    let no_scheme = base.trim_start_matches("http://").to_string() + "/echo";
    let res = send(
        &req("GET", no_scheme),
        &Auth::None,
        &Options::default(),
        &mut vec![],
    )
    .await
    .unwrap();
    assert_eq!(res.status, 200);
}

#[test]
fn build_url_raw_query_when_encoding_disabled() {
    let mut r = req("GET", "http://x.io/a?z=1".into());
    r.parameters = vec![KeyValue::new("q", "a b")];
    r.settings = RequestSettings {
        encode_url: false,
        ..Default::default()
    };
    assert_eq!(
        build_url(&r, false).unwrap().as_str(),
        "http://x.io/a?z=1&q=a%20b"
    );
    assert_eq!(
        build_url(&r, true).unwrap().as_str(),
        "http://x.io/a?z=1&q=a+b"
    );
}

#[tokio::test]
async fn digest_oauth1_and_sigv4_end_to_end() {
    let base = server().await;
    let res = send(
        &req("GET", format!("{base}/digest?x=1")),
        &Auth::Digest {
            username: "ada".into(),
            password: "lovelace".into(),
            disabled: false,
        },
        &Options::default(),
        &mut vec![],
    )
    .await
    .unwrap();
    assert_eq!((res.status, res.text().as_str()), (200, "welcome ada"));
    assert!(
        res.timeline
            .iter()
            .any(|t| t.text.contains("Digest challenge"))
    );
    let bad = send(
        &req("GET", format!("{base}/digest")),
        &Auth::Digest {
            username: "ada".into(),
            password: "nope".into(),
            disabled: false,
        },
        &Options::default(),
        &mut vec![],
    )
    .await
    .unwrap();
    assert_eq!(
        bad.status, 401,
        "wrong password: one retry, then the 401 is returned"
    );

    let mut r = req("POST", format!("{base}/echo?a=1"));
    r.body = Body {
        mime_type: Some(mime::FORM.into()),
        params: vec![BodyParam {
            name: "status".into(),
            value: "hi there".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let cfg = irs_core::OAuth1Config {
        consumer_key: "ck".into(),
        consumer_secret: "cs".into(),
        token_key: "tk".into(),
        token_secret: "ts".into(),
        ..Default::default()
    };
    let j = json_of(
        &send(&r, &Auth::OAuth1(cfg), &Options::default(), &mut vec![])
            .await
            .unwrap(),
    );
    let h = j["headers"]["authorization"].as_str().unwrap();
    assert!(
        h.starts_with("OAuth ")
            && h.contains(r#"oauth_consumer_key="ck""#)
            && h.contains("oauth_signature=\""),
        "{h}"
    );

    let iam = irs_core::AwsIamConfig {
        access_key_id: "AKID".into(),
        secret_access_key: "secret".into(),
        region: "eu-west-1".into(),
        service: "execute-api".into(),
        session_token: "sess".into(),
        ..Default::default()
    };
    let mut r = req("PUT", format!("{base}/echo"));
    r.body = Body {
        mime_type: Some(mime::JSON.into()),
        text: Some("{}".into()),
        ..Default::default()
    };
    let j = json_of(
        &send(&r, &Auth::Iam(iam), &Options::default(), &mut vec![])
            .await
            .unwrap(),
    );
    let a = j["headers"]["authorization"].as_str().unwrap();
    assert!(
        a.starts_with("AWS4-HMAC-SHA256 Credential=AKID/")
            && a.contains("/eu-west-1/execute-api/aws4_request"),
        "{a}"
    );
    assert!(j["headers"]["x-amz-date"].is_string());
    assert_eq!(j["headers"]["x-amz-security-token"], "sess");
}

#[tokio::test]
async fn requests_go_through_the_proxy_unless_bypassed() {
    let base = server().await; // the echo server doubles as a forward proxy for plain HTTP
    let opts = Options {
        proxy: Some(base.clone()),
        no_proxy: Some("bypass.invalid".into()),
        ..Default::default()
    };
    let res = send(
        &req("GET", "http://api.example.invalid/echo?via=proxy".into()),
        &Auth::None,
        &opts,
        &mut vec![],
    )
    .await
    .unwrap();
    let j = json_of(&res);
    assert_eq!(
        (j["path"].as_str(), j["query"].as_str()),
        (Some("/echo"), Some("via=proxy"))
    );
    assert_eq!(
        j["headers"]["host"], "api.example.invalid",
        "absolute-form request reached the proxy"
    );
    assert!(
        res.timeline
            .iter()
            .any(|t| t.text.starts_with("Using proxy"))
    );
    let err = send(
        &req("GET", "http://bypass.invalid/echo".into()),
        &Auth::None,
        &opts,
        &mut vec![],
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, HttpError::Network(_)),
        "no_proxy host is resolved directly (and fails)"
    );
}
