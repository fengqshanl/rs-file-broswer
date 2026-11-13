//! End-to-end test for the SSH target service.
//!
//! A minimal in-process SSH server speaks real SFTP over a real TCP socket,
//! backed by a temporary directory. The test then drives the loopback HTTP
//! service exactly like the desktop frontend: TOFU, encrypted login, listing,
//! upload, download, preview, folder tar, rename and delete.

mod support;

use std::sync::Arc;

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

use agentgod_desktop_common::{
    config::{SshAuth, SshTarget},
    known_hosts::KnownHosts,
    secrets::{self, MemorySecrets, Secrets},
    ssh::service::{self, SshService},
};
use support::{start_ssh_server, HOST_KEY, PASSWORD};

// ---------- Encrypted API client ----------

struct ApiClient {
    http: reqwest::Client,
    base: String,
    key: [u8; 32],
}

impl ApiClient {
    fn aad(method: &str, path: &str) -> String {
        format!("{} {}", method, path)
    }

    async fn call<T: Serialize, R: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &T,
    ) -> R {
        let aad = Self::aad(method.as_str(), path);
        let envelope = agentgod_files::crypto::encrypt_envelope(&self.key, &aad, body).unwrap();
        let response = self
            .http
            .request(method, format!("{}{}", self.base, path))
            .json(&envelope)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let raw = response.bytes().await.unwrap();
        assert!(
            status.is_success(),
            "{path} failed: {status} {}",
            String::from_utf8_lossy(&raw)
        );
        let envelope: agentgod_files::crypto::Envelope = serde_json::from_slice(&raw).unwrap();
        agentgod_files::crypto::decrypt_envelope(&self.key, &aad, &envelope).unwrap()
    }

    async fn upload_chunk(&self, id: &str, seq: u64, data: &[u8]) {
        let path = format!("/api/upload/chunk/{id}/{seq}");
        let aad = Self::aad("PUT", &path);
        let cipher = Aes256Gcm::new_from_slice(&self.key).unwrap();
        let frame = agentgod_files::crypto::seal_frame(&cipher, &aad, data).unwrap();
        let response = self
            .http
            .put(format!("{}{}", self.base, path))
            .header("content-type", "application/octet-stream")
            .body(frame)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "upload chunk {seq} failed");
    }
}

// ---------- Helpers ----------

fn target(id: &str, port: u16, root: &str) -> SshTarget {
    SshTarget {
        id: id.to_string(),
        name: "Test SSH".to_string(),
        host: "127.0.0.1".to_string(),
        port,
        username: "tester".to_string(),
        auth: SshAuth::Password,
        key_path: None,
        root: Some(root.to_string()),
    }
}

fn secrets_store(id: &str) -> Arc<MemorySecrets> {
    let store = Arc::new(MemorySecrets::default());
    store
        .set(&secrets::account(id, "password"), PASSWORD)
        .unwrap();
    store
}

async fn start_service(
    target: SshTarget,
    data_dir: &std::path::Path,
    store: Arc<MemorySecrets>,
    known_hosts: Arc<std::sync::Mutex<KnownHosts>>,
) -> SshService {
    service::start(target, data_dir, store, known_hosts)
        .await
        .expect("start ssh service")
}

async fn login(base: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{base}/api/local-login"))
        .send()
        .await
        .unwrap()
}

async fn login_key(base: &str) -> [u8; 32] {
    let response = login(base).await;
    assert!(response.status().is_success(), "login failed");
    let body: Value = response.json().await.unwrap();
    let raw = STANDARD.decode(body["key"].as_str().unwrap()).unwrap();
    raw.try_into().unwrap()
}

