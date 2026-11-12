#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use agentgod_desktop_common::{start, AppMode};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let runtime = tauri::async_runtime::block_on(start(AppMode::Client))?;
            let url: tauri::Url = runtime.url.parse()?;
            // Tauri owns the runtime so the loopback services stay alive
            // without leaking a Box into the heap.
            app.manage(Arc::new(runtime));
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("AgentGod Files · 客户端")
                .inner_size(1280.0, 820.0)
                .min_inner_size(960.0, 640.0)
                .disable_drag_drop_handler()
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("AgentGod Files 启动失败");
}
