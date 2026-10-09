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
        ("postman/orders-v2.1.json", Format::PostmanCollection),
        ("postman/orders-v2.0.json", Format::PostmanCollection),
        ("postman/auth-v2.1.json", Format::PostmanCollection),
        (
            "postman/staging.postman_environment.json",
            Format::PostmanEnvironment,
        ),
        ("openapi/bookstore-v3.yaml", Format::OpenApi3),
        ("openapi/bookstore-v2.json", Format::Swagger2),
        ("har/browser-session.har", Format::Har),
        ("har/single-request.json", Format::Har),
        ("insomnia/orders.yaml", Format::InsomniaV5),
    ];
    for (f, want) in cases {
        assert_eq!(detect(&fixture(f)), Some(want), "{f}");
    }
    assert_eq!(detect("# setup\ncurl https://x.io"), Some(Format::Curl));
    assert_eq!(
        detect("logicSocket: 1\nkind: collection\nname: X\n"),
        Some(Format::LogicSocket)
    );
    assert_eq!(detect("hello: world"), None);
    assert!(matches!(import("{}"), Err(ConvertError::Unknown)));
}

// ------------------------------------------------------------------ Logic Socket format

#[test]
fn native_format_round_trips_every_item_kind() {
    let b = full_bundle();
    let text = native::export(&b).unwrap();
    assert!(
        text.starts_with("logicSocket: 1\nkind: collection\n"),
        "{text}"
    );
    let again = import(&text).unwrap();
    assert_eq!(again.format, Format::LogicSocket);
    assert!(again.warnings.is_empty(), "{:?}", again.warnings);
    assert_eq!(again.workspaces[0], b);
}

#[test]
fn native_format_is_readable_and_keeps_local_state_out() {
    let mut b = full_bundle();
    b.workspace.active_environment_id = Some("env_2".into());
    let text = native::export(&b).unwrap();
    let v: serde_json::Value = serde_yaml_ng::from_str(&text).unwrap();
    assert_eq!(v["name"], "Everything");
    assert!(v.get("activeEnvironmentId").is_none() && v.get("scope").is_none());
    assert_eq!(v["environments"]["base"]["secretKeys"], json!(["token"]));
    let kinds: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["folder", "realtime", "realtime", "realtime", "grpc", "mcp"]
    );
    assert_eq!(v["items"][0]["children"][1]["type"], "llm");
    assert_eq!(
        v["items"][0]["children"][0]["url"],
        "{{ _.base }}/items/:id"
    );
    assert_eq!(
        native::file_name_for("Shop API v2!"),
        "logic-socket.shop-api-v2.yaml"
    );
    // newer files are refused with a clear message
    let future = text.replacen("logicSocket: 1", "logicSocket: 9", 1);
    assert!(
        import(&future)
            .unwrap_err()
            .to_string()
            .contains("update Logic Socket")
    );
}

// ------------------------------------------------------------------ Postman

