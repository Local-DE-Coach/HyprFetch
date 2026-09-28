//! End-to-end test for `/ws`: a real WebSocket client connects to a live
//! server, a download runs, and the client must receive `task:progress`,
//! `task:state`, and `global:speed` events without polling.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use hyprfetch_api::AppState;
use hyprfetch_core::{Engine, SsrfPolicy};
use hyprfetch_db::{open_in_memory, TasksRepo};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOTAL: i64 = 2 * 1024 * 1024; // 2 MiB
const SEGMENTS: i64 = 4;
const CHUNK: i64 = TOTAL / SEGMENTS;

async fn mount_download(server: &MockServer) {
    let body: Vec<u8> = (0..TOTAL as usize).map(|i| (i % 251) as u8).collect();

    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-length", TOTAL.to_string())
                .insert_header("accept-ranges", "bytes"),
        )
        .mount(server)
        .await;

    for i in 0..SEGMENTS {
        let start = i * CHUNK;
        let end_inclusive = if i == SEGMENTS - 1 {
            TOTAL - 1
        } else {
            start + CHUNK - 1
        };
        let slice = body[start as usize..=(end_inclusive as usize)].to_vec();
        Mock::given(method("GET"))
            .and(header("range", format!("bytes={start}-{end_inclusive}")))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header(
                        "content-range",
                        format!("bytes {start}-{end_inclusive}/{TOTAL}"),
                    )
                    .insert_header("content-length", slice.len().to_string())
                    .set_body_bytes(slice),
            )
            .mount(server)
            .await;
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Read JSON text frames until `deadline` or until `on_msg` returns true,
/// feeding each parsed event to `on_msg`.
async fn collect_events(
    ws: &mut WsStream,
    deadline: Duration,
    mut on_msg: impl FnMut(serde_json::Value) -> bool,
) {
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        let msg = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        match msg {
            Ok(Some(Ok(Message::Text(t)))) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if on_msg(v) {
                        return;
                    }
                }
            }
            Ok(Some(Ok(_))) => {} // ignore binary/pong frames
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => {} // per-read timeout elapsed; loop checks deadline
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_streams_progress_state_and_global_speed() {
    let server = MockServer::start().await;
    mount_download(&server).await;

    // App state with SSRF disabled so the engine can hit the mock server.
    let db = open_in_memory().unwrap();
    let engine = Engine::with_ssrf_policy(
        db.clone(),
        SsrfPolicy {
            block_private: false,
        },
    );
    // Throttle to ~1 MiB/s so the 2 MiB download spans > 1s and the
    // debounced progress broadcasts + speed ticks actually fire.
    engine.set_qos(true, 1024 * 1024);
    let state = AppState::with_defaults(db.clone(), Arc::new(engine));

    // Bind an ephemeral port and serve.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = hyprfetch_api::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Connect a real WebSocket client.
    let (ws_stream, resp) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("ws handshake must succeed");
    assert_eq!(resp.status(), 101); // switching protocols
    let mut ws = ws_stream;

    // Seed a task and start it AFTER the client is connected so no events
    // are missed.
    let id = uuid::Uuid::now_v7().to_string();
    let now = 0i64;
    let row = hyprfetch_db::TaskRow {
        id: id.clone(),
        url: format!("{}/file.bin", server.uri()),
        filename: "file.bin".into(),
        save_path: "/tmp/hyprfetch-ws-test.bin".into(),
        total_bytes: Some(TOTAL),
        downloaded_bytes: 0,
        state: hyprfetch_db::schema::TaskState::Queued,
        etag: None,
        last_modified: None,
        accept_ranges: false,
        segments_requested: SEGMENTS,
        qos_override: None,
        extra_headers: None,
        error_message: None,
        created_at: now,
        updated_at: now,
        completed_at: None,
    };
    TasksRepo::new(&db).insert(&row).unwrap();
    state.engine.start(&id).await.unwrap();

    // Collect events until we've seen everything, up to 15s.
    let mut saw_progress = false;
    let mut saw_state_downloading = false;
    let mut saw_state_complete = false;
    let mut saw_global_speed = false;
    collect_events(&mut ws, Duration::from_secs(15), |v| {
        match v["event"].as_str() {
            Some("task:progress") => {
                if v["task_id"] == id && v["speed_bps"].as_u64().unwrap_or(0) > 0 {
                    saw_progress = true;
                }
            }
            Some("task:state") => match v["state"].as_str() {
                Some("downloading") => saw_state_downloading = true,
                Some("complete") => saw_state_complete = true,
                _ => {}
            },
            Some("global:speed")
                if v["speed_bps"].as_u64().unwrap_or(0) > 0
                    && v["active_tasks"].as_u64().unwrap_or(0) > 0 =>
            {
                saw_global_speed = true;
            }
            _ => {}
        }
        saw_state_complete && saw_progress && saw_global_speed
    })
    .await;

    let _ = futures::SinkExt::close(&mut ws).await;

    assert!(saw_state_downloading, "never saw task:state downloading");
    assert!(saw_state_complete, "never saw task:state complete");
    assert!(
        saw_progress,
        "never saw a task:progress with non-zero speed"
    );
    assert!(
        saw_global_speed,
        "never saw global:speed with non-zero speed"
    );

    // Sanity: the download really completed.
    let row = TasksRepo::new(&db).get(&id).unwrap().unwrap();
    assert_eq!(row.state, hyprfetch_db::schema::TaskState::Complete);
    let _ = std::fs::remove_file("/tmp/hyprfetch-ws-test.bin");
}

#[tokio::test]
async fn ws_rejects_non_upgrade_requests() {
    use tower::ServiceExt;

    let db = open_in_memory().unwrap();
    let state = AppState::with_defaults(db.clone(), Arc::new(Engine::new(db)));
    let app = hyprfetch_api::router(state);
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/ws")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Missing Upgrade headers → handshake rejected (4xx), not a panic.
    assert!(
        res.status().as_u16() >= 400,
        "expected 4xx for non-upgrade request, got {}",
        res.status()
    );
}
