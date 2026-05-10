use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

mod gateway;

const CHAT_GAP: i32 = 12;
const CHAT_VERTICAL_OFFSET: i32 = 42;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatPlacement {
    side: &'static str,
    x: i32,
    y: i32,
}

fn clamp(value: i32, min: i32, max: i32) -> i32 {
    if max < min {
        return min;
    }

    value.max(min).min(max)
}

fn monitor_bounds(
    window: &WebviewWindow,
) -> tauri::Result<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    if let Some(monitor) = window.current_monitor()? {
        return Ok((*monitor.position(), *monitor.size()));
    }

    if let Some(monitor) = window.primary_monitor()? {
        return Ok((*monitor.position(), *monitor.size()));
    }

    Ok((PhysicalPosition::new(0, 0), PhysicalSize::new(1920, 1080)))
}

#[tauri::command]
fn sync_chat_window_position(app: AppHandle) -> Result<ChatPlacement, String> {
    let pet = app
        .get_webview_window("pet")
        .ok_or_else(|| "pet window not found".to_string())?;
    let chat = app
        .get_webview_window("chat")
        .ok_or_else(|| "chat window not found".to_string())?;

    let pet_pos = pet.outer_position().map_err(|error| error.to_string())?;
    let pet_size = pet.outer_size().map_err(|error| error.to_string())?;
    let chat_size = chat.outer_size().map_err(|error| error.to_string())?;
    let (monitor_pos, monitor_size) = monitor_bounds(&pet).map_err(|error| error.to_string())?;

    let area_left = monitor_pos.x;
    let area_top = monitor_pos.y;
    let area_right = area_left + monitor_size.width as i32;
    let area_bottom = area_top + monitor_size.height as i32;
    let chat_width = chat_size.width as i32;
    let chat_height = chat_size.height as i32;

    let preferred_left = pet_pos.x - chat_width - CHAT_GAP;
    let preferred_right = pet_pos.x + pet_size.width as i32 + CHAT_GAP;

    let (side, raw_x) = if preferred_left < area_left {
        ("right", preferred_right)
    } else if preferred_right + chat_width > area_right {
        ("left", preferred_left)
    } else {
        ("left", preferred_left)
    };

    let x = clamp(raw_x, area_left, area_right - chat_width);
    let y = clamp(
        pet_pos.y + CHAT_VERTICAL_OFFSET,
        area_top,
        area_bottom - chat_height,
    );

    let placement = ChatPlacement { side, x, y };

    chat.set_position(PhysicalPosition::new(x, y))
        .map_err(|error| error.to_string())?;
    app.emit("chat:placement", &placement)
        .map_err(|error| error.to_string())?;

    Ok(placement)
}

fn configure_window(window: &WebviewWindow) -> tauri::Result<()> {
    window.set_always_on_top(true)?;
    #[cfg(target_os = "macos")]
    window.set_visible_on_all_workspaces(true)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(gateway::GatewayBridge::default())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("pet") {
                configure_window(&window)?;
            }

            if let Some(window) = app.get_webview_window("chat") {
                configure_window(&window)?;
            }

            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                gateway::initialize_gateway(&app_handle).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sync_chat_window_position,
            gateway::gateway_get_settings,
            gateway::gateway_get_status,
            gateway::gateway_save_settings,
            gateway::gateway_connect,
            gateway::gateway_disconnect,
            gateway::gateway_send_message,
            gateway::gateway_abort
        ])
        .run(tauri::generate_context!())
        .expect("error while running Anima");
}