#[test]
fn postman_v21_collection() {
    let (b, w) = one("postman/orders-v2.1.json");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(b.workspace.name, "Acme Orders");
    assert_eq!(
        b.workspace.description,
        "Orders service used by the storefront."
    );
    let env = &b.base_env.as_ref().unwrap().body.data;
    assert_eq!(env["base_url"], json!("https://orders.acme.test"));
    // collection auth + scripts live on a root folder named after the collection
    let Node::Folder(root, kids) = &b.items[0].node else {
        panic!("root folder")
    };
    assert_eq!(root.name, "Acme Orders");
    assert!(
        matches!(&root.authentication, Auth::Bearer { token, .. } if token == "{{ _['access-token'] }}")
    );
    assert_eq!(
        root.pre_request_script.as_deref(),
        Some("ls.variables.set('started', Date.now());")
    );
    assert_eq!(
        kids.iter().map(|k| k.node.name()).collect::<Vec<_>>(),
        ["Orders", "Sign in", "Search (GraphQL)", "Health"]
    );

    let list = find(&b, "List orders");
    assert_eq!(list.url, "{{ base_url }}/orders");
    assert_eq!(
        list.parameters
            .iter()
            .map(|p| (p.name.as_str(), p.value.as_str(), p.disabled))
            .collect::<Vec<_>>(),
        [("status", "open", false), ("limit", "20", true)]
    );
    assert_eq!(
        list.after_response_script.as_deref(),
        Some(
            "ls.test('status is 200', function() { ls.expect(ls.response.code === 200).to.be.true; });\nls.test('has orders', () => ls.expect(ls.response.json().orders).to.be.an('array'));"
        )
    );
    assert_eq!(
        find(&b, "Get order").path_parameters,
        vec![KeyValue::new("orderId", "ord_123")]
    );

    let create = find(&b, "Create order");
    assert_eq!(create.body.mime_type.as_deref(), Some(mime::JSON));
    assert!(
        create
            .body
            .text
            .as_deref()
            .unwrap()
            .contains("\"quantity\": {% faker 'randomInt' %}")
    );
    assert!(
        create
            .body
            .text
            .as_deref()
            .unwrap()
            .contains("\"ref\": \"{% uuid 'v4' %}\"")
    );
    assert!(
        create
            .headers
            .contains(&KeyValue::new("Content-Type", mime::JSON))
    );

    let upload = find(&b, "Upload invoice");
    assert_eq!(upload.body.mime_type.as_deref(), Some(mime::MULTIPART));
    let file = &upload.body.params[1];
    assert_eq!(
        (file.kind.as_deref(), file.file_name.as_deref()),
        (Some("file"), Some("/tmp/invoice.pdf"))
    );
    assert!(upload.body.params[2].disabled);
    assert!(
        !upload
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("content-type")),
        "multipart sets its own boundary"
    );

    let sign_in = find(&b, "Sign in");
    assert_eq!(sign_in.body.mime_type.as_deref(), Some(mime::FORM));
    assert_eq!(sign_in.body.params[0].value, "{% faker 'randomEmail' %}");

    let gql = find(&b, "Search (GraphQL)");
    assert_eq!(gql.body.mime_type.as_deref(), Some(mime::GRAPHQL));
    let body: serde_json::Value = serde_json::from_str(gql.body.text.as_deref().unwrap()).unwrap();
    assert_eq!(
        body["variables"],
        json!({"q": "open"}),
        "variables parsed from Postman's string form"
    );
    assert!(
        gql.headers
            .contains(&KeyValue::new("Content-Type", mime::JSON))
    );

    let health = find(&b, "Health");
    assert_eq!(
        (health.method.as_str(), health.url.as_str()),
        ("GET", "https://orders.acme.test/health")
    );
}

#[test]
fn postman_v20_collection() {
    let (b, _) = one("postman/orders-v2.0.json");
    let Node::Folder(admin, _) = &b.items[0].node else {
        panic!()
    };
    assert!(
        matches!(&admin.authentication, Auth::Basic { username, password, .. } if username == "admin" && password == "{{ admin_password }}")
    );
    let refund = find(&b, "Refund");
    assert_eq!(refund.method, "POST");
    assert_eq!(
        refund
            .body
            .params
            .iter()
            .map(|p| p.disabled)
            .collect::<Vec<_>>(),
        [false, true],
        "v2.0 uses `enabled`"
    );
}

