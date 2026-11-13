//! Companion-level SSH target lifecycle.
//!
//! Drives the same HTTP surface the desktop UI uses: add an SSH target, hit
//! TOFU through the proxy, trust the fingerprint, log in, test the connection,
//! change credentials, forget the pin and remove the target.

mod support;

use std::time::Duration;

use serde_json::{json, Value};

use support::{start_ssh_server, PASSWORD};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
}

async fn action(http: &reqwest::Client, base: &str, body: Value) -> (reqwest::StatusCode, Value) {
    let response = http
        .post(format!("{base}/app/servers"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let value = response.json().await.unwrap_or(Value::Null);
    (status, value)
}

async fn login(http: &reqwest::Client, base: &str, id: &str) -> (reqwest::StatusCode, Value) {
    let response = http
        .get(format!("{base}/api/local-login"))
        .header("x-agentgod-target", id)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let value = response.json().await.unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn companion_manages_an_ssh_target_end_to_end() {
    let remote_root = tempfile::tempdir().unwrap();
    let port = start_ssh_server(PASSWORD, remote_root.path().to_path_buf()).await;

    let data_dir = tempfile::tempdir().unwrap();
    let local_root = tempfile::tempdir().unwrap();
    std::env::set_var("AGENTGOD_DATA_DIR", data_dir.path());
    std::env::set_var("AGENTGOD_MEMORY_SECRETS", "1");
    std::env::set_var("AGENTGOD_LOCAL_ROOT", local_root.path());
    let runtime = agentgod_desktop_common::start().await.unwrap();
    let base = runtime.url.trim_end_matches('/').to_string();
    let http = client();

    let (status, added) = action(
        &http,
        &base,
        json!({
            "action": "add_ssh",
            "name": "Companion SSH",
            "host": "127.0.0.1",
            "port": port,
            "username": "tester",
            "auth": "password",
            "password": PASSWORD,
            "root": "/",
        }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{added}");
    assert_eq!(added["ok"], true);
    let id = added["id"].as_str().unwrap().to_string();

    // Credentials never reach targets.json.
    let raw = std::fs::read_to_string(data_dir.path().join("targets.json")).unwrap();
    assert!(!raw.contains(PASSWORD), "password leaked into targets.json");
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    let stored = parsed["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|target| target["id"] == id.as_str())
        .unwrap();
    assert_eq!(stored["kind"], "ssh");
    assert!(stored.get("password").is_none());

    // First contact must ask for the host key.
    let (status, error) = login(&http, &base, &id).await;
    assert_eq!(status, reqwest::StatusCode::CONFLICT);
    assert_eq!(error["code"], "host_key_unknown");
    let fingerprint = error["host_key"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, trusted) = action(
        &http,
        &base,
        json!({ "action": "trust_host", "id": id, "fingerprint": fingerprint }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{trusted}");
    assert_eq!(trusted["trusted"], true);

    let (status, session) = login(&http, &base, &id).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{session}");
    assert!(session["key"].as_str().unwrap().len() > 20);

    // The target-aware /app/info reaches the SSH service, not the companion.
    let info: Value = http
        .get(format!("{base}/app/info"))
        .header("x-agentgod-target", &id)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["mode"], "ssh");

    let (status, tested) = action(&http, &base, json!({ "action": "test", "id": id })).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{tested}");
    assert_eq!(tested["ok"], true);

    // Wrong credentials surface as an auth error, not a fingerprint prompt.
    let (status, updated) = action(
        &http,
        &base,
        json!({
            "action": "update_ssh",
            "id": id,
            "name": "Companion SSH",
            "host": "127.0.0.1",
            "port": port,
            "username": "tester",
            "auth": "password",
            "password": "definitely-wrong",
            "root": "",
        }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{updated}");
    let (status, denied) = login(&http, &base, &id).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(denied["code"], "ssh_auth_failed");

    // Restoring the password reconnects without another TOFU prompt.
    let (status, updated) = action(
        &http,
        &base,
        json!({
            "action": "update_ssh",
            "id": id,
            "name": "Companion SSH",
            "host": "127.0.0.1",
            "port": port,
            "username": "tester",
            "auth": "password",
            "password": PASSWORD,
            "root": "",
        }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{updated}");
    let (status, session) = login(&http, &base, &id).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{session}");

    let known_hosts = std::fs::read_to_string(data_dir.path().join("known_hosts.json")).unwrap();
    assert!(known_hosts.contains(&fingerprint));

    let (status, forgotten) =
        action(&http, &base, json!({ "action": "forget_host", "id": id })).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{forgotten}");
    assert!(forgotten["removed"].as_u64().unwrap() >= 1);
    let (status, error) = login(&http, &base, &id).await;
    assert_eq!(status, reqwest::StatusCode::CONFLICT);
    assert_eq!(error["code"], "host_key_unknown");

    let (status, removed) = action(&http, &base, json!({ "action": "remove", "id": id })).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{removed}");
    let info: Value = http
        .get(format!("{base}/app/info"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(info["targets"]
        .as_array()
        .unwrap()
        .iter()
        .all(|target| target["id"] != id.as_str()));

    drop(runtime);
    drop(data_dir);
    drop(remote_root);
    drop(local_root);
}
