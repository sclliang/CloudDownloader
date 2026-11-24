use super::*;
use crate::{engine::Engine, store::Store};
use std::{fs, path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{sleep, timeout},
};

// Wry needs Common Controls v6. Link the same Windows manifest into this
// test executable that Tauri's build script already links into the app.
#[cfg(target_env = "msvc")]
#[link(name = "resource", kind = "static", modifiers = "-bundle")]
extern "C" {}

const CONTENT: &[u8] = b"complete non-range download";

async fn http_fixture() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut request = [0; 1024];
                let size = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..size]);
                if request.starts_with("GET /hold ") {
                    sleep(Duration::from_secs(30)).await;
                    return;
                }
                let (status, body) = if request.starts_with("GET /file ") {
                    ("200 OK", CONTENT)
                } else if request.starts_with("GET /announce") {
                    ("200 OK", &b"d8:intervali120e5:peers0:e"[..])
                } else {
                    ("404 Not Found", &[][..])
                };
                let header = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: none\r\nConnection: close\r\n\r\n", body.len());
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(body).await;
            });
        }
    });
    format!("http://{address}")
}

fn record(dir: &Path, name: &str, status: &str, url: &str) -> Task {
    serde_json::from_value(json!({
        "id": name, "name": name, "size": CONTENT.len(), "completed": 0, "speed": 0,
        "status": status, "error": "", "directory": dir, "createdAt": 1, "gid": null,
        "share": { "id": "", "passcode": "" },
        "file": { "id": "", "name": name, "size": CONTENT.len(), "isDir": false, "parentId": "" },
        "direct": { "url": url, "connections": 1, "rangeSupported": false }
    }))
    .unwrap()
}