#[test]
fn postman_auth_kinds() {
    let (b, w) = one("postman/auth-v2.1.json");
    assert!(matches!(find(&b, "no auth").authentication, Auth::None));
    assert!(
        matches!(&find(&b, "basic").authentication, Auth::Basic { username, .. } if username == "ana")
    );
    assert!(
        matches!(&find(&b, "digest").authentication, Auth::Digest { username, .. } if username == "ben")
    );
    assert!(
        matches!(&find(&b, "api key in query").authentication, Auth::ApiKey { key, value, add_to: Some(t), .. } if key == "api_key" && value == "{{ key }}" && t == "queryParams")
    );
    assert!(
        matches!(&find(&b, "aws").authentication, Auth::Iam(c) if c.region == "eu-west-1" && c.service == "execute-api")
    );
    assert!(
        matches!(&find(&b, "oauth1").authentication, Auth::OAuth1(c) if c.consumer_key == "ck" && c.signature_method == "HMAC-SHA256")
    );
    let Auth::OAuth2(pkce) = &find(&b, "oauth2 pkce").authentication else {
        panic!()
    };
    assert!(pkce.use_pkce);
    assert_eq!(
        (pkce.authorization_url.as_str(), pkce.scope.as_str()),
        ("https://id.test/authorize", "orders:read")
    );
    assert_eq!(
        pkce.audience, "https://api.acme.test",
        "object-valued params use their value"
    );
    let Auth::OAuth2(code) = &find(&b, "oauth2 code").authentication else {
        panic!()
    };
    assert!(!code.use_pkce && code.credentials_in_body);
    assert!(
        matches!(&find(&b, "oauth2 password").authentication, Auth::OAuth2(c) if c.grant_type == "password" && c.username == "cy")
    );
    assert!(
        matches!(&find(&b, "oauth2 client").authentication, Auth::OAuth2(c) if c.grant_type == "client_credentials")
    );
    assert!(
        matches!(&find(&b, "oauth2 implicit").authentication, Auth::OAuth2(c) if c.grant_type == "authorization_code")
    );
    assert!(matches!(find(&b, "hawk").authentication, Auth::None));
    assert!(find(&b, "from header").authentication.is_inherit());
    assert_eq!(w.len(), 2, "implicit grant and hawk are reported: {w:?}");
}