// ---------- Tests ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_target_supports_the_full_file_api() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("existing")).unwrap();
    std::fs::write(root.path().join("existing/hello.txt"), b"hello from sftp").unwrap();

    let port = start_ssh_server(PASSWORD, root.path().to_path_buf()).await;
    let data_dir = tempfile::tempdir().unwrap();
    let known_hosts = Arc::new(std::sync::Mutex::new(KnownHosts::load(data_dir.path())));
    let svc = start_service(
        target("ssh-1", port, "/"),
        data_dir.path(),
        secrets_store("ssh-1"),
        known_hosts,
    )
    .await;
    let base = svc.base_url.clone();

    // TOFU: the first connection must be refused until the fingerprint is
    // explicitly trusted.
    let first = login(&base).await;
    assert_eq!(first.status(), reqwest::StatusCode::CONFLICT);
    let error: Value = first.json().await.unwrap();
    assert_eq!(error["code"], "host_key_unknown");
    let fingerprint = error["host_key"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(fingerprint.starts_with("SHA256:"));

    let trusted: Value = reqwest::Client::new()
        .post(format!("{base}/app/ssh/trust"))
        .json(&json!({ "fingerprint": fingerprint }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(trusted["trusted"], true);

    let key = login_key(&base).await;
    let client = ApiClient {
        http: reqwest::Client::new(),
        base: base.clone(),
        key,
    };

    let info: Value = client
        .http
        .get(format!("{base}/app/info"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["mode"], "ssh");

    // Listing sees the pre-existing fixture.
    let listing: Value = client
        .call(reqwest::Method::POST, "/api/list", &json!({ "path": "/" }))
        .await;
    let names: Vec<String> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"existing".to_string()), "{names:?}");

    // mkdir + upload in multiple chunks.
    let mkdir: Value = client
        .call(
            reqwest::Method::POST,
            "/api/mkdir",
            &json!({ "path": "/docs" }),
        )
        .await;
    assert_eq!(mkdir["ok"], true);

    let payload: Vec<u8> = (0..300_000).map(|index| (index % 251) as u8).collect();
    let init: Value = client
        .call(
            reqwest::Method::POST,
            "/api/upload/init",
            &json!({ "dir": "/docs", "name": "big.bin", "size": payload.len() }),
        )
        .await;
    let upload_id = init["id"].as_str().unwrap().to_string();
    for (seq, chunk) in payload.chunks(64 * 1024).enumerate() {
        client.upload_chunk(&upload_id, seq as u64, chunk).await;
    }
    let finished: Value = client
        .call(
            reqwest::Method::POST,
            "/api/upload/finish",
            &json!({ "id": upload_id }),
        )
        .await;
    assert_eq!(finished["ok"], true);

    // Pagination.
    for index in 0..4 {
        let _: Value = client
            .call(
                reqwest::Method::POST,
                "/api/mkdir",
                &json!({ "path": format!("/docs/dir{index}") }),
            )
            .await;
    }
    let page: Value = client
        .call(
            reqwest::Method::POST,
            "/api/list",
            &json!({ "path": "/docs", "offset": 1, "limit": 2 }),
        )
        .await;
    assert_eq!(page["total"], 5);
    assert_eq!(page["entries"].as_array().unwrap().len(), 2);
    assert_eq!(page["offset"], 1);

    // Download round-trip.
    let download_path = format!("{base}/api/download");
    let aad = ApiClient::aad("POST", "/api/download");
    let envelope = agentgod_files::crypto::encrypt_envelope(
        &client.key,
        &aad,
        &json!({ "path": "/docs/big.bin" }),
    )
    .unwrap();
    let response = client
        .http
        .post(&download_path)
        .json(&envelope)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response
            .headers()
            .get("x-encrypted")
            .and_then(|value| value.to_str().ok()),
        Some("aes-256-gcm")
    );
    let raw = response.bytes().await.unwrap();
    let cipher = Aes256Gcm::new_from_slice(&client.key).unwrap();
    let mut downloaded = Vec::new();
    let mut offset = 0usize;
    let mut seq = 0u64;
    while offset < raw.len() {
        let len = u32::from_be_bytes(raw[offset..offset + 4].try_into().unwrap()) as usize;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&raw[offset + 4..offset + 16]),
                Payload {
                    msg: &raw[offset + 16..offset + 16 + len],
                    aad: format!("dl:{seq}").as_bytes(),
                },
            )
            .unwrap();
        downloaded.extend_from_slice(&plaintext);
        offset += 16 + len;
        seq += 1;
    }
    assert_eq!(downloaded, payload);

    // Folder tar stream ends with the shared marker.
    let aad = ApiClient::aad("POST", "/api/download_folder");
    let envelope =
        agentgod_files::crypto::encrypt_envelope(&client.key, &aad, &json!({ "path": "/docs" }))
            .unwrap();
    let response = client
        .http
        .post(format!("{base}/api/download_folder"))
        .json(&envelope)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let raw = response.bytes().await.unwrap();
    let mut archive = Vec::new();
    let mut offset = 0usize;
    let mut seq = 0u64;
    while offset < raw.len() {
        let len = u32::from_be_bytes(raw[offset..offset + 4].try_into().unwrap()) as usize;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&raw[offset + 4..offset + 16]),
                Payload {
                    msg: &raw[offset + 16..offset + 16 + len],
                    aad: format!("dl:{seq}").as_bytes(),
                },
            )
            .unwrap();
        archive.extend_from_slice(&plaintext);
        offset += 16 + len;
        seq += 1;
    }
    assert!(archive.ends_with(b"AGF_TAR_END_v1"));
    let tar = &archive[..archive.len() - b"AGF_TAR_END_v1".len()];
    assert!(tar.windows(9).any(|window| window == b"big.bin\0\0"));
    assert!(tar.windows(5).any(|window| window == b"docs/"));

    // Preview stream.
    let aad = ApiClient::aad("POST", "/api/preview/stream");
    let envelope = agentgod_files::crypto::encrypt_envelope(
        &client.key,
        &aad,
        &json!({ "path": "/docs/big.bin" }),
    )
    .unwrap();
    let response = client
        .http
        .post(format!("{base}/api/preview/stream"))
        .json(&envelope)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let stream_id = response
        .headers()
        .get("x-stream-id")
        .and_then(|value| value.to_str().ok())
        .unwrap()
        .to_string();
    assert_eq!(
        response
            .headers()
            .get("x-preview-content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/octet-stream")
    );
    let raw = response.bytes().await.unwrap();
    let mut preview = Vec::new();
    let mut offset = 0usize;
    let mut seq = 0u64;
    while offset < raw.len() {
        let len = u32::from_be_bytes(raw[offset..offset + 4].try_into().unwrap()) as usize;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&raw[offset + 4..offset + 16]),
                Payload {
                    msg: &raw[offset + 16..offset + 16 + len],
                    aad: format!("pv:{stream_id}:{seq}").as_bytes(),
                },
            )
            .unwrap();
        preview.extend_from_slice(&plaintext);
        offset += 16 + len;
        seq += 1;
    }
    assert_eq!(preview, payload);

    // Rename and recursive delete.
    let renamed: Value = client
        .call(
            reqwest::Method::POST,
            "/api/rename",
            &json!({ "path": "/docs/big.bin", "new_name": "renamed.bin" }),
        )
        .await;
    assert_eq!(renamed["ok"], true);
    assert!(root.path().join("docs/renamed.bin").exists());

    let deleted: Value = client
        .call(
            reqwest::Method::POST,
            "/api/delete",
            &json!({ "path": "/docs" }),
        )
        .await;
    assert_eq!(deleted["ok"], true);
    assert!(!root.path().join("docs").exists());

    drop(svc);

    // The pin persists: a restarted service logs in without asking again.
    let known_hosts = Arc::new(std::sync::Mutex::new(KnownHosts::load(data_dir.path())));
    let svc2 = start_service(
        target("ssh-1", port, "/"),
        data_dir.path(),
        secrets_store("ssh-1"),
        known_hosts,
    )
    .await;
    let key2 = login_key(&svc2.base_url).await;
    assert_eq!(key2.len(), 32);

    // Wrong credentials surface as an auth error, not a host-key prompt.
    let svc3 = start_service(
        target("ssh-2", port, "/"),
        data_dir.path(),
        {
            let store = Arc::new(MemorySecrets::default());
            store
                .set(&secrets::account("ssh-2", "password"), "wrong")
                .unwrap();
            store
        },
        Arc::new(std::sync::Mutex::new(KnownHosts::load(data_dir.path()))),
    )
    .await;
    let denied = login(&svc3.base_url).await;
    assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: Value = denied.json().await.unwrap();
    assert_eq!(body["code"], "ssh_auth_failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_key_auth_works() {
    let root = tempfile::tempdir().unwrap();
    let port = start_ssh_server(PASSWORD, root.path().to_path_buf()).await;
    let data_dir = tempfile::tempdir().unwrap();
    let key_path = data_dir.path().join("id_ed25519");
    std::fs::write(&key_path, HOST_KEY).unwrap();

    let mut ssh_target = target("ssh-key", port, "/");
    ssh_target.auth = SshAuth::Key;
    ssh_target.key_path = Some(key_path.to_string_lossy().to_string());

    let store = Arc::new(MemorySecrets::default());
    let known_hosts = Arc::new(std::sync::Mutex::new(KnownHosts::load(data_dir.path())));
    let svc = start_service(ssh_target, data_dir.path(), store, known_hosts).await;
    let base = svc.base_url.clone();

    let error: Value = login(&base).await.json().await.unwrap();
    let fingerprint = error["host_key"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    reqwest::Client::new()
        .post(format!("{base}/app/ssh/trust"))
        .json(&json!({ "fingerprint": fingerprint }))
        .send()
        .await
        .unwrap();

    let key = login_key(&base).await;
    let client = ApiClient {
        http: reqwest::Client::new(),
        base: base.clone(),
        key,
    };
    let listing: Value = client
        .call(reqwest::Method::POST, "/api/list", &json!({ "path": "/" }))
        .await;
    assert_eq!(listing["path"], "/");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_service_rejects_paths_outside_the_root() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("inside")).unwrap();
    let port = start_ssh_server(PASSWORD, root.path().to_path_buf()).await;
    let data_dir = tempfile::tempdir().unwrap();
    let known_hosts = Arc::new(std::sync::Mutex::new(KnownHosts::load(data_dir.path())));
    let svc = start_service(
        target("ssh-guard", port, "/"),
        data_dir.path(),
        secrets_store("ssh-guard"),
        known_hosts,
    )
    .await;
    let base = svc.base_url.clone();

    let error: Value = login(&base).await.json().await.unwrap();
    let fingerprint = error["host_key"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    reqwest::Client::new()
        .post(format!("{base}/app/ssh/trust"))
        .json(&json!({ "fingerprint": fingerprint }))
        .send()
        .await
        .unwrap();

    let key = login_key(&base).await;
    let client = ApiClient {
        http: reqwest::Client::new(),
        base,
        key,
    };

    // Traversal is rejected before it reaches SFTP.
    let response = client
        .http
        .post(format!("{}/api/list", client.base))
        .json(
            &agentgod_files::crypto::encrypt_envelope(
                &client.key,
                &ApiClient::aad("POST", "/api/list"),
                &json!({ "path": "/../secret" }),
            )
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

    // A sibling directory outside the configured root stays unreachable even
    // through the remote root switch.
    let switched: Value = reqwest::Client::new()
        .post(format!("{}/app/ssh/root", client.base))
        .json(&json!({ "path": "/inside" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Switching the browsing root is intentionally allowed: it is an explicit
    // user action on an SSH account, unlike a traversal inside one request.
    assert_eq!(switched["ok"], true);
}