async fn wait_status(rpc: &crate::Rpc, gid: &str, expected: &str) {
    timeout(Duration::from_secs(10), async {
        loop {
            let result = rpc
                .call("aria2.tellStatus", vec![json!(gid), json!(["status"])])
                .await
                .unwrap();
            assert_ne!(result["status"], "error", "{gid}: {result}");
            if result["status"] == expected {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn pan123_engine_retry_is_bounded_and_preserves_user_pauses_and_permanent_failures() {
    let mut task = record(Path::new("."), "test", "error", "https://example.test/");
    let detail = "SSL/TLS handshake failure: Error: connection closed";
    assert_eq!(task.connection_retries, 0);
    assert!(!super::queue_connection_retry(
        &mut task, "active", "1", detail
    ));
    task.direct = None;
    task.share.provider = crate::Provider::Pan123;
    assert!(!super::queue_connection_retry(
        &mut task, "paused", "1", detail
    ));
    assert!(!super::queue_connection_retry(
        &mut task,
        "active",
        "1",
        "SSL/TLS handshake failure: invalid certificate"
    ));
    assert!(!super::queue_connection_retry(
        &mut task,
        "active",
        "22",
        "status=403"
    ));
    for attempt in 1..=2 {
        task.gid = Some("failed".into());
        assert!(super::queue_connection_retry(
            &mut task, "active", "1", detail
        ));
        assert_eq!(task.connection_retries, attempt);
        assert_eq!(task.status, "waiting");
        assert!(task.gid.is_none());
    }
    assert!(!super::queue_connection_retry(
        &mut task, "active", "1", detail
    ));
}

#[tokio::test]
async fn batches_control_only_recorded_tasks_and_report_individual_outcomes() {
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = "com.sixunited.clouddownloader.batchtest".into();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let dir = std::env::temp_dir().join(format!("clouddownloader-batch-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&dir).unwrap();
    let store = Store::open(&dir.join("tasks.sqlite3")).unwrap();
    let engine = Engine::with_args(&[
        "--enable-dht=false",
        "--enable-dht6=false",
        "--bt-enable-lpd=false",
        "--interface=127.0.0.1",
    ])
    .unwrap();
    let rpc = engine.rpc.clone();
    let state = AppState {
        store: store.into(),
        engine: Some(engine).into(),
        engine_error: Default::default(),
        accounts: Default::default(),
        resolving: Default::default(),
        uploads: Default::default(),
        session_cache_error: Default::default(),
        exiting: Default::default(),
        bt_jobs: Default::default(),
    };
    timeout(Duration::from_secs(5), async {
        while rpc.call("aria2.getVersion", vec![]).await.is_err() {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let base = http_fixture().await;
    let file_url = format!("{base}/file");
    let mut direct = record(&dir, "direct.bin", "active", &file_url);
    let gid = rpc
        .call(
            "aria2.addUri",
            vec![
                json!([format!("{base}/hold")]),
                json!({"dir": dir, "out": direct.name}),
            ],
        )
        .await
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    let magnet = format!("magnet:?xt=urn:btih:{}&tr={base}/announce", "5".repeat(40));
    let untracked = rpc
        .call(
            "aria2.addUri",
            vec![
                json!([magnet]),
                json!({"dir": dir, "bt-metadata-only": "true"}),
            ],
        )
        .await
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    wait_status(&rpc, &untracked, "active").await;
    direct.gid = Some(gid.clone());
    state.store.lock().unwrap().save(&direct).unwrap();
    let paused = apply_batch(app.handle(), &state, None, Action::Pause, false)
        .await
        .unwrap();
    assert_eq!(paused.changed, 1);
    assert!(paused.failures.is_empty());
    wait_status(&rpc, &gid, "paused").await;
    wait_status(&rpc, &untracked, "active").await;

    // A partially downloaded non-range file, a persisted task without a GID,
    // and an unavailable source must produce two real downloads and one failure.
    fs::write(dir.join("direct.bin"), b"old").unwrap();
    fs::write(dir.join("direct.bin.aria2"), b"old checkpoint").unwrap();
    let saved = record(&dir, "restored.bin", "paused", &file_url);
    let broken = record(&dir, "missing.bin", "error", &format!("{base}/missing"));
    {
        let store = state.store.lock().unwrap();
        store.save(&saved).unwrap();
        store.save(&broken).unwrap();
    }
    let resumed = apply_batch(app.handle(), &state, None, Action::Resume, false)
        .await
        .unwrap();
    assert_eq!(resumed.changed, 2);
    assert_eq!(resumed.failures.len(), 1);
    assert_eq!(resumed.failures[0].id, broken.id);
    assert!(resumed.failures[0].message.contains("404"));
    for task in [&direct, &saved] {
        let fresh = state.store.lock().unwrap().task(&task.id).unwrap().unwrap();
        assert_ne!(fresh.gid, task.gid);
        wait_status(&rpc, fresh.gid.as_deref().unwrap(), "complete").await;
        assert_eq!(fs::read(dir.join(&task.name)).unwrap(), CONTENT);
    }

    // Even a responsive engine can reject a status query for a malformed GID.
    // Keep that record intact while still removing an independent selected task.
    direct.gid = Some("invalid".into());
    state.store.lock().unwrap().save(&direct).unwrap();
    let selected = HashSet::from([direct.id.clone(), saved.id.clone()]);
    let removed = apply_batch(app.handle(), &state, Some(selected), Action::Remove, false)
        .await
        .unwrap();
    assert_eq!(removed.changed, 1);
    assert_eq!(removed.failures.len(), 1);
    assert_eq!(removed.failures[0].id, direct.id);
    {
        let store = state.store.lock().unwrap();
        assert_eq!(store.task(&direct.id).unwrap().unwrap().gid, direct.gid);
        assert!(store.task(&saved.id).unwrap().is_none());
        assert!(store.task(&broken.id).unwrap().is_some());
        assert_eq!(fs::read(dir.join(saved.name)).unwrap(), CONTENT);
    }
    drop(state);
    fs::remove_dir_all(dir).unwrap();
}
