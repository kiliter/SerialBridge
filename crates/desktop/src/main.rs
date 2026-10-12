#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use bytes::Bytes;
use serde_json::{json, Value};
use serialbridge_core::{config, node::Node, transport::TransportMode};
use std::{sync::Arc, time::Duration};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, State,
};
use uuid::Uuid;
struct Desktop {
    node: Arc<Node>,
    port: u16,
}
#[tauri::command]
async fn api(
    state: State<'_, Desktop>,
    method: String,
    path: String,
    body: Option<Value>,
) -> Result<Value, String> {
    // This is a fixed loopback API adapter, not an arbitrary URL proxy or shell bridge.
    if !path.starts_with("/v1/")
        || path.contains("..")
        || (path.contains('?') || path.contains('#') || path.contains('%'))
        || path.ends_with("/content")
        || !["GET", "POST", "PUT", "DELETE"].contains(&method.as_str())
    {
        return Err("Unsupported API route".into());
    }
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let mut req = state
        .node
        .client
        .request(method, format!("http://127.0.0.1:{}{}", state.port, path))
        .bearer_auth(&state.node.cfg().api_token)
        .timeout(Duration::from_secs(125));
    if let Some(value) = body {
        req = req.json(&value)
    }
    let res = req.send().await.map_err(|e| e.to_string())?;
    let ok = res.status().is_success();
    let value: Value = res.json().await.map_err(|e| e.to_string())?;
    if ok {
        Ok(value)
    } else {
        Err(value["error"]["message"]
            .as_str()
            .unwrap_or("API request failed")
            .to_string())
    }
}
#[tauri::command]
async fn file_stage(
    state: State<'_, Desktop>,
    op: String,
    params: Value,
    data: Vec<u8>,
) -> Result<Value, String> {
    if data.len() > 16384 {
        return Err("Staging chunk limit is 16 KiB".into());
    }
    let result = match op.as_str() {
        "create" => {
            let mut params = params;
            params["local_staging"] = json!(true);
            state.node.create_upload(params).await
        }
        "write" => state.node.write_block(params, Bytes::from(data)).await,
        "finish" => {
            let id =
                Uuid::parse_str(params["id"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
            state
                .node
                .finish_upload(params)
                .await
                .map_err(|e| e.to_string())?;
            return state.node.start_push(id).map_err(|e| e.to_string());
        }
        "cancel" => state.node.remove_artifact(params).await,
        _ => return Err("Unsupported staging operation".into()),
    };
    result.map(|r| r.value).map_err(|e| e.to_string())
}
#[tauri::command]
fn file_location(state: State<'_, Desktop>, id: String) -> Result<Value, String> {
    let id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let artifact = state
        .node
        .transfers
        .artifact(id)
        .map_err(|e| e.to_string())?;
    Ok(json!({"path":artifact.path,"filename":artifact.filename}))
}
#[tauri::command]
async fn export_logs(state: State<'_, Desktop>) -> Result<Value, String> {
    let lines = {
        let logs = state.node.logs.lock().unwrap();
        logs.iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .join("\n")
    };
    let path = state
        .node
        .cfg()
        .download_dir
        .join(format!("serialbridge-logs-{}.jsonl", Uuid::new_v4()));
    tokio::fs::write(&path, lines)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"path":path}))
}
fn main() {
    let application = tauri::Builder::default()
        .setup(|app| {
            let node = tauri::async_runtime::block_on(serialbridge_core::start(
                config::default_path(),
                TransportMode::Serial,
            ))?;
            let port = node.cfg().http_port;
            app.manage(Desktop { node, port });
            let show = MenuItem::with_id(app, "show", "显示控制台", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出 SerialBridge", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("SerialBridge")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.unminimize();
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => {
                        app.state::<Desktop>().node.shutdown.cancel();
                        app.exit(0);
                    }
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            api,
            file_stage,
            file_location,
            export_logs
        ])
        .build(tauri::generate_context!());
    match application {
        Ok(app) => app.run(|handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(state) = handle.try_state::<Desktop>() {
                    state.node.shutdown.cancel();
                }
            }
        }),
        Err(error) => {
            eprintln!("SerialBridge startup failed: {error}");
            std::process::exit(1);
        }
    }
}
