#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};

use agentgod_desktop_common::{
    data_dir, start_with_updater, AutoInstaller, InstallStatus, UpdateSource,
};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_updater::UpdaterExt;

fn main() {
    let status = Arc::new(Mutex::new(InstallStatus::idle()));
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            // The signed updater must follow the same (possibly mirrored)
            // release source the update check uses, otherwise the manifest
            // fetch still goes to github.com and fails on blocked networks.
            let endpoints = UpdateSource::load(&data_dir()).endpoints();
            let installer: Arc<dyn AutoInstaller> = Arc::new(DesktopUpdater {
                app: app.handle().clone(),
                status: status.clone(),
                endpoints,
            });
            let runtime = tauri::async_runtime::block_on(start_with_updater(installer))?;
            let url: tauri::Url = runtime.url.parse()?;
            // Keep both loopback services alive for the app lifetime. Tauri
            // owns the state now instead of leaking a Box into the heap.
            app.manage(Arc::new(runtime));
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("AgentGod Files")
                .inner_size(1280.0, 820.0)
                .min_inner_size(960.0, 640.0)
                .disable_drag_drop_handler()
                // WKWebView cancels downloads unless the host application
                // accepts them, which silently broke "save file" in the app.
                // Accepting keeps the default destination (the user's
                // Downloads folder) with a de-duplicated file name.
                .on_download(|_webview, event| {
                    match event {
                        tauri::webview::DownloadEvent::Requested { url, destination } => {
                            agentgod_files::error::log(&format!(
                                "下载开始 {url} -> {}",
                                destination.display()
                            ));
                        }
                        tauri::webview::DownloadEvent::Finished { url, path, success } => {
                            agentgod_files::error::log(&format!(
                                "下载结束 {url}: success={success} path={}",
                                path.map(|value| value.display().to_string())
                                    .unwrap_or_default()
                            ));
                        }
                        _ => {}
                    }
                    true
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("AgentGod Files 启动失败");
}

fn set_status(
    status: &Arc<Mutex<InstallStatus>>,
    phase: &str,
    percent: u8,
    message: impl Into<String>,
) {
    if let Ok(mut guard) = status.lock() {
        *guard = InstallStatus::new(phase, percent, message);
    }
}

/// Bridges the loopback update API to the signed Tauri updater. Windows exits
/// the process as part of the installer; macOS/Linux need an explicit restart.
struct DesktopUpdater {
    app: tauri::AppHandle,
    status: Arc<Mutex<InstallStatus>>,
    endpoints: Vec<String>,
}

impl AutoInstaller for DesktopUpdater {
    fn available(&self) -> bool {
        true
    }

    fn start(&self) -> Result<(), String> {
        {
            let guard = self
                .status
                .lock()
                .map_err(|_| "更新状态不可用".to_string())?;
            if matches!(
                guard.phase.as_str(),
                "checking" | "downloading" | "installing"
            ) {
                return Err("更新正在进行中".to_string());
            }
        }
        let app = self.app.clone();
        let status = self.status.clone();
        let endpoints = self.endpoints.clone();
        tauri::async_runtime::spawn(async move {
            set_status(&status, "checking", 0, "正在检查签名更新包…");
            let outcome: Result<(), String> = async {
                let mut builder = app.updater_builder();
                if !endpoints.is_empty() {
                    let urls: Vec<tauri::Url> = endpoints
                        .iter()
                        .filter_map(|raw| tauri::Url::parse(raw).ok())
                        .collect();
                    if !urls.is_empty() {
                        builder = builder
                            .endpoints(urls)
                            .map_err(|err| format!("更新源地址无效：{err}"))?;
                    }
                }
                let update = builder
                    .build()
                    .map_err(|err| format!("更新器初始化失败：{err}"))?
                    .check()
                    .await
                    .map_err(|err| format!("检查更新包失败：{err}"))?
                    .ok_or_else(|| "没有可用的签名更新包".to_string())?;
                let version = update.version.clone();
                let downloaded = Arc::new(Mutex::new(0u64));
                let chunk_status = status.clone();
                let chunk_downloaded = downloaded.clone();
                let chunk_version = version.clone();
                let finished_status = status.clone();
                update
                    .download_and_install(
                        move |chunk, total| {
                            let mut done = chunk_downloaded.lock().expect("progress lock");
                            *done += chunk as u64;
                            let percent = total
                                .filter(|value| *value > 0)
                                .map(|value| (*done * 100 / value).min(100) as u8)
                                .unwrap_or(0);
                            drop(done);
                            set_status(
                                &chunk_status,
                                "downloading",
                                percent,
                                format!("正在下载 v{chunk_version}…"),
                            );
                        },
                        move || {
                            set_status(&finished_status, "installing", 100, "正在安装更新…");
                        },
                    )
                    .await
                    .map_err(|err| format!("安装更新失败：{err}"))?;
                set_status(&status, "done", 100, "更新已安装，重启应用后生效");
                Ok(())
            }
            .await;
            if let Err(err) = outcome {
                set_status(&status, "error", 0, err);
            }
        });
        Ok(())
    }

    fn restart(&self) -> Result<(), String> {
        self.app.restart()
    }

    fn status(&self) -> InstallStatus {
        self.status
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}