#[test]
fn postman_variables_and_faker() {
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
fn postman_script_translation() {
    let t = postman::translate_script;
    assert_eq!(
        t("tests['ok'] = responseCode.code === 200;"),
        "ls.test('ok', function() { ls.expect(ls.response.code === 200).to.be.true; });"
    );
    assert_eq!(
        t("environment.token = 'abc';"),
        "ls.environment.set('token', 'abc');"
    );
    assert_eq!(t("var x = globals.foo;"), "var x = ls.globals.get('foo');");
    assert_eq!(
        t("postman.setEnvironmentVariable('a', 1)"),
        "ls.environment.set('a', 1)"
    );
    assert_eq!(
        t("postman.clearEnvironmentVariable()"),
        "ls.environment.clear()"
    );
    assert_eq!(
        t("const b = JSON.parse(responseBody);"),
        "const b = JSON.parse(ls.response.text());"
    );
    assert_eq!(
        t("pm.expect(pm.response.code).to.eql(200)"),
        "ls.expect(ls.response.code).to.eql(200)"
    );
    // left alone: identifiers that only end in pm/environment, properties and strings
    assert_eq!(
        t("upm.x; obj.environment.y; 'pm.z'"),
        "upm.x; obj.environment.y; 'pm.z'"
    );
    assert_eq!(
        postman::script_to_postman("ls.test('a', () => ls.expect(1).to.eql(1))"),
        "pm.test('a', () => pm.expect(1).to.eql(1))"
    );
}

#[test]
fn postman_environment_and_secrets() {
    let (b, _) = one("postman/staging.postman_environment.json");
    assert_eq!(b.workspace.scope, WorkspaceScope::Environment);
    let sub = &b.sub_envs[0].body;
    assert_eq!(sub.name, "Staging");
    assert_eq!(sub.data["base_url"], json!("https://staging.acme.test"));
    assert_eq!(sub.secret_keys, vec!["access-token"]);
    assert!(
        !sub.data.contains_key("retired"),
        "disabled values are skipped"
    );
    let back: serde_json::Value = serde_json::from_str(&postman::export_environment(sub)).unwrap();
    assert_eq!(back["values"][1]["type"], "secret");
}

#[test]
fn postman_export_round_trips() {
    let (b, _) = one("postman/orders-v2.1.json");
    let (text, skipped) = postman::export_collection(&b);
    assert_eq!(skipped, 0);
    let again = import(&text).unwrap();
    assert_eq!(again.format, Format::PostmanCollection);
    let shape = |b: &WorkspaceBundle| {
        requests(b)
            .iter()
            .map(|r| {
                (
                    r.name.clone(),
                    r.method.clone(),
                    r.url.clone(),
                    r.body.clone(),
                    r.after_response_script.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(shape(&b), shape(&again.workspaces[0]));
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let list = &v["item"][0]["item"][0]["item"][0];
    assert_eq!(
        list["request"]["url"]["raw"],
        "{{base_url}}/orders?status=open"
    );
    assert!(
        list["event"][0]["script"]["exec"][0]
            .as_str()
            .unwrap()
            .starts_with("pm.test('status is 200'")
    );
}

// ------------------------------------------------------------------ OpenAPI / Swagger

#[test]
#[ignore = "BUG-016"]
fn openapi_cookie_parameter_is_imported() {
    let spec = r#"{"openapi":"3.0.0","info":{"title":"t","version":"1"},"paths":{"/a":{"get":{"operationId":"getA","parameters":[{"name":"sid","in":"cookie","required":true,"schema":{"type":"string","example":"abc"}}]}}}}"#;
    let imported = import(spec).unwrap_or_else(|e| panic!("{e}"));
    let reqs = requests(&imported.workspaces[0]);
    let r = reqs[0];
    let kept = r
        .headers
        .iter()
        .any(|h| h.name.eq_ignore_ascii_case("cookie") && h.value.contains("sid"))
        || r.parameters.iter().any(|p| p.name == "sid")
        || r.headers.iter().any(|h| h.name == "sid");
    assert!(
        kept,
        "cookie parameter dropped\nheaders={:?}\nparams={:?}",
        r.headers, r.parameters
    );
}

#[test]
fn openapi3_bookstore() {
    let (b, w) = one("openapi/bookstore-v3.yaml");
    assert_eq!(b.workspace.name, "Bookstore 2.1");
    let env = &b.base_env.as_ref().unwrap().body.data;
    assert_eq!(
        env["base_url"],
        json!("{{ _.scheme }}://{{ _.host }}/api/{{ _.version }}")
    );
    assert_eq!(
        (env["scheme"].clone(), env["host"].clone()),
        (json!("https"), json!("books.acme.test"))
    );
    let folders: Vec<_> = b.items.iter().map(|i| i.node.name().to_string()).collect();
    assert_eq!(
        folders,
        ["books", "orders", "Health check", "Who am I", "Search"],
        "tagged ops in folders, untagged at the root"
    );

    let list = find(&b, "List books");
    assert_eq!(list.url, "{{ _.base_url }}/books");
    assert_eq!(
        list.parameters
            .iter()
            .map(|p| (p.name.as_str(), p.value.as_str(), p.disabled))
            .collect::<Vec<_>>(),
        [("genre", "fiction", false), ("page", "1", true)]
    );
    assert_eq!(list.headers[0].name, "X-Trace");
    assert!(
        matches!(&list.authentication, Auth::ApiKey { key, add_to: Some(t), .. } if key == "X-Api-Key" && t == "header"),
        "global security"
    );

    let add = find(&b, "Add a book");
    assert!(
        matches!(&add.authentication, Auth::Bearer { token, .. } if token == "{{ _.bearer_token }}")
    );
    let book: serde_json::Value = serde_json::from_str(add.body.text.as_deref().unwrap()).unwrap();
    assert_eq!(book["title"], "The Rust Book");
    assert_eq!(book["author"]["email"], "user@example.com");
    assert!(book.get("id").is_none(), "readOnly properties are left out");

    let get = find(&b, "Get a book");
    assert_eq!(get.url, "{{ _.base_url }}/books/:bookId");
    assert_eq!(
        get.path_parameters,
        vec![KeyValue::new("bookId", "bk_42")],
        "path-level parameters apply"
    );
    assert!(
        matches!(get.authentication, Auth::None),
        "`security: []` means no auth"
    );

    assert!(
        matches!(&find(&b, "deleteBook").authentication, Auth::OAuth2(c) if c.grant_type == "authorization_code" && c.scope == "books:write" && c.access_token_url == "https://id.acme.test/token")
    );
    let cover = find(&b, "Upload a cover");
    assert_eq!(cover.body.mime_type.as_deref(), Some(mime::MULTIPART));
    assert_eq!(
        cover
            .body
            .params
            .iter()
            .map(|p| (p.name.as_str(), p.kind.as_deref()))
            .collect::<Vec<_>>(),
        [("image", Some("file")), ("caption", Some("text"))]
    );

    let order = find(&b, "Place an order");
    assert_eq!(
        order.body.mime_type.as_deref(),
        Some(mime::JSON),
        "*/* becomes JSON"
    );
    let o: serde_json::Value = serde_json::from_str(order.body.text.as_deref().unwrap()).unwrap();
    assert_eq!(
        o,
        json!({"bookId": "00000000-0000-0000-0000-000000000000", "quantity": 0, "gift": true}),
        "allOf merged"
    );
    assert!(
        matches!(&order.authentication, Auth::Basic { username, .. } if username == "{{ _.http_username }}")
    );

    let cancel = find(&b, "Cancel an order");
    assert_eq!(cancel.body.mime_type.as_deref(), Some(mime::FORM));
    assert_eq!(cancel.body.params[0].value, "changed my mind");
    assert!(
        matches!(&cancel.authentication, Auth::ApiKey { add_to: Some(t), .. } if t == "cookie")
    );
    assert!(
        matches!(&find(&b, "Search").authentication, Auth::ApiKey { add_to: Some(t), .. } if t == "queryParams")
    );
    assert!(matches!(
        &find(&b, "Who am I").authentication,
        Auth::OAuth2(_)
    ));
    assert_eq!(
        w.len(),
        2,
        "implicit flow + OpenID Connect, once each: {w:?}"
    );
    assert!(b.spec.as_deref().unwrap().contains("openapi: 3.0.3"));
}

#[test]
fn swagger2_bookstore() {
    let (b, _) = one("openapi/bookstore-v2.json");
    assert_eq!(
        b.base_env.as_ref().unwrap().body.data["base_url"],
        json!("http://legacy.acme.test/v1")
    );
    let create = find(&b, "Create book");
    assert_eq!(create.body.mime_type.as_deref(), Some(mime::JSON));
    assert!(
        create
            .body
            .text
            .as_deref()
            .unwrap()
            .contains("\"pages\": 0")
    );
    assert!(matches!(&create.authentication, Auth::ApiKey { key, .. } if key == "X-Token"));
    let rate = find(&b, "Rate a book");
    assert_eq!(rate.body.mime_type.as_deref(), Some(mime::FORM));
    assert_eq!(
        rate.body
            .params
            .iter()
            .map(|p| (p.name.as_str(), p.value.as_str()))
            .collect::<Vec<_>>(),
        [("stars", "5"), ("comment", "")]
    );
    let scan = find(&b, "Upload a scan");
    assert_eq!(scan.body.mime_type.as_deref(), Some(mime::MULTIPART));
    assert_eq!(scan.body.params[0].kind.as_deref(), Some("file"));
    assert!(
        matches!(&scan.authentication, Auth::OAuth2(c) if c.grant_type == "client_credentials")
    );
}

// ------------------------------------------------------------------ HAR / cURL

#[test]
fn har_and_curl() {
    let (b, _) = one("har/browser-session.har");
    assert_eq!(b.workspace.name, "HAR import (Browser DevTools)");
    let cart = find(&b, "GET /api/cart");
    assert_eq!(cart.url, "https://shop.acme.test/api/cart");
    assert_eq!(cart.parameters, vec![KeyValue::new("currency", "EUR")]);
    assert_eq!(
        cart.headers,
        vec![KeyValue::new("accept", "application/json")],
        "pseudo, sec-* and length headers dropped"
    );
    let add = find(&b, "Add to cart");
    assert_eq!(add.method, "POST");
    assert_eq!(add.body.mime_type.as_deref(), Some(mime::JSON));
    assert_eq!(
        add.body.text.as_deref(),
        Some("{\"sku\":\"BOOK-42\",\"qty\":1}")
    );

    let (b, _) = one("har/single-request.json");
    let r = requests(&b)[0];
    assert_eq!(r.body.mime_type.as_deref(), Some(mime::MULTIPART));
    assert_eq!(
        r.body.params[0].file_name.as_deref(),
        Some("/home/ana/me.png")
    );
    assert!(
        r.headers.is_empty(),
        "multipart content type is regenerated"
    );
    assert_eq!(
        requests(&import(&har::export(&b)).unwrap().workspaces[0])[0]
            .body
            .params
            .len(),
        2
    );

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
fn insomnia_file_import_and_round_trip() {
    let (b, w) = one("insomnia/orders.yaml");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(
        (b.workspace.name.as_str(), b.workspace.description.as_str()),
        ("Acme Orders", "Orders service")
    );
    assert_eq!(b.meta.id.as_deref(), Some("wrk_orders01"));
    let Node::Folder(orders, kids) = &b.items[0].node else {
        panic!()
    };
    assert_eq!(orders.environment["page_size"], json!(20));
    assert!(
        matches!(&orders.authentication, Auth::Bearer { token, .. } if token == "{{ _.token }}")
    );
    assert_eq!(
        orders.pre_request_script.as_deref(),
        Some("ls.environment.set('ts', Date.now());"),
        "scripts move to ls.*"
    );
    let Node::Request(get) = &kids[0].node else {
        panic!()
    };
    assert_eq!(get.description, "Fetch one order");
    assert_eq!(get.path_parameters, vec![KeyValue::new("id", "ord_1")]);
    assert!(get.parameters[0].disabled);
    assert_eq!(get.settings.follow_redirects, Toggle::Off);
    assert!(!get.settings.store_cookies);
    assert!(
        get.after_response_script
            .as_deref()
            .unwrap()
            .starts_with("ls.test('ok'")
    );
    let Node::Realtime(ws) = &b.items[1].node else {
        panic!()
    };
    assert_eq!(
        (ws.kind.as_str(), ws.url.as_str()),
        ("websocket", "wss://orders.acme.test/live")
    );
    let Node::Grpc(g) = &b.items[2].node else {
        panic!()
    };
    assert_eq!(
        (g.url.as_str(), g.method.as_str(), g.schema_source.as_str()),
        (
            "grpc://grpc.acme.test:443",
            "/stock.Stock/Reserve",
            "reflection"
        )
    );
    assert_eq!(
        b.cookie_jar.as_ref().unwrap().body.cookies[0].expires,
        Some(1_893_456_000_000)
    );
    assert_eq!(b.sub_envs[0].body.color.as_deref(), Some("#22c55e"));

    // exporting back writes insomnia.* scripts again and re-imports identically
    let text = insomnia::export_v5(&b).unwrap();
    assert!(text.contains("insomnia.test('ok'") && !text.contains("ls.test"));
    assert_eq!(import(&text).unwrap().workspaces[0], b);
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
                        pre_request_script: Some("ls.environment.set('a', 1);".into()),
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
fn insomnia_export_keeps_every_item_kind() {
    let b = full_bundle();
    let text = insomnia::export_v5(&b).unwrap();
    let again = import(&text).unwrap();
    assert_eq!(again.format, Format::InsomniaV5);
    assert!(again.warnings.is_empty(), "{:?}", again.warnings);
    assert_eq!(again.workspaces[0], b);
}

#[test]
fn insomnia_export_uses_the_shapes_insomnia_expects() {
    let text = insomnia::export_v5(&full_bundle()).unwrap();
    let v: serde_json::Value = serde_yaml_ng::from_str(&text).unwrap();
    assert_eq!(v["type"], "collection.insomnia.rest/5.0");
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
    // AI requests and MCP servers have no Insomnia equivalent: kept in an extension block
    let names: Vec<_> = v["x-logic-socket"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["LlmRequest", "McpServer"]);
    assert_eq!(v["x-logic-socket"]["items"][0]["parentId"], "fld_1");
    assert!(
        coll.iter()
            .find(|i| i["name"] == "Hello")
            .unwrap()
            .get("authentication")
            .is_none(),
        "inherit is written as nothing"
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
