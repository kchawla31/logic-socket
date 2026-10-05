//! Script runtime tests. Cases marked "ported" mirror tests in
//! packages/insomnia-scripting-environment/src/objects/__tests__.

use super::*;
use serde_json::json;

fn vars(v: Json) -> VarMap {
    serde_json::from_value(v).unwrap()
}

fn input(script: &str) -> ScriptInput {
    ScriptInput {
        script: script.into(),
        event: Some(Event::PreRequest),
        request: ScriptRequest {
            id: "req_1".into(),
            name: "Get user".into(),
            method: "GET".into(),
            url: "https://api.example.com/users/{{ _.id }}".into(),
            query: vec![ScriptKv {
                key: "page".into(),
                value: "1".into(),
                disabled: false,
            }],
            headers: vec![ScriptKv {
                key: "Accept".into(),
                value: "application/json".into(),
                disabled: false,
            }],
            body: json!({}),
            auth: json!({ "type": "inherit" }),
        },
        response: None,
        environment: NamedVars {
            name: "Staging".into(),
            data: vars(json!({ "value": "subEnv-value", "token": "t1" })),
        },
        base_environment: NamedVars {
            name: "Base".into(),
            data: vars(json!({ "value": "base-value", "host": "example.com" })),
        },
        globals: NamedVars {
            name: "Globals".into(),
            data: vars(json!({ "value": "global-value" })),
        },
        iteration_data: VarMap::new(),
        local_variables: VarMap::new(),
        folders: vec![],
        cookies: vec![],
        info: Info {
            iteration: 1,
            iteration_count: 1,
            request_name: "Get user".into(),
            request_id: "req_1".into(),
            ..Default::default()
        },
        location: vec!["My Collection".into()],
    }
}

fn after(script: &str, code: u16, body: &str) -> ScriptInput {
    let mut i = input(script);
    i.event = Some(Event::AfterResponse);
    i.response = Some(ScriptResponseData {
        code,
        status: if code == 200 {
            "OK".into()
        } else {
            "Not Found".into()
        },
        headers: vec![ScriptKv {
            key: "Content-Type".into(),
            value: "application/json".into(),
            disabled: false,
        }],
        body: body.into(),
        response_time: 12.0,
    });
    i
}

async fn exec(i: ScriptInput) -> ScriptOutput {
    run(
        i,
        Limits::default(),
        Arc::new(NoNetwork),
        Arc::new(Renderer::new()),
    )
    .await
}

fn ok(o: &ScriptOutput) {
    assert!(o.error.is_none(), "script error: {:?}", o.error);
}

#[tokio::test]
async fn environment_get_set_unset_and_persist() {
    // ported: environments.test.ts "test basic operations"
    let o = exec(input(
        r#"
        insomnia.environment.set('newKey', 'v');
        insomnia.environment.set('n', 42);
        insomnia.environment.unset('token');
        insomnia.baseEnvironment.set('fromScript', true);
        insomnia.collectionVariables.set('alias', 'base');
        insomnia.globals.set('g', 'x');
        if (!insomnia.environment.has('newKey')) throw new Error('has failed');
        if (insomnia.environment.name !== 'Staging') throw new Error('name');
        "#,
    ))
    .await;
    ok(&o);
    assert_eq!(o.environment.get("newKey"), Some(&json!("v")));
    assert_eq!(o.environment.get("n"), Some(&json!(42)));
    assert!(!o.environment.contains_key("token"));
    assert_eq!(o.base_environment.get("fromScript"), Some(&json!(true)));
    assert_eq!(o.base_environment.get("alias"), Some(&json!("base")));
    assert_eq!(o.globals.get("g"), Some(&json!("x")));
}

#[tokio::test]
async fn variables_precedence_matches_insomnia() {
    // ported: environments.test.ts "test environment overriding"
    let mut i = input(
        r#"
        const v = insomnia.variables;
        insomnia.environment.set('r1', v.get('value'));
        v.set('value', 'local-value');
        insomnia.environment.set('r2', v.get('value'));
        insomnia.environment.set('r3', v.get('host'));
        insomnia.environment.set('r4', v.replaceIn('{{ host }}/{{value}}'));
        "#,
    );
    i.iteration_data = vars(json!({ "value": "iterationData-value" }));
    i.folders = vec![
        FolderVars {
            name: "f1".into(),
            environment: vars(json!({ "value": "folderLevel1-value" })),
        },
        FolderVars {
            name: "f2".into(),
            environment: vars(json!({ "value": "folderLevel2-value" })),
        },
    ];
    let o = exec(i).await;
    ok(&o);
    assert_eq!(o.environment["r1"], json!("folderLevel2-value"));
    assert_eq!(o.environment["r2"], json!("local-value"));
    assert_eq!(o.environment["r3"], json!("example.com"));
    assert_eq!(o.environment["r4"], json!("example.com/local-value"));
    assert_eq!(o.local_variables["value"], json!("local-value"));
}

