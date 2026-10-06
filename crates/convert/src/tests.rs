use lsock_core::*;
use serde_json::json;

use super::Meta;
use super::*;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn one(name: &str) -> (WorkspaceBundle, Vec<String>) {
    let i = import(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(i.workspaces.len(), 1, "{name}");
    (i.workspaces.into_iter().next().unwrap(), i.warnings)
}

fn requests(b: &WorkspaceBundle) -> Vec<&Request> {
    b.walk()
        .into_iter()
        .filter_map(|i| match &i.node {
            Node::Request(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn find<'a>(b: &'a WorkspaceBundle, name: &str) -> &'a Request {
    requests(b)
        .into_iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("no request {name}"))
}

#[test]
fn every_fixture_is_detected() {
    let cases = [
        ("insomnia4-basic.yaml", Format::InsomniaV4),
        ("variable_inheritance.yaml", Format::InsomniaV5),
        ("v5-spec-with-tests.yaml", Format::InsomniaV5),
        ("postman-complex-v2_0.json", Format::PostmanCollection),
        ("postman-complex-v2_1.json", Format::PostmanCollection),
        ("postman-env.json", Format::PostmanEnvironment),
        ("openapi3-petstore.json", Format::OpenApi3),
        ("openapi3-security.yaml", Format::OpenApi3),
        ("swagger2-petstore.json", Format::Swagger2),
        ("har-deep.json", Format::Har),
        ("har-form.json", Format::Har),
    ];
    for (f, want) in cases {
        assert_eq!(detect(&fixture(f)), Some(want), "{f}");
    }
    assert_eq!(detect("curl https://x.io"), Some(Format::Curl));
    assert_eq!(detect("hello: world"), None);
    assert!(matches!(import("{}"), Err(ConvertError::Unknown)));
}

// ------------------------------------------------------------------ Postman

#[test]
fn postman_collections_v20_and_v21() {
    for f in ["postman-complex-v2_0.json", "postman-complex-v2_1.json"] {
        let (b, _) = one(f);
        assert_eq!(b.workspace.name, "Complex Test Collection");
        let Node::Folder(folder, kids) = &b.items[0].node else {
            panic!("{f}: folder first")
        };
        assert_eq!(folder.name, "First Folder");
        assert_eq!(kids.len(), 2);
        let Node::Request(multipart) = &kids[0].node else {
            panic!()
        };
        assert_eq!(multipart.url, "{{ base_url }}/api/users");
        assert_eq!(multipart.body.mime_type.as_deref(), Some(mime::MULTIPART));
        assert!(!multipart.body.params.is_empty());
        let gql = find(&b, "Test Request GraphQL Body");
        assert_eq!(gql.body.mime_type.as_deref(), Some(mime::GRAPHQL));
        let v: serde_json::Value = serde_json::from_str(gql.body.text.as_deref().unwrap()).unwrap();
        assert!(
            v["query"]
                .as_str()
                .unwrap()
                .starts_with("mutation loginUser")
        );
        assert!(
            v["variables"].is_object(),
            "variables parsed from Postman's string form"
        );
        assert!(
            gql.headers
                .iter()
                .any(|h| h.name == "Content-Type" && h.value == mime::JSON)
        );
    }
}

#[test]
fn postman_auth_kinds() {
    let (b, _) = one("postman-basic-auth-v2_0.json");
    assert!(
        matches!(&requests(&b)[0].authentication, Auth::Basic { username, .. } if username == "basic-username")
    );
    let (b, _) = one("postman-api-key-header-v2_1.json");
    assert!(
        matches!(&requests(&b)[0].authentication, Auth::ApiKey { key, add_to: Some(t), .. } if key == "test" && t == "header")
    );
    let (b, _) = one("postman-aws-signature-auth-v2_1.json");
    assert!(
        matches!(&requests(&b)[0].authentication, Auth::Iam(c) if c.region == "aws-region" && c.service == "aws-service-name")
    );
    let (b, w) = one("postman-oauth2_0-auth-v2_1.json");
    let Auth::OAuth2(pkce) = &find(&b, "pkce").authentication else {
        panic!()
    };
    assert!(pkce.use_pkce);
    assert_eq!(
        pkce.audience, "test",
        "object-valued params use their value"
    );
    let Auth::OAuth2(code) = &find(&b, "auth code").authentication else {
        panic!()
    };
    assert!(!code.use_pkce);
    assert!(
        matches!(&find(&b, "password").authentication, Auth::OAuth2(c) if c.grant_type == "password" && c.username == "test")
    );
    assert!(
        matches!(&find(&b, "client").authentication, Auth::OAuth2(c) if c.grant_type == "client_credentials")
    );
    assert_eq!(w.len(), 1, "implicit grant warned: {w:?}");
}

#[test]
fn postman_variables_scripts_and_faker() {
    let (b, _) = one("postman-scripts-import-v2_1.json");
    assert_eq!(
        b.base_env.as_ref().unwrap().body.data["dssx"],
        json!("{{ _['env-var-in-global'] }}")
    );
    let Node::Folder(root, _) = &b.items[0].node else {
        panic!("collection scripts live on a root folder")
    };
    assert_eq!(
        root.pre_request_script.as_deref(),
        Some("console.log('pre')")
    );
    let (b, _) = one("postman-faker-vars-v2_1.json");
    assert!(requests(&b)[0].url.contains("{% uuid 'v4' %}"));
    assert_eq!(
        postman::vars_in("{{$randomEmail}} {{a-b}} {{ok}}"),
        "{% faker 'randomEmail' %} {{ _['a-b'] }} {{ ok }}"
    );
    assert_eq!(
        postman::vars_out("{{ _.host }}/{{ _['a-b'] }}/{% uuid 'v4' %}/{% faker 'randomInt' %}"),
        "{{host}}/{{a-b}}/{{$guid}}/{{$randomInt}}"
    );
}

#[test]
fn postman_script_translation_matches_insomnia_rules() {
    let t = postman::translate_script;
    assert_eq!(
        t("tests['ok'] = responseCode.code === 200;"),
        "insomnia.test('ok', function() { insomnia.expect(insomnia.response.code === 200).to.be.true; });"
    );
    assert_eq!(
        t("environment.token = 'abc';"),
        "insomnia.environment.set('token', 'abc');"
    );
    assert_eq!(
        t("var x = globals.foo;"),
        "var x = insomnia.globals.get('foo');"
    );
    assert_eq!(
        t("postman.setEnvironmentVariable('a', 1)"),
        "insomnia.environment.set('a', 1)"
    );
    assert_eq!(
        t("postman.clearEnvironmentVariable()"),
        "insomnia.environment.clear()"
    );
    assert_eq!(
        t("const b = JSON.parse(responseBody);"),
        "const b = JSON.parse(insomnia.response.text());"
    );
    assert_eq!(
        t("pm.expect(pm.response.code).to.eql(200)"),
        "insomnia.expect(insomnia.response.code).to.eql(200)"
    );
    // not touched: other identifiers ending in pm/environment, strings and properties
    assert_eq!(
        t("upm.x; obj.environment.y; 'pm.z'"),
        "upm.x; obj.environment.y; 'pm.z'"
    );
    assert_eq!(
        postman::script_to_postman("insomnia.test('a', () => insomnia.expect(1).to.eql(1))"),
        "pm.test('a', () => pm.expect(1).to.eql(1))"
    );
}

#[test]
fn postman_environment_and_secrets() {
    let (b, _) = one("postman-env.json");
    assert_eq!(b.workspace.scope, WorkspaceScope::Environment);
    assert_eq!(b.sub_envs[0].body.data["foo"], json!("production"));
    let env = json!({"name": "Prod", "values": [
        {"key": "host", "value": "x.io", "enabled": true},
        {"key": "token", "value": "s3cr3t", "type": "secret", "enabled": true},
        {"key": "off", "value": "1", "enabled": false},
    ]});
    let i = import(&env.to_string()).unwrap();
    let sub = &i.workspaces[0].sub_envs[0].body;
    assert_eq!(sub.secret_keys, vec!["token"]);
    assert!(!sub.data.contains_key("off"));
    let back: serde_json::Value = serde_json::from_str(&postman::export_environment(sub)).unwrap();
    assert_eq!(back["values"][1]["type"], "secret");
}

#[test]
fn postman_export_round_trips() {
    let (b, _) = one("postman-complex-v2_1.json");
    let (text, skipped) = postman::export_collection(&b);
    assert_eq!(skipped, 0);
    let again = import(&text).unwrap();
    assert_eq!(again.format, Format::PostmanCollection);
    let a: Vec<_> = requests(&b)
        .iter()
        .map(|r| {
            (
                r.name.clone(),
                r.method.clone(),
                r.url.clone(),
                r.body.clone(),
            )
        })
        .collect();
    let b2 = &again.workspaces[0];
    let c: Vec<_> = requests(b2)
        .iter()
        .map(|r| {
            (
                r.name.clone(),
                r.method.clone(),
                r.url.clone(),
                r.body.clone(),
            )
        })
        .collect();
    assert_eq!(a, c);
    // auth + scripts survive too
    let mut w = WorkspaceBundle::default();
    w.items.push(Item::new(Node::Request(Request {
        name: "Signed".into(),
        url: "{{ _.base }}/x".into(),
        authentication: Auth::Bearer {
            token: "{{ _.tok }}".into(),
            prefix: None,
            disabled: false,
        },
        parameters: vec![KeyValue::new("q", "1")],
        after_response_script: Some("insomnia.test('ok', () => {});".into()),
        ..Default::default()
    })));
    let (text, _) = postman::export_collection(&w);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["item"][0]["request"]["url"]["raw"], "{{base}}/x?q=1");
    assert_eq!(
        v["item"][0]["event"][0]["script"]["exec"][0],
        "pm.test('ok', () => {});"
    );
    let r = import(&text).unwrap().workspaces.remove(0);
    let back = requests(&r)[0];
    assert_eq!(back.url, "{{ base }}/x");
    assert_eq!(back.parameters, vec![KeyValue::new("q", "1")]);
    assert!(matches!(&back.authentication, Auth::Bearer { token, .. } if token == "{{ tok }}"));
    assert_eq!(
        back.after_response_script.as_deref(),
        Some("insomnia.test('ok', () => {});")
    );
}

// ------------------------------------------------------------------ OpenAPI / Swagger

#[test]
fn openapi3_petstore() {
    let (b, w) = one("openapi3-petstore.json");
    assert_eq!(b.workspace.name, "Swagger Petstore 1.0.0");
    assert_eq!(
        b.base_env.as_ref().unwrap().body.data["base_url"],
        json!("http://petstore.swagger.io/v2")
    );
    let folders: Vec<_> = b.items.iter().map(|i| i.node.name().to_string()).collect();
    assert_eq!(folders, ["pet", "store", "user"]);
    let get = find(&b, "Find pet by ID");
    assert_eq!(get.url, "{{ _.base_url }}/pet/:petId");
    assert_eq!(get.path_parameters, vec![KeyValue::new("petId", "0")]);
    assert!(matches!(&get.authentication, Auth::ApiKey { key, .. } if key == "api_key"));
    let order = find(&b, "Place an order for a pet");
    assert_eq!(
        order.body.mime_type.as_deref(),
        Some(mime::JSON),
        "*/* becomes JSON"
    );
    let body: serde_json::Value =
        serde_json::from_str(order.body.text.as_deref().unwrap()).unwrap();
    assert_eq!(body["shipDate"], "2024-01-01T00:00:00Z");
    let login = find(&b, "Logs user into the system");
    assert_eq!(
        login
            .parameters
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["username", "password"]
    );
    assert_eq!(
        w.len(),
        1,
        "one warning per scheme, not per operation: {w:?}"
    );
    assert!(b.spec.as_deref().unwrap().contains("\"openapi\""));
}

#[test]
fn openapi3_security_schemes_and_server_variables() {
    let (b, _) = one("openapi3-security.yaml");
    assert!(matches!(find(&b, "GET /none").authentication, Auth::None));
    assert!(
        matches!(&find(&b, "GET /bearer").authentication, Auth::Bearer { token, .. } if token == "{{ _.bearer_token }}")
    );
    assert!(
        matches!(&find(&b, "GET /key/query").authentication, Auth::ApiKey { add_to: Some(t), .. } if t == "queryParams")
    );
    assert!(
        matches!(&find(&b, "GET /key/cookie").authentication, Auth::ApiKey { add_to: Some(t), .. } if t == "cookie")
    );
    assert!(
        matches!(&find(&b, "GET /oauth2/client-credentials").authentication,
        Auth::OAuth2(c) if c.grant_type == "client_credentials" && c.access_token_url == "https://api.server.test/v1/token" && c.scope == "read:something write:something")
    );
    let env = &b.base_env.as_ref().unwrap().body.data;
    for k in [
        "http_username",
        "bearer_token",
        "oauth2_client_id",
        "oauth2_password",
    ] {
        assert!(env.contains_key(k), "{k} placeholder");
    }
    let (b, _) = one("openapi3-server-vars.yaml");
    let env = &b.base_env.as_ref().unwrap().body.data;
    assert_eq!(
        env["base_url"],
        json!("{{ _.protocol }}://{{ _.host }}:{{ _.port }}/{{ _.basePath }}")
    );
    assert_eq!(env["port"], json!("8080"));
}

#[test]
fn swagger2_petstore() {
    let (b, _) = one("swagger2-petstore.json");
    assert_eq!(
        b.base_env.as_ref().unwrap().body.data["base_url"],
        json!("http://petstore.swagger.io/v2")
    );
    let form = find(&b, "Updates a pet in the store with form data");
    assert_eq!(form.body.mime_type.as_deref(), Some(mime::FORM));
    assert_eq!(
        form.body
            .params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["name", "status"]
    );
    let upload = find(&b, "uploads an image");
    assert_eq!(upload.body.mime_type.as_deref(), Some(mime::MULTIPART));
    assert!(
        upload
            .body
            .params
            .iter()
            .any(|p| p.kind.as_deref() == Some("file"))
    );
    assert!(
        find(&b, "Create user")
            .body
            .text
            .as_deref()
            .unwrap()
            .contains("\"username\"")
    );
}

// ------------------------------------------------------------------ HAR / cURL

#[test]
fn har_and_curl() {
    let (b, _) = one("har-deep.json");
    let r = find(&b, "POST /foo/bar");
    assert_eq!(r.parameters, vec![KeyValue::new("foo", "bar")]);
    assert_eq!(r.body.text.as_deref(), Some("hello world!"));
    let (b, _) = one("har-form.json");
    let r = requests(&b)[0];
    assert_eq!(r.body.mime_type.as_deref(), Some(mime::MULTIPART));
    assert_eq!(
        r.body.params[0].file_name.as_deref(),
        Some("/home/user/test.txt")
    );
    let again = import(&har::export(&b)).unwrap();
    assert_eq!(requests(&again.workspaces[0])[0].body.params.len(), 2);

    let text = "# two calls\ncurl https://api.x.io/users -H 'Accept: application/json'\ncurl -X POST https://api.x.io/users \\\n  -d '{\"a\":1}' \\\n  -H 'Content-Type: application/json'\n";
    let i = import(text).unwrap();
    assert_eq!(i.format, Format::Curl);
    let rs = requests(&i.workspaces[0]);
    assert_eq!(
        rs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["GET /users", "POST /users"]
    );
    assert_eq!(rs[1].body.text.as_deref(), Some("{\"a\":1}"));
}

// ------------------------------------------------------------------ Insomnia

#[test]
fn insomnia_v5_fixtures_round_trip() {
    for f in [
        "variable_inheritance.yaml",
        "collection_runner.yaml",
        "operations.yaml",
        "v5-spec-with-tests.yaml",
    ] {
        let (b, _) = one(f);
        assert!(b.request_count() > 0, "{f}");
        let text = insomnia::export_v5(&b).unwrap();
        let (again, _) = {
            let i = import(&text).unwrap();
            (i.workspaces.into_iter().next().unwrap(), i.warnings)
        };
        let mut a = b.clone();
        a.spec = None; // the spec isn't stored, so it isn't re-exported
        let mut c = again.clone();
        c.spec = None;
        assert_eq!(a, c, "{f} round trip");
    }
    let (b, _) = one("variable_inheritance.yaml");
    let Node::Folder(_, kids) = &b.items[0].node else {
        panic!()
    };
    let Node::Folder(parent, _) = &kids[0].node else {
        panic!()
    };
    assert_eq!(parent.name, "parent");
    let (b, _) = one("collection_runner.yaml");
    assert!(
        find(&b, "01 Request")
            .after_response_script
            .as_deref()
            .unwrap()
            .contains("insomnia.test")
    );
}

fn full_bundle() -> WorkspaceBundle {
    let meta = |id: &str, k: f64| Meta {
        id: Some(id.into()),
        sort_key: Some(k),
        created: Some(1),
        modified: Some(2),
    };
    let mut data = VarMap::new();
    data.insert("base".into(), json!("https://x.io"));
    data.insert("token".into(), json!(""));
    WorkspaceBundle {
        meta: Meta {
            id: Some("wrk_1".into()),
            sort_key: None,
            created: Some(1),
            modified: Some(2),
        },
        workspace: Workspace {
            name: "Everything".into(),
            description: "all kinds".into(),
            ..Default::default()
        },
        base_env: Some(Entry {
            meta: Meta {
                sort_key: None,
                ..meta("env_1", 0.0)
            },
            body: Environment {
                name: "Base".into(),
                data,
                secret_keys: vec!["token".into()],
                ..Default::default()
            },
        }),
        sub_envs: vec![Entry {
            meta: meta("env_2", 1.0),
            body: Environment {
                name: "Prod".into(),
                color: Some("#f00".into()),
                is_private: true,
                ..Default::default()
            },
        }],
        cookie_jar: Some(Entry {
            meta: Meta {
                sort_key: None,
                ..meta("jar_1", 0.0)
            },
            body: CookieJar {
                name: "Default Jar".into(),
                cookies: vec![Cookie {
                    name: "sid".into(),
                    value: "1".into(),
                    domain: "x.io".into(),
                    path: "/".into(),
                    expires: Some(1_900_000_000_000),
                    secure: true,
                    http_only: true,
                    host_only: false,
                }],
            },
        }),
        items: vec![
            Item {
                meta: meta("fld_1", 1.0),
                node: Node::Folder(
                    Folder {
                        name: "Folder".into(),
                        description: "folder docs".into(),
                        headers: vec![KeyValue::new("X-Team", "core")],
                        authentication: Auth::Basic {
                            username: "u".into(),
                            password: "p".into(),
                            disabled: false,
                        },
                        pre_request_script: Some("insomnia.environment.set('a', 1);".into()),
                        ..Default::default()
                    },
                    vec![
                        Item {
                            meta: meta("req_1", 1.0),
                            node: Node::Request(Request {
                                name: "Create".into(),
                                method: "POST".into(),
                                url: "{{ _.base }}/items/:id".into(),
                                description: "makes one".into(),
                                path_parameters: vec![KeyValue::new("id", "7")],
                                parameters: vec![KeyValue {
                                    name: "dry".into(),
                                    value: "1".into(),
                                    disabled: true,
                                    ..Default::default()
                                }],
                                body: Body {
                                    mime_type: Some(mime::JSON.into()),
                                    text: Some("{\"a\":1}".into()),
                                    ..Default::default()
                                },
                                settings: RequestSettings {
                                    disable_user_agent: true,
                                    follow_redirects: Toggle::Off,
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        },
                        Item {
                            meta: meta("llm_1", 2.0),
                            node: Node::Llm(LlmRequest {
                                name: "Summarise".into(),
                                model: "m".into(),
                                ..Default::default()
                            }),
                        },
                    ],
                ),
            },
            Item {
                meta: meta("rt_1", 2.0),
                node: Node::Realtime(RealtimeRequest {
                    name: "Socket".into(),
                    payload: "{\"hi\":1}".into(),
                    subprotocols: vec!["v1".into()],
                    ..Default::default()
                }),
            },
            Item {
                meta: meta("rt_2", 3.0),
                node: Node::Realtime(RealtimeRequest {
                    name: "Chat".into(),
                    kind: "socketio".into(),
                    event: "chat".into(),
                    namespace: "/room".into(),
                    ..Default::default()
                }),
            },
            Item {
                meta: meta("rt_3", 4.0),
                node: Node::Realtime(RealtimeRequest {
                    name: "Events".into(),
                    kind: "sse".into(),
                    url: "https://x.io/events".into(),
                    ..Default::default()
                }),
            },
            Item {
                meta: meta("grpc_1", 5.0),
                node: Node::Grpc(GrpcRequest {
                    name: "Hello".into(),
                    method: "/demo.Greeter/SayHello".into(),
                    schema_source: "protos".into(),
                    message: "{\"name\":\"a\"}".into(),
                    timeout_ms: 5000,
                    ..Default::default()
                }),
            },
            Item {
                meta: meta("mcp_1", 6.0),
                node: Node::Mcp(McpServer {
                    name: "Tools".into(),
                    transport: McpTransport::Stdio {
                        command: "npx".into(),
                        args: vec!["-y".into(), "server everything".into()],
                        cwd: None,
                    },
                    ..Default::default()
                }),
            },
        ],
        protos: vec![Entry {
            meta: Meta::with_id("pf_1"),
            body: ProtoFile {
                name: "demo.proto".into(),
                contents: "syntax = \"proto3\";".into(),
            },
        }],
        spec: None,
    }
}

#[test]
fn our_own_v5_export_round_trips_every_item_kind() {
    let b = full_bundle();
    let text = insomnia::export_v5(&b).unwrap();
    let again = import(&text).unwrap();
    assert_eq!(again.format, Format::InsomniaV5);
    assert!(again.warnings.is_empty(), "{:?}", again.warnings);
    assert_eq!(again.workspaces[0], b);
}

#[test]
fn v5_export_stays_loadable_by_insomnia() {
    let text = insomnia::export_v5(&full_bundle()).unwrap();
    let v: serde_json::Value = serde_yaml_ng::from_str(&text).unwrap();
    assert_eq!(v["type"], "collection.insomnia.rest/5.0");
    // Insomnia tells item kinds apart by shape and id prefix
    let coll = v["collection"].as_array().unwrap();
    assert!(coll.iter().any(|i| i["meta"]["id"] == "ws-req_rt_1"));
    assert!(coll.iter().any(|i| i["meta"]["id"] == "socketio-req_rt_2"));
    assert!(
        coll.iter()
            .any(|i| i["reflectionApi"].is_object() && i["url"] == "localhost:50051")
    );
    let sse = coll.iter().find(|i| i["name"] == "Events").unwrap();
    assert!(
        sse["headers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["value"] == "text/event-stream")
    );
    // AI requests and MCP servers live in our extension, not in `collection`
    let names: Vec<_> = v["x-logic-socket"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["LlmRequest", "McpServer"]);
    assert_eq!(v["x-logic-socket"]["items"][0]["parentId"], "fld_1");
    // inherit auth is Insomnia's `{}`, which the exporter prunes
    assert!(
        coll.iter()
            .find(|i| i["name"] == "Hello")
            .unwrap()
            .get("authentication")
            .is_none()
    );
}

#[test]
fn mcp_client_workspace_exports_as_mcp_client_file() {
    let mut b = WorkspaceBundle {
        workspace: Workspace {
            name: "Tools".into(),
            scope: WorkspaceScope::Mcp,
            ..Default::default()
        },
        ..Default::default()
    };
    let server = McpServer {
        name: "Tools".into(),
        transport: McpTransport::StreamableHttp {
            url: "http://localhost:3333/mcp".into(),
        },
        sampling: McpSampling {
            enabled: true,
            max_tokens: 99,
            ..Default::default()
        },
        ..Default::default()
    };
    b.items.push(Item {
        meta: Meta::with_id("mcp_9"),
        node: Node::Mcp(server.clone()),
    });
    let text = insomnia::export_v5(&b).unwrap();
    assert!(text.contains("type: mcpClient.insomnia/5.0"));
    assert!(text.contains("transportType: streamable-http"));
    let again = import(&text).unwrap().workspaces.remove(0);
    assert_eq!(again.workspace.scope, WorkspaceScope::Mcp);
    assert!(matches!(&again.items[0].node, Node::Mcp(s) if *s == server));
}

#[test]
fn insomnia_v4_export() {
    let v4 = json!({
        "_type": "export", "__export_format": 4, "__export_source": "insomnia.desktop.app:v8",
        "resources": [
            {"_id": "wrk_a", "_type": "workspace", "name": "Legacy", "scope": "collection", "description": "old"},
            {"_id": "env_base", "_type": "environment", "parentId": "wrk_a", "name": "Base", "data": {"host": "x.io"}},
            {"_id": "env_prod", "_type": "environment", "parentId": "env_base", "name": "Prod", "data": {"host": "prod.x.io"}, "isPrivate": true},
            {"_id": "jar_a", "_type": "cookie_jar", "parentId": "wrk_a", "name": "Jar", "cookies": [{"key": "a", "value": "1", "domain": "x.io", "path": "/", "expires": "2030-01-01T00:00:00.000Z"}]},
            {"_id": "fld_a", "_type": "request_group", "parentId": "wrk_a", "name": "API", "metaSortKey": 2, "environment": {"v": 1},
             "authentication": {"type": "bearer", "token": "t"}, "preRequestScript": "console.log(1)"},
            {"_id": "req_b", "_type": "request", "parentId": "fld_a", "name": "Second", "method": "post", "url": "https://{{ _.host }}/b", "metaSortKey": 5,
             "body": {"mimeType": "application/json", "text": "{}"}, "settingFollowRedirects": "off", "settingSendCookies": false, "authentication": {}},
            {"_id": "req_a", "_type": "request", "parentId": "fld_a", "name": "First", "method": "GET", "url": "https://x.io/a", "metaSortKey": 1,
             "authentication": {"type": "hawk", "id": "x"}},
            {"_id": "ws-req_c", "_type": "websocket_request", "parentId": "wrk_a", "name": "WS", "url": "wss://x.io", "metaSortKey": 3},
            {"_id": "ws-payload_c", "_type": "websocket_payload", "parentId": "ws-req_c", "value": "{\"ping\":1}", "mode": "application/json"},
            {"_id": "greq_d", "_type": "grpc_request", "parentId": "wrk_a", "name": "Greet", "url": "grpcb.in:9000", "protoFileId": "pf_d",
             "protoMethodName": "/hello.HelloService/SayHello", "body": {"text": "{\"greeting\":\"x\"}"}, "metaSortKey": 4},
            {"_id": "pd_1", "_type": "proto_directory", "parentId": "wrk_a", "name": "protos"},
            {"_id": "pf_d", "_type": "proto_file", "parentId": "pd_1", "name": "hello.proto", "protoText": "syntax = \"proto3\";"},
            {"_id": "uts_1", "_type": "unit_test_suite", "parentId": "wrk_a", "name": "Suite"},
            {"_id": "x_1", "_type": "plugin_data", "parentId": "wrk_a"},
        ],
    });
    let i = import(&v4.to_string()).unwrap();
    assert_eq!(i.format, Format::InsomniaV4);
    let b = &i.workspaces[0];
    assert_eq!(b.meta.id.as_deref(), Some("wrk_a"));
    assert_eq!(
        b.base_env.as_ref().unwrap().body.data["host"],
        json!("x.io")
    );
    assert!(b.sub_envs[0].body.is_private);
    assert_eq!(
        b.cookie_jar.as_ref().unwrap().body.cookies[0].expires,
        Some(1_893_456_000_000)
    );
    let names: Vec<_> = b.items.iter().map(|i| i.node.name().to_string()).collect();
    assert_eq!(names, ["API", "WS", "Greet"], "sorted by metaSortKey");
    let Node::Folder(f, kids) = &b.items[0].node else {
        panic!()
    };
    assert!(matches!(f.authentication, Auth::Bearer { .. }));
    assert_eq!(
        kids.iter().map(|k| k.node.name()).collect::<Vec<_>>(),
        ["First", "Second"]
    );
    let second = find(b, "Second");
    assert_eq!(second.method, "POST");
    assert_eq!(second.settings.follow_redirects, Toggle::Off);
    assert!(!second.settings.send_cookies);
    assert!(second.authentication.is_inherit());
    assert!(
        matches!(find(b, "First").authentication, Auth::None),
        "unsupported auth becomes No auth"
    );
    let Node::Realtime(ws) = &b.items[1].node else {
        panic!()
    };
    assert_eq!(
        (ws.payload.as_str(), ws.payload_format.as_str()),
        ("{\"ping\":1}", "json")
    );
    let Node::Grpc(g) = &b.items[2].node else {
        panic!()
    };
    assert_eq!(
        (g.url.as_str(), g.schema_source.as_str()),
        ("grpc://grpcb.in:9000", "protos")
    );
    assert_eq!(b.protos[0].body.name, "protos/hello.proto");
    assert_eq!(i.warnings.len(), 3, "{:?}", i.warnings); // hawk, unit tests, plugin_data
}

// ------------------------------------------------------------------ code generation

#[test]
fn code_snippets() {
    use codegen::*;
    let r = CodeRequest {
        method: "POST".into(),
        url: "https://api.x.io/items?q=a b".into(),
        headers: vec![
            ("Content-Type".into(), "application/json".into()),
            ("X-Quote".into(), "it's".into()),
        ],
        body: CodeBody::Text {
            text: "{\"a\":\"b\"}".into(),
        },
    };
    let curl = generate(&r, Target::Curl);
    assert_eq!(
        curl,
        "curl --request POST \\\n  --url 'https://api.x.io/items?q=a b' \\\n  --header 'Content-Type: application/json' \\\n  --header 'X-Quote: it'\\''s' \\\n  --data '{\"a\":\"b\"}'"
    );
    // the generated curl parses back to the same request
    let back = crate::curl::parse(&curl).unwrap();
    assert_eq!(back.method, "POST");
    assert_eq!(back.body.text.as_deref(), Some("{\"a\":\"b\"}"));
    assert!(
        back.headers
            .iter()
            .any(|h| h.name == "X-Quote" && h.value == "it's")
    );
    assert!(generate(&r, Target::PythonRequests).contains("requests.request(\"POST\", \"https://api.x.io/items?q=a b\", headers=headers, data=data.encode())"));
    assert!(generate(&r, Target::JsFetch).contains("body: \"{\\\"a\\\":\\\"b\\\"}\""));
    assert!(generate(&r, Target::Go).contains("strings.NewReader("));
    assert!(generate(&r, Target::RustReqwest).contains(".body(\"{\\\"a\\\":\\\"b\\\"}\")"));
    assert!(generate(&r, Target::Httpie).starts_with("printf %s '{\"a\":\"b\"}' | http POST"));
    let m = CodeRequest {
        method: "POST".into(),
        url: "https://x.io/up".into(),
        body: CodeBody::Multipart {
            parts: vec![
                Part {
                    name: "f".into(),
                    file: Some("/tmp/a.txt".into()),
                    ..Default::default()
                },
                Part {
                    name: "n".into(),
                    value: "1".into(),
                    file: None,
                },
            ],
        },
        ..Default::default()
    };
    assert!(generate(&m, Target::Curl).contains("--form f=@/tmp/a.txt"));
    let go = generate(&m, Target::Go);
    assert!(
        go.contains("w.CreateFormFile(\"f\"")
            && go.contains("w.FormDataContentType()")
            && go.contains("\"mime/multipart\"")
    );
    assert_eq!(Target::parse("python"), Some(Target::PythonRequests));
    assert_eq!(Target::parse("fetch"), Some(Target::JsFetch));
    assert_eq!(Target::parse("cobol"), None);
}

#[test]
fn files_written_before_the_rename_still_import() {
    let text = insomnia::export_v5(&full_bundle())
        .unwrap()
        .replace("x-logic-socket", "x-insomnia-rs")
        .replace("x-lsock", "x-irs");
    assert_eq!(import(&text).unwrap().workspaces[0], full_bundle());
}
