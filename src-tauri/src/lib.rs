//! Arachne — a macOS coding cockpit over Loom.
//!
//! Architecture: the Vue UI never talks HTTP to loom directly. This Rust core
//! owns the Loom client (REST operations + SSE subscriptions), forwards pushed
//! events to the UI via `loom://*` emits, and exposes commands for every
//! interaction. That keeps auth (loopback now, bearer token later for the
//! DGX) and reconnection logic in one place, and sidesteps the browser's
//! EventSource limitations (no headers, 6-connection cap).
//!
//! The bearer token is stored in the macOS Keychain (never localStorage).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod blocks;
pub mod client;
pub mod commands;
pub mod loom;
pub mod resources;
pub mod secret;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(commands::LoomState::default())
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::save_token,
            commands::load_token,
            commands::open_session,
            commands::chat_older_cursor,
            commands::fetch_chat,
            commands::load_session_image,
            commands::send_input,
            commands::send_to_thread,
            commands::interrupt,
            commands::launch_session,
            commands::launch_options,
            commands::repo_branches,
            commands::handoff_session,
            commands::delegate_task,
            commands::archive_session,
            commands::update_session,
            commands::open_in_zed,
            commands::refresh_fleet,
            commands::reparent_session,
            commands::create_group,
            commands::move_to_group,
            commands::delete_group,
            commands::integrate_session,
            commands::land_topic,
            commands::work_summary,
            commands::work_changes,
            commands::topic_resources,
            commands::attach_topic_resource,
            commands::detach_topic_resource,
            commands::read_topic_resource,
            commands::open_topic_resource_in_zed,
            commands::topic_todos,
            commands::add_todo,
            commands::toggle_todo,
            commands::remove_todo,
        ])
        .run(tauri::generate_context!())
        .expect("error while running arachne");
}