#[tokio::test]
async fn request_url_query_headers_method_mutations() {
    // ported: request.test.ts / urls.test.ts basics
    let o = exec(input(
        r#"
        const r = insomnia.request;
        if (r.url.toString() !== 'https://api.example.com/users/{{ _.id }}?page=1') throw new Error(r.url.toString());
        if (r.url.getHost() !== 'api.example.com') throw new Error('host ' + r.url.getHost());
        r.url.query.upsert({ key: 'page', value: '2' });
        r.url.addQueryParams('sort=asc&q=a b');
        r.headers.add({ key: 'X-Trace', value: 'abc' });
        r.headers.upsert({ key: 'accept', value: 'text/plain' });
        r.addHeader('X-Removed: 1');
        r.removeHeader('X-Removed');
        r.method = 'post';
        "#,
    ))
    .await;
    ok(&o);
    let r = o.request.unwrap();
    assert_eq!(r.method, "POST");
    assert_eq!(
        r.query
            .iter()
            .map(|q| (q.key.as_str(), q.value.as_str()))
            .collect::<Vec<_>>(),
        [("page", "2"), ("sort", "asc"), ("q", "a b")]
    );
    assert_eq!(
        r.headers
            .iter()
            .map(|h| (h.key.as_str(), h.value.as_str()))
            .collect::<Vec<_>>(),
        [("Accept", "text/plain"), ("X-Trace", "abc")]
    );
}

#[tokio::test]
async fn request_url_assignment_and_body_and_auth() {
    let o = exec(input(
        r#"
        insomnia.request.url = 'https://other.io/v2/items?limit=5';
        insomnia.request.body.update({ mode: 'raw', raw: JSON.stringify({ a: 1 }), options: { raw: { language: 'json' } } });
        insomnia.request.auth.update({ type: 'bearer', bearer: [{ key: 'token', value: '{{ _.token }}' }] });
        "#,
    ))
    .await;
    ok(&o);
    let r = o.request.unwrap();
    assert_eq!(r.url, "https://other.io/v2/items");
    assert_eq!(r.query[0].key, "limit");
    assert_eq!(r.body["raw"], json!("{\"a\":1}"));
    assert_eq!(
        r.auth,
        json!({ "type": "bearer", "token": "{{ _.token }}", "prefix": null, "disabled": false })
    );
}

#[tokio::test]
async fn tests_and_chai_expect() {
    // ported: test.test.ts
    let o = exec(after(
        r#"
        insomnia.test('status is 200', () => { insomnia.expect(insomnia.response.code).to.eql(200); });
        insomnia.test('fails', () => { insomnia.expect(1).to.equal(2); });
        insomnia.test.skip('skipped', () => {});
        insomnia.test('async passes', async () => { await new Promise(r => setTimeout(r, 10)); insomnia.expect(true).to.be.true; });
        insomnia.test('json body', () => {
          const data = insomnia.response.json();
          insomnia.expect(data).to.have.property('id', 7);
          insomnia.expect(data.tags).to.include('a');
        });
        "#,
        200,
        r#"{"id":7,"tags":["a","b"]}"#,
    ))
    .await;
    ok(&o);
    let by: Vec<(&str, &str)> = o
        .tests
        .iter()
        .map(|t| (t.name.as_str(), t.status.as_str()))
        .collect();
    assert_eq!(
        by,
        [
            ("status is 200", "passed"),
            ("fails", "failed"),
            ("skipped", "skipped"),
            ("json body", "passed"),
            ("async passes", "passed")
        ]
    );
    assert_eq!(o.tests[1].error.as_deref(), Some("expected 1 to equal 2"));
    assert!(o.tests.iter().all(|t| t.category == "after-response"));
}

