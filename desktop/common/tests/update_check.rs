//! Live checks for the desktop update channel.
//!
//! The GitHub test needs network access and is therefore opted into with
//! `cargo test -- --ignored`. The failure test is hermetic: it points the hub
//! at a closed port and only asserts on the error surface.

use std::{path::PathBuf, sync::Arc};

use agentgod_desktop_common::update::UpdateSource;
use agentgod_desktop_common::{NoopInstaller, UpdateHub};

fn hub(source: UpdateSource) -> Arc<UpdateHub> {
    UpdateHub::new(Arc::new(NoopInstaller), source, PathBuf::from("."))
}

#[tokio::test]
#[ignore = "needs network access to api.github.com"]
async fn github_release_check_reports_the_published_version() {
    let check = hub(UpdateSource::default()).check(true).await;
    assert!(check.error.is_none(), "unexpected error: {:?}", check.error);
    assert_eq!(check.current_version, agentgod_files::VERSION);
    assert!(
        !check.latest_version.is_empty(),
        "latest version was not parsed"
    );
    assert_eq!(check.app, "app");
    assert!(
        !check.silent,
        "NoopInstaller must never advertise the silent channel"
    );
    assert!(check.release_url.starts_with("https://"));
}

#[tokio::test]
async fn unreachable_update_source_reports_an_error_instead_of_hanging() {
    let source = UpdateSource {
        api_url: "https://127.0.0.1:1/releases/latest".to_string(),
        ..UpdateSource::default()
    };
    let check = hub(source).check(true).await;
    let error = check.error.expect("a closed port must surface an error");
    assert!(error.starts_with("检查更新失败："), "{error}");
    assert!(!check.has_update);
}
