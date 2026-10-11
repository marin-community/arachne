//! Explicit server routing and per-server subscription/pagination lifecycle.
use crate::{
    client::{LoomClient, LoomError},
    loom::ChatCursorView,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;

pub fn normalize_server_url(base_url: &str) -> Result<String, LoomError> {
    let mut url = reqwest::Url::parse(base_url.trim())
        .map_err(|e| LoomError::Connection(format!("invalid server URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(LoomError::Connection(
            "server URL must use HTTP(S) without embedded credentials, query, or fragment".into(),
        ));
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

#[derive(Default)]
pub struct ServerRegistry {
    state: RwLock<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    servers: HashMap<String, Arc<ServerEntry>>,
    connect_generations: HashMap<String, u64>,
}

pub struct ConnectReservation {
    server_url: String,
    generation: u64,
}

impl ServerRegistry {
    pub async fn resolve(&self, base_url: &str) -> Result<Arc<ServerEntry>, LoomError> {
        let url = normalize_server_url(base_url)?;
        self.state
            .read()
            .await
            .servers
            .get(&url)
            .cloned()
            .ok_or_else(|| LoomError::Connection(format!("not connected to Loom server {url}")))
    }

    /// Reserve before any network await. A newer attempt supersedes earlier
    /// attempts for this URL, without retiring the currently working entry.
    pub async fn reserve_connect(&self, base_url: &str) -> Result<ConnectReservation, LoomError> {
        let server_url = normalize_server_url(base_url)?;
        let mut state = self.state.write().await;
        let generation = state
            .connect_generations
            .entry(server_url.clone())
            .or_default();
        *generation += 1;
        Ok(ConnectReservation {
            server_url,
            generation: *generation,
        })
    }

    /// Install only an authenticated, still-current connect attempt. Replacing
    /// an entry retires its tasks/pages; unrelated entries remain live.
    pub async fn install(
        &self,
        reservation: ConnectReservation,
        client: Arc<LoomClient>,
    ) -> Result<Arc<ServerEntry>, LoomError> {
        let mut state = self.state.write().await;
        if reservation.server_url != client.server_url()
            || state.connect_generations.get(&reservation.server_url)
                != Some(&reservation.generation)
        {
            return Err(LoomError::Api {
                status: 409,
                method: "POST",
                path: "connect".into(),
                message: "connection attempt superseded by a newer connect".into(),
            });
        }
        let entry = Arc::new(ServerEntry::new(client));
        if let Some(old) = state.servers.get(&reservation.server_url) {
            old.cancel();
        }
        state.servers.insert(reservation.server_url, entry.clone());
        Ok(entry)
    }

    pub async fn entry_for_client(&self, client: &Arc<LoomClient>) -> Option<Arc<ServerEntry>> {
        self.state
            .read()
            .await
            .servers
            .get(&client.server_url())
            .filter(|entry| Arc::ptr_eq(&entry.client, client))
            .cloned()
    }

    #[cfg(test)]
    pub async fn install_for_test(&self, client: Arc<LoomClient>) -> Arc<ServerEntry> {
        let reservation = self.reserve_connect(&client.server_url()).await.unwrap();
        self.install(reservation, client).await.unwrap()
    }
}

pub struct ServerEntry {
    pub client: Arc<LoomClient>,
    pub fleet_cancel: CancellationToken,
    pub fleet_reconcile: Arc<Notify>,
    chat: Mutex<ChatState>,
    publication: Mutex<FleetPublication>,
}

struct FleetPublication {
    generation: u64,
    available: bool,
    action_cancel: CancellationToken,
}

impl FleetPublication {
    fn set_available(&mut self, available: bool, retired: &CancellationToken) {
        if !available {
            self.action_cancel.cancel();
        } else if !self.available {
            // Recovery permits new actions, never an old preflight to continue.
            self.action_cancel = retired.child_token();
        }
        self.available = available;
    }
}

#[derive(Default)]
struct ChatState {
    generation: u64,
    session_id: Option<String>,
    cancel: Option<CancellationToken>,
    page_generation: u64,
    older_cursor: Option<ChatCursorView>,
}

#[derive(Clone, Copy)]
pub struct PageTicket {
    generation: u64,
    page_generation: u64,
}

impl ServerEntry {
    pub(crate) fn new(client: Arc<LoomClient>) -> Self {
        let fleet_cancel = CancellationToken::new();
        let action_cancel = fleet_cancel.child_token();
        Self {
            client,
            fleet_cancel,
            fleet_reconcile: Arc::new(Notify::new()),
            chat: Mutex::default(),
            publication: Mutex::new(FleetPublication {
                generation: 0,
                available: true,
                action_cancel,
            }),
        }
    }

    /// Transport transitions invalidate every in-flight REST read immediately.
    pub fn publish_fleet(&self, available: bool, publish: impl FnOnce()) {
        let mut state = self.publication.lock().unwrap();
        if !self.fleet_cancel.is_cancelled() {
            state.generation += 1;
            state.set_available(available, &self.fleet_cancel);
            publish();
        }
    }

    /// Read leases permit authoritative recovery while unavailable, but never
    /// publication after retirement or another producer's newer generation.
    pub fn fleet_ticket(&self) -> Option<u64> {
        let state = self.publication.lock().unwrap();
        (!self.fleet_cancel.is_cancelled()).then_some(state.generation)
    }

    pub fn publish_read(&self, ticket: u64, available: bool, publish: impl FnOnce()) -> bool {
        let mut state = self.publication.lock().unwrap();
        if self.fleet_cancel.is_cancelled() || state.generation != ticket {
            return false;
        }
        state.generation += 1;
        state.set_available(available, &self.fleet_cancel);
        publish();
        true
    }

    /// The connect snapshot was read before this entry became resolvable.
    /// Its generation is therefore zero, never the actor's later start time.
    pub fn publish_initial(&self, publish: impl FnOnce()) -> bool {
        self.publish_read(0, true, publish)
    }

    pub fn request_fleet_reconcile(&self) {
        if !self.fleet_cancel.is_cancelled() {
            self.fleet_reconcile.notify_one();
        }
    }

    /// A completed mutation invalidates reads already in flight even if the
    /// backend is unavailable and a direct refresh cannot start yet.
    pub fn begin_mutation_refresh(&self) -> Option<u64> {
        let mut state = self.publication.lock().unwrap();
        if self.fleet_cancel.is_cancelled() {
            return None;
        }
        state.generation += 1;
        if state.available {
            Some(state.generation)
        } else {
            self.fleet_reconcile.notify_one();
            None
        }
    }

    pub fn mutation_ticket(&self) -> Option<u64> {
        let state = self.publication.lock().unwrap();
        (!self.fleet_cancel.is_cancelled() && state.available).then_some(state.generation)
    }

    /// Selected actions share one availability/retirement lease across all HTTP
    /// reads and writes. Healthy fleet publications do not invalidate it.
    pub(crate) fn selected_action_token(&self) -> Option<CancellationToken> {
        let state = self.publication.lock().unwrap();
        (!self.fleet_cancel.is_cancelled() && state.available).then(|| state.action_cancel.clone())
    }

    /// Serialize with retirement AND availability changes while REST was in flight.
    pub fn publish_mutation(&self, ticket: u64, available: bool, publish: impl FnOnce()) -> bool {
        let mut state = self.publication.lock().unwrap();
        if !self.fleet_cancel.is_cancelled() && state.available && state.generation == ticket {
            state.generation += 1;
            state.set_available(available, &self.fleet_cancel);
            publish();
            if !available {
                // Wake only this entry's actor. The permit is retained even
                // if the actor is not currently awaiting notifications.
                self.fleet_reconcile.notify_one();
            }
            true
        } else {
            false
        }
    }

    fn cancel(&self) {
        let _guard = self.publication.lock().unwrap();
        self.fleet_cancel.cancel();
        let mut chat = self.chat.lock().unwrap();
        if let Some(cancel) = chat.cancel.take() {
            cancel.cancel();
        }
        chat.generation += 1;
        chat.session_id = None;
        chat.older_cursor = None;
    }

    pub fn begin_open(&self, id: &str) -> Result<u64, LoomError> {
        let mut chat = self.chat.lock().unwrap();
        if self.fleet_cancel.is_cancelled() {
            return Err(stale_chat());
        }
        if let Some(cancel) = chat.cancel.take() {
            cancel.cancel();
        }
        chat.generation += 1;
        chat.session_id = Some(id.to_owned());
        chat.older_cursor = None;
        Ok(chat.generation)
    }

    pub fn activate_chat(&self, id: &str, generation: u64) -> Result<CancellationToken, LoomError> {
        let mut chat = self.chat.lock().unwrap();
        if self.fleet_cancel.is_cancelled()
            || chat.generation != generation
            || chat.session_id.as_deref() != Some(id)
        {
            return Err(stale_chat());
        }
        let cancel = self.fleet_cancel.child_token();
        chat.cancel = Some(cancel.clone());
        Ok(cancel)
    }

    pub fn older_cursor(&self, id: &str) -> Result<Option<ChatCursorView>, LoomError> {
        let chat = self.chat.lock().unwrap();
        if self.fleet_cancel.is_cancelled() {
            return Err(stale_chat());
        }
        Ok(if chat.session_id.as_deref() == Some(id) {
            chat.older_cursor.clone()
        } else {
            None
        })
    }

    pub fn begin_page(&self, id: &str) -> Result<PageTicket, LoomError> {
        let mut chat = self.chat.lock().unwrap();
        if self.fleet_cancel.is_cancelled() || chat.session_id.as_deref() != Some(id) {
            return Err(stale_chat());
        }
        chat.page_generation += 1;
        Ok(PageTicket {
            generation: chat.generation,
            page_generation: chat.page_generation,
        })
    }

    /// Reject late pages after a switch, reconnect, or newer page request,
    /// including a switch away and back to the same raw session ID.
    pub fn finish_page(
        &self,
        id: &str,
        ticket: PageTicket,
        cursor: Option<ChatCursorView>,
    ) -> Result<(), LoomError> {
        let mut chat = self.chat.lock().unwrap();
        if self.fleet_cancel.is_cancelled()
            || chat.session_id.as_deref() != Some(id)
            || chat.generation != ticket.generation
            || chat.page_generation != ticket.page_generation
        {
            return Err(stale_chat());
        }
        chat.older_cursor = cursor;
        Ok(())
    }
}

fn stale_chat() -> LoomError {
    LoomError::Api {
        status: 409,
        method: "POST",
        path: "chat".into(),
        message: "chat request superseded by a session switch, reconnect, or newer page".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn client(url: &str) -> Arc<LoomClient> {
        Arc::new(LoomClient::new(url, None).unwrap())
    }

    fn cursor(turn: i64) -> Option<ChatCursorView> {
        Some(ChatCursorView { turn, seq: 2 })
    }

    #[test]
    fn identity_normalizes_equivalent_urls_and_rejects_ambiguous_addresses() {
        assert_eq!(
            normalize_server_url(" HTTP://LOCALHOST:80/// ").unwrap(),
            "http://localhost"
        );
        assert_eq!(
            normalize_server_url("https://EXAMPLE.com:443/loom///").unwrap(),
            "https://example.com/loom"
        );
        assert_ne!(
            normalize_server_url("http://localhost:7878").unwrap(),
            normalize_server_url("http://localhost:7879").unwrap()
        );
        for url in [
            "",
            "file:///tmp/loom",
            "http://user:secret@localhost",
            "https://host/?token=secret",
            "https://host/#chat",
        ] {
            assert!(normalize_server_url(url).is_err(), "{url}");
        }
    }

    #[tokio::test]
    async fn reconnect_retires_only_its_server_and_preserves_other_chat_and_cursor() {
        let registry = ServerRegistry::default();
        let a = registry
            .install_for_test(client("http://localhost:7878"))
            .await;
        let b = registry
            .install_for_test(client("http://localhost:7879"))
            .await;
        let a_generation = a.begin_open("same-id").unwrap();
        let a_chat = a.activate_chat("same-id", a_generation).unwrap();
        let a_page = a.begin_page("same-id").unwrap();
        let b_generation = b.begin_open("same-id").unwrap();
        let b_chat = b.activate_chat("same-id", b_generation).unwrap();
        let b_page = b.begin_page("same-id").unwrap();
        let a_new = registry
            .install_for_test(client("HTTP://LOCALHOST:7878///"))
            .await;
        assert!(a.fleet_cancel.is_cancelled());
        assert!(a_chat.is_cancelled());
        assert!(a.finish_page("same-id", a_page, cursor(10)).is_err());
        assert!(a.activate_chat("same-id", a_generation).is_err());
        assert!(a.older_cursor("same-id").is_err());
        assert!(!a_new.fleet_cancel.is_cancelled());
        assert!(Arc::ptr_eq(
            &registry.resolve("http://localhost:7878/").await.unwrap(),
            &a_new
        ));
        assert!(Arc::ptr_eq(
            &registry.resolve("http://localhost:7879").await.unwrap(),
            &b
        ));
        assert!(!b.fleet_cancel.is_cancelled());
        assert!(!b_chat.is_cancelled());
        b.finish_page("same-id", b_page, cursor(20)).unwrap();
        assert_eq!(b.older_cursor("same-id").unwrap().unwrap().turn, 20);
        assert!(a_new.older_cursor("same-id").unwrap().is_none());
    }

    #[tokio::test]
    async fn newer_failed_attempt_preserves_live_entry_and_retires_older_reservation_only_for_its_url(
    ) {
        let registry = ServerRegistry::default();
        let live = registry
            .install_for_test(client("http://localhost:7878"))
            .await;
        let older = registry
            .reserve_connect("http://localhost:7878")
            .await
            .unwrap();
        let other = registry
            .reserve_connect("http://localhost:7879")
            .await
            .unwrap();
        // The newest attempt fails validation and never installs. It still
        // supersedes the older attempt, while the prior working client stays.
        let _failed = registry
            .reserve_connect("HTTP://LOCALHOST:7878///")
            .await
            .unwrap();
        assert!(registry
            .install(older, client("http://localhost:7878"))
            .await
            .is_err());
        assert!(Arc::ptr_eq(
            &registry.resolve("http://localhost:7878").await.unwrap(),
            &live
        ));
        assert!(!live.fleet_cancel.is_cancelled());
        let other_entry = registry
            .install(other, client("http://localhost:7879"))
            .await
            .unwrap();
        assert!(!other_entry.fleet_cancel.is_cancelled());
    }

    #[tokio::test]
    async fn mutation_recovery_wakes_only_its_entry_and_retired_errors_cannot_wake_replacement() {
        let registry = ServerRegistry::default();
        let a = registry
            .install_for_test(client("http://localhost:7878"))
            .await;
        let b = registry
            .install_for_test(client("http://localhost:7879"))
            .await;
        let ticket = a.mutation_ticket().unwrap();
        a.publish_mutation(ticket, false, || {});
        // A retained permit wakes the actor even when it starts awaiting later.
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            a.fleet_reconcile.notified(),
        )
        .await
        .unwrap();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(50),
            b.fleet_reconcile.notified()
        )
        .await
        .is_err());
        assert!(b.mutation_ticket().is_some());
        let replacement = registry
            .install_for_test(client("http://localhost:7878"))
            .await;
        a.publish_mutation(ticket, false, || panic!("retired error must not publish"));
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(50),
            replacement.fleet_reconcile.notified()
        )
        .await
        .is_err());
        assert!(replacement.mutation_ticket().is_some());
    }

    #[tokio::test]
    async fn sessions_and_page_generations_reject_stale_results_even_after_returning_to_same_id() {
        let registry = ServerRegistry::default();
        let a = registry
            .install_for_test(client("http://localhost:7878"))
            .await;
        let b = registry
            .install_for_test(client("http://localhost:7879"))
            .await;
        let old_open = a.begin_open("shared").unwrap();
        let old_chat = a.activate_chat("shared", old_open).unwrap();
        b.begin_open("shared").unwrap();
        let b_page = b.begin_page("shared").unwrap();
        let older = a.begin_page("shared").unwrap();
        let newest = a.begin_page("shared").unwrap();
        a.finish_page("shared", newest, cursor(9)).unwrap();
        assert!(a.finish_page("shared", older, cursor(30)).is_err());
        assert_eq!(a.older_cursor("shared").unwrap().unwrap().turn, 9);
        assert!(a.older_cursor("different").unwrap().is_none());
        assert!(a.begin_page("different").is_err());
        let delayed = a.begin_page("shared").unwrap();
        a.begin_open("different").unwrap();
        assert!(old_chat.is_cancelled());
        assert!(a.older_cursor("shared").unwrap().is_none());
        a.begin_open("shared").unwrap();
        assert!(a.activate_chat("shared", old_open).is_err());
        assert!(a.finish_page("shared", delayed, cursor(30)).is_err());
        assert!(a.older_cursor("shared").unwrap().is_none());
        b.finish_page("shared", b_page, cursor(40)).unwrap();
        assert_eq!(b.older_cursor("shared").unwrap().unwrap().turn, 40);
    }

    async fn http_server(
        label: &'static str,
        expected_token: &'static str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if request.len() < end + 4 + length {
                        continue;
                    }
                    assert!(headers.starts_with("post /api/routing-probe http/1.1"));
                    assert!(headers.contains(&format!("authorization: bearer {expected_token}")));
                    let body: serde_json::Value =
                        serde_json::from_slice(&request[end + 4..]).unwrap();
                    assert_eq!(body["id"], "shared-raw-id");
                    break;
                }
            }
            let body = serde_json::json!({"server": label}).to_string();
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        (url, task)
    }

    #[tokio::test]
    async fn explicit_routing_sends_same_raw_id_and_independent_credentials_to_selected_server() {
        let registry = ServerRegistry::default();
        let (a_url, a_task) = http_server("a", "a-token").await;
        let (b_url, b_task) = http_server("b", "b-token").await;
        registry
            .install_for_test(Arc::new(
                LoomClient::new(&a_url, Some("a-token".into())).unwrap(),
            ))
            .await;
        // Even with exactly one entry, missing/unknown addresses cannot route globally.
        assert!(registry.resolve("").await.is_err());
        assert!(registry.resolve("http://127.0.0.1:1").await.is_err());
        registry
            .install_for_test(Arc::new(
                LoomClient::new(&b_url, Some("b-token".into())).unwrap(),
            ))
            .await;
        let a = registry.resolve(&format!("{a_url}/")).await.unwrap();
        let b = registry.resolve(&b_url).await.unwrap();
        let args = serde_json::json!({"id": "shared-raw-id"});
        let (a_reply, b_reply) = tokio::join!(
            a.client
                .op::<serde_json::Value>("/api/routing-probe", &args),
            b.client
                .op::<serde_json::Value>("/api/routing-probe", &args),
        );
        assert_eq!(a_reply.unwrap()["server"], "a");
        assert_eq!(b_reply.unwrap()["server"], "b");
        a_task.await.unwrap();
        b_task.await.unwrap();
    }
}