#[tokio::test]
async fn response_assertions() {
    // ported: response.test.ts assertion helpers
    let o = exec(after(
        r#"
        insomnia.test('to', () => {
          insomnia.response.to.have.status(200);
          insomnia.response.to.have.status('OK');
          insomnia.response.to.have.header('content-type');
          insomnia.response.to.have.header('Content-Type', 'application/json');
          insomnia.response.to.have.jsonBody('user.name', 'ada');
          insomnia.response.to.be.ok;
          insomnia.response.to.be.success;
          insomnia.response.to.be.json;
          insomnia.response.to.not.be.error;
        });
        insomnia.test('negative', () => { insomnia.response.to.have.status(404); });
        "#,
        200,
        r#"{"user":{"name":"ada"}}"#,
    ))
    .await;
    ok(&o);
    assert_eq!(o.tests[0].status, "passed", "{:?}", o.tests[0].error);
    assert_eq!(
        o.tests[1].error.as_deref(),
        Some("expected response to have status code 404 but got 200")
    );
}

#[tokio::test]
async fn execution_skip_and_next_request() {
    // ported: execution.test.ts
    let o = exec(input(
        "insomnia.execution.skipRequest(); insomnia.execution.setNextRequest('Login');",
    ))
    .await;
    ok(&o);
    assert_eq!(
        o.execution,
        Execution {
            skip_request: true,
            next_request: Some("Login".into())
        }
    );
    let o = exec(input("insomnia.execution.setNextRequest(null);")).await;
    assert_eq!(o.execution.next_request.as_deref(), Some("__stop__"));
    let o = exec(after("insomnia.execution.skipRequest();", 200, "")).await;
    assert!(
        o.error
            .unwrap()
            .message
            .contains("only be used in pre-request")
    );
    let o = exec(input(
        "insomnia.environment.set('loc', insomnia.execution.location.join('/'));",
    ))
    .await;
    assert_eq!(o.environment["loc"], json!("My Collection/Get user"));
}

#[tokio::test]
async fn info_and_iteration_data() {
    // ported: request-info.test.ts
    let mut i = input(
        r#"
        insomnia.environment.set('ev', insomnia.info.eventName);
        insomnia.environment.set('it', insomnia.info.iteration);
        insomnia.environment.set('row', insomnia.iterationData.get('email'));
        insomnia.environment.set('rn', insomnia.info.requestName);
        "#,
    );
    i.iteration_data = vars(json!({ "email": "a@b.c" }));
    let o = exec(i).await;
    ok(&o);
    assert_eq!(o.environment["ev"], json!("prerequest"));
    assert_eq!(o.environment["it"], json!(1));
    assert_eq!(o.environment["row"], json!("a@b.c"));
    assert_eq!(o.environment["rn"], json!("Get user"));
}

#[tokio::test]
async fn console_is_captured_with_levels() {
    // ported: console.test.ts
    let o = exec(input(
        "console.log('hi', {a: 1}); console.warn('careful'); console.error(new Error('boom'));",
    ))
    .await;
    ok(&o);
    let lines: Vec<(&str, &str)> = o
        .console
        .iter()
        .map(|c| (c.level.as_str(), c.text.as_str()))
        .collect();
    assert_eq!(
        lines,
        [
            ("log", "hi {\n  \"a\": 1\n}"),
            ("warn", "careful"),
            ("error", "Error: boom")
        ]
    );
}

#[tokio::test]
async fn cookies_jar_operations() {
    // ported: cookies.test.ts (jar set/get/unset)
    let mut i = input(
        r#"
        const jar = insomnia.cookies.jar();
        jar.set('https://api.example.com', 'session', 'abc', (err, c) => { if (err) throw err; });
        jar.get('https://api.example.com/x', 'session', (err, v) => insomnia.environment.set('got', v));
        jar.unset('https://api.example.com', 'old');
        insomnia.environment.set('reqCookie', insomnia.cookies.get('old'));
        "#,
    );
    i.cookies = vec![Cookie {
        name: "old".into(),
        value: "1".into(),
        domain: "api.example.com".into(),
        path: "/".into(),
        ..Default::default()
    }];
    let o = exec(i).await;
    ok(&o);
    assert_eq!(o.environment["got"], json!("abc"));
    assert_eq!(o.environment["reqCookie"], json!("1"));
    let jar = o.cookies.unwrap();
    assert_eq!(jar.len(), 1);
    assert_eq!(
        (jar[0].name.as_str(), jar[0].value.as_str()),
        ("session", "abc")
    );
}

