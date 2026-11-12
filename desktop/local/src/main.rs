#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use agentgod_desktop_common::{start, AppMode};
use tauri::{WebviewUrl, WebviewWindowBuilder};

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let runtime = tauri::async_runtime::block_on(start(AppMode::Local))?;
            let url: tauri::Url = runtime.url.parse()?;
            // Both loopback services live as long as the process; keep the
            // runtime alive even though nothing else references it.
            Box::leak(Box::new(runtime));
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("AgentGod Files · 本机")
                .inner_size(1280.0, 820.0)
                .min_inner_size(960.0, 640.0)
                .disable_drag_drop_handler()
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("AgentGod Files 启动失败");
}
