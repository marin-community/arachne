//! Test-only IPC stand-in: run actual registry/client transport without a webview.
//! `NATIVE_NAVIGATION_BRIDGE=<this binary> node --test tests/multiServerApp.test.ts`
use arachne_lib::{
    blocks::DisplayBlock, client::LoomClient, commands::ChatSnapshot, routing::ServerRegistry,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() {
    let registry = ServerRegistry::default();
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await.unwrap() {
        let request: Value = serde_json::from_str(&line).unwrap();
        let args = &request["args"];
        let url = args["baseUrl"].as_str().unwrap();
        let command = request["command"].as_str().unwrap();
        let result = async {
            if command == "connect" {
                let client = Arc::new(LoomClient::new(url, None)?);
                client.health().await?;
                let reservation = registry.reserve_connect(url).await?;
                registry.install(reservation, client).await?;
                return Ok(Value::Null);
            }
            let entry = registry.resolve(url).await?;
            match command {
                "refresh_fleet" => Ok(json!({"server_url": url, "sessions": entry.client.list_sessions().await?, "layout": entry.client.session_layout().await?})),
                "open_session" => {
                    let id = args["id"].as_str().unwrap();
                    let generation = entry.begin_open(id)?;
                    let view = entry.client.get_session(id).await?;
                    // Idle fixtures do not need queued recovery or a GUI SSE forwarder.
                    assert_eq!(view.status, "idle");
                    entry.activate_chat(id, generation)?;
                    Ok(json!(view))
                }
                "fetch_chat" => {
                    let id = args["id"].as_str().unwrap();
                    let page = entry.begin_page(id)?;
                    let chat = entry.client.session_chat(id, None).await?;
                    let (live_started_at, live_progress_at) = ChatSnapshot::live_timing(&chat);
                    entry.finish_page(id, page, chat.older_cursor.clone())?;
                    Ok(json!(ChatSnapshot {
                        blocks: chat.blocks.iter().map(DisplayBlock::from_view).collect(),
                        older_cursor: chat.older_cursor, live_turn: chat.live_turn,
                        pending_prompt: chat.pending_prompt, live_started_at, live_progress_at,
                        metadata: chat.metadata,
                    }))
                }
                _ => panic!("unsupported test command {command}"),
            }
        }.await;
        let response = match result {
            Ok::<Value, arachne_lib::client::LoomError>(value) => json!({"result":value}),
            Err(error) => json!({"error":error.to_string()}),
        };
        stdout
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
        stdout.flush().await.unwrap();
    }
}