#[tokio::test]
async fn require_vendored_modules() {
    let o = exec(input(
        r#"
        const _ = require('lodash');
        const CryptoJS = require('crypto-js');
        const moment = require('moment');
        const uuid = require('uuid');
        const Ajv = require('ajv');
        const tv4 = require('tv4');
        const chai = require('chai');
        insomnia.environment.set('chunk', _.chunk([1,2,3], 2));
        insomnia.environment.set('globalLodash', globalThis._.sum([1, 2]));
        insomnia.environment.set('sha', CryptoJS.SHA256('abc').toString());
        insomnia.environment.set('hmac', CryptoJS.HmacSHA256('msg', 'key').toString(CryptoJS.enc.Base64));
        insomnia.environment.set('year', moment('2020-05-17').format('YYYY'));
        insomnia.environment.set('uuidOk', uuid.validate(uuid.v4()));
        const ajv = new Ajv();
        insomnia.environment.set('ajv', ajv.validate({ type: 'object', required: ['a'] }, { a: 1 }));
        insomnia.environment.set('tv4', tv4.validate(5, { type: 'number' }));
        insomnia.environment.set('chai', typeof chai.expect);
        insomnia.environment.set('b64', btoa('hi') + '|' + atob('aGk='));
        "#,
    ))
    .await;
    ok(&o);
    let e = &o.environment;
    assert_eq!(e["chunk"], json!([[1, 2], [3]]));
    assert_eq!(e["globalLodash"], json!(3));
    assert_eq!(
        e["sha"],
        json!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    assert_eq!(
        e["hmac"],
        json!("LZPLwb4We8sWN6SiPL/wGnh48MUO6DOVTqUiG7G4xig=")
    );
    assert_eq!(e["year"], json!("2020"));
    assert_eq!(e["uuidOk"], json!(true));
    assert_eq!(e["ajv"], json!(true));
    assert_eq!(e["tv4"], json!(true));
    assert_eq!(e["chai"], json!("function"));
    assert_eq!(e["b64"], json!("aGk=|hi"));
}

#[tokio::test]
async fn unsupported_modules_and_apis_fail_clearly() {
    for (m, needle) in [
        ("fs", "NotSupported: require('fs')"),
        ("cheerio", "NotSupported: require('cheerio')"),
        ("leftpad", "Cannot find module"),
    ] {
        let o = exec(input(&format!("require('{m}');"))).await;
        assert!(
            o.error.as_ref().unwrap().message.contains(needle),
            "{m}: {:?}",
            o.error
        );
    }
    let o = exec(input("insomnia.vault.get('x')")).await;
    assert!(
        o.error
            .unwrap()
            .message
            .contains("NotSupported: insomnia.vault")
    );
    let o = exec(input("fetch('http://x')")).await;
    assert!(
        o.error.unwrap().message.contains("fetch"),
        "no fetch global"
    );
}

#[tokio::test]
async fn errors_report_script_line_and_keep_partial_results() {
    let o = exec(input(
        "insomnia.environment.set('before', 1);\nconsole.log('x');\nundefinedFunction();\n",
    ))
    .await;
    let e = o.error.unwrap();
    assert!(e.message.contains("undefinedFunction"), "{e:?}");
    assert_eq!(e.line, Some(3));
    assert_eq!(
        o.environment["before"],
        json!(1),
        "mutations before the error are kept"
    );
    assert_eq!(o.console.len(), 1);

    let o = exec(input("let x = ;")).await;
    let e = o.error.unwrap();
    assert!(e.message.to_lowercase().contains("unexpected"), "{e:?}");
    assert_eq!(e.line, Some(1));
}

#[tokio::test]
async fn sandbox_kills_infinite_loops() {
    let limits = Limits {
        timeout: Duration::from_millis(300),
        ..Default::default()
    };
    let t = Instant::now();
    let o = run(
        input("while (true) {}"),
        limits,
        Arc::new(NoNetwork),
        Arc::new(Renderer::new()),
    )
    .await;
    assert!(t.elapsed() < Duration::from_secs(3));
    assert!(o.error.unwrap().message.contains("timed out"));

    // an await that never resolves is cut off by the outer timeout
    let t = Instant::now();
    let o = run(
        input("await new Promise(() => {});"),
        limits,
        Arc::new(NoNetwork),
        Arc::new(Renderer::new()),
    )
    .await;
    assert!(t.elapsed() < Duration::from_secs(3));
    assert!(o.error.is_some());
}

#[tokio::test]
async fn sandbox_contains_memory_exhaustion() {
    let limits = Limits {
        memory_bytes: 16 * 1024 * 1024,
        ..Default::default()
    };
    let o = run(
        input("const a = []; while (true) a.push('x'.repeat(1024 * 64));"),
        limits,
        Arc::new(NoNetwork),
        Arc::new(Renderer::new()),
    )
    .await;
    let msg = o.error.unwrap().message;
    assert!(msg.contains("memory") || msg.contains("timed out"), "{msg}");
}

struct Echo;
impl Host for Echo {
    fn send(&self, req: HostRequest) -> BoxFuture<'static, Result<ScriptResponseData, String>> {
        Box::pin(async move {
            if req.url.contains("fail") {
                return Err("connection refused".into());
            }
            Ok(ScriptResponseData {
                code: 201,
                status: "Created".into(),
                headers: vec![ScriptKv {
                    key: "X-Echo".into(),
                    value: req.method.clone(),
                    disabled: false,
                }],
                body:
                    serde_json::json!({ "url": req.url, "body": req.body, "headers": req.headers })
                        .to_string(),
                response_time: 1.0,
            })
        })
    }
}

