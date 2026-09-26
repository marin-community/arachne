//! Arachne — a macOS coding cockpit over Loom.
//!
//! Architecture: the Vue UI never talks HTTP to loom directly. This Rust core
//! owns the Loom client (REST operations + SSE subscriptions), forwards pushed
//! events to the UI via `loom://*` emits, and exposes commands for every
//! interaction. That keeps auth (loopback now, bearer token later for the
//! DGX) and reconnection logic in one place, and sidesteps the browser's
//! EventSource limitations (no headers, 6-connection cap).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod blocks;
pub mod client;
pub mod commands;
pub mod loom;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(commands::LoomState::default())
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::open_session,
            commands::chat_older_cursor,
            commands::fetch_chat,
            commands::send_input,
            commands::interrupt,
            commands::launch_session,
            commands::delegate_task,
            commands::archive_session,
            commands::open_in_zed,
            commands::refresh_fleet,
            commands::reparent_session,
            commands::move_to_workstream,
            commands::delete_workstream,
        ])
        .run(tauri::generate_context!())
        .expect("error while running arachne");
}