#[tokio::test]
async fn send_request_callback_and_promise() {
    let script = r#"
        const r1 = await insomnia.sendRequest('https://x.io/a');
        insomnia.environment.set('code', r1.code);
        await new Promise(resolve => insomnia.sendRequest({
            url: 'https://x.io/b', method: 'post',
            header: [{ key: 'X-A', value: '1' }],
            body: { mode: 'raw', raw: 'hello' },
        }, (err, res) => {
            insomnia.environment.set('echo', res.json().body);
            insomnia.environment.set('hdr', res.headers.get('x-echo'));
            resolve();
        }));
        await new Promise(resolve => insomnia.sendRequest('https://fail.io', (err, res) => {
            insomnia.environment.set('err', err);
            resolve();
        }));
        try { await insomnia.sendRequest('https://fail.io'); } catch (e) { insomnia.environment.set('rejected', e.message); }
    "#;
    let o = run(
        input(script),
        Limits::default(),
        Arc::new(Echo),
        Arc::new(Renderer::new()),
    )
    .await;
    ok(&o);
    assert_eq!(o.environment["code"], json!(201));
    assert_eq!(o.environment["echo"], json!("hello"));
    assert_eq!(o.environment["hdr"], json!("POST"));
    assert_eq!(o.environment["err"], json!("connection refused"));
    assert_eq!(o.environment["rejected"], json!("connection refused"));
}

#[tokio::test]
async fn timers_run_before_finish_and_clear_timeout_works() {
    let o = exec(input(
        r#"
        setTimeout(() => insomnia.environment.set('late', 'yes'), 20);
        const id = setTimeout(() => insomnia.environment.set('cancelled', 'no'), 10);
        clearTimeout(id);
        "#,
    ))
    .await;
    ok(&o);
    assert_eq!(o.environment.get("late"), Some(&json!("yes")));
    assert!(!o.environment.contains_key("cancelled"));
}

#[tokio::test]
async fn dollar_and_pm_aliases_and_parent_folders() {
    let mut i = input(
        "$.environment.set('a', 1); pm.environment.set('b', 2); insomnia.environment.set('f', insomnia.parentFolders.get('Auth').environment.get('scope'));",
    );
    i.folders = vec![FolderVars {
        name: "Auth".into(),
        environment: vars(json!({ "scope": "admin" })),
    }];
    let o = exec(i).await;
    ok(&o);
    assert_eq!(
        (
            o.environment["a"].clone(),
            o.environment["b"].clone(),
            o.environment["f"].clone()
        ),
        (json!(1), json!(2), json!("admin"))
    );
}

#[tokio::test]
async fn each_vendored_module_loads_alone() {
    for m in [
        "chai",
        "lodash",
        "crypto-js",
        "moment",
        "tv4",
        "ajv",
        "uuid",
    ] {
        let o = exec(input(&format!(
            "const m = require('{m}'); if (!m) throw new Error('empty');"
        )))
        .await;
        assert!(o.error.is_none(), "{m}: {:?}", o.error);
    }
}

#[tokio::test]
async fn oauth2_auth_is_preserved_through_scripts() {
    let mut i = input("insomnia.environment.set('t', insomnia.request.auth.type);");
    i.request.auth =
        json!({"type": "oauth2", "grantType": "client_credentials", "clientId": "app"});
    let o = exec(i).await;
    ok(&o);
    assert_eq!(o.environment["t"], json!("oauth2"));
    assert_eq!(
        o.request.unwrap().auth,
        json!({"type": "oauth2", "grantType": "client_credentials", "clientId": "app"})
    );
}
