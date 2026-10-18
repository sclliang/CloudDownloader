use super::*;
use crate::{bt, model::*};
use base64::Engine as _;
use sha1::{Digest, Sha1};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}
fn local_engine(port: u16) -> Engine {
    Engine::with_args(&[
        "--enable-dht=false",
        "--enable-dht6=false",
        "--bt-enable-lpd=false",
        "--enable-peer-exchange=false",
        &format!("--listen-port={port}"),
        "--bt-tracker-interval=1",
    ])
    .unwrap()
}
async fn ready(rpc: &Rpc) {
    for _ in 0..50 {
        if rpc.call("aria2.getVersion", vec![]).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("aria2 not ready");
}
async fn status(rpc: &Rpc, gid: &str) -> Value {
    rpc.call("aria2.tellStatus", vec![json!(gid)])
        .await
        .unwrap()
}
async fn until(rpc: &Rpc, gid: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    let mut last = Value::Null;
    for _ in 0..400 {
        last = status(rpc, gid).await;
        assert_ne!(last["status"], "error", "BT status: {last}");
        if predicate(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("BT condition timed out: {last}");
}
fn make_torrent(tracker: &str, payload: &[u8]) -> Vec<u8> {
    let pieces: Vec<_> = payload
        .chunks(16384)
        .flat_map(|p| Sha1::digest(p).to_vec())
        .collect();
    // Two files with an intentionally unaligned boundary exercise selected-file accounting.
    let mut data = format!("d8:announce{}:{}4:infod5:filesld6:lengthi98309e4:pathl5:a.bineed6:lengthi{}e4:pathl5:b.bineee4:name7:fixture12:piece lengthi16384e6:pieces{}:", tracker.len(), tracker, payload.len()-98309, pieces.len()).into_bytes();
    data.extend(pieces);
    data.extend(b"ee");
    data
}
fn task(directory: &Path, torrent: &bt::Torrent) -> Task {
    Task {
        id: uuid::Uuid::new_v4().to_string(),
        name: torrent.name.clone(),
        size: torrent.files[1].size,
        completed: 0,
        speed: 0,
        status: "paused".into(),
        error: String::new(),
        directory: directory.to_string_lossy().into(),
        created_at: now(),
        gid: None,
        share: ShareLink {
            provider: Provider::Quark,
            id: "".into(),
            passcode: "".into(),
        },
        file: ShareFile {
            id: "".into(),
            name: torrent.name.clone(),
            size: 0,
            is_dir: false,
            parent_id: "".into(),
            token: "".into(),
        },
        own: None,
        direct: None,
        connections: 0,
        upload_speed: 0,
        bt: Some(BtSource {
            info_hash: torrent.info_hash.clone(),
            piece_length: torrent.piece_length,
            files: torrent.files.clone(),
            selected: vec![2],
            max_peers: 64,
            upload_limit: 512,
            seed_minutes: 0,
        }),
    }
}

#[tokio::test]
async fn actual_bt_magnet_metadata_selection_pause_restart_and_cleanup() {
    let root = std::env::temp_dir().join(format!(
        "clouddownloader-bt-transfer-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&root).unwrap();
    let seed_dir = root.join("seed");
    let target = root.join("download");
    let meta_dir = root.join("metadata");
    for dir in [&seed_dir, &target, &meta_dir] {
        std::fs::create_dir(dir).unwrap();
    }
    let seed_port = port();
    let engine = local_engine(port());
    let seeder = local_engine(seed_port);
    ready(&engine.rpc).await;
    ready(&seeder.rpc).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tracker = format!("http://{}/announce", listener.local_addr().unwrap());
    let tracker_task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut input = [0u8; 8192];
                if socket.read(&mut input).await.is_err() {
                    return;
                }
                let mut body = b"d8:completei1e10:incompletei0e8:intervali1e5:peers6:".to_vec();
                body.extend([127, 0, 0, 1]);
                body.extend(seed_port.to_be_bytes());
                body.push(b'e');
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    let payload: Vec<_> = (0..768 * 1024)
        .map(|i| ((i * 13 + i / 1024) % 251) as u8)
        .collect();
    let data = make_torrent(&tracker, &payload);
    let torrent = bt::parse(&data).unwrap();
    std::fs::create_dir(seed_dir.join("fixture")).unwrap();
    std::fs::write(seed_dir.join("fixture/a.bin"), &payload[..98309]).unwrap();
    std::fs::write(seed_dir.join("fixture/b.bin"), &payload[98309..]).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
    let seed_gid = seeder.rpc.call("aria2.addTorrent", vec![json!(encoded), json!([]), json!({"dir":seed_dir, "check-integrity":"true", "seed-time":"60", "seed-ratio":"0.0", "rpc-save-upload-metadata":"false"})]).await.unwrap().as_str().unwrap().to_string();
    until(&seeder.rpc, &seed_gid, |v| v["seeder"] == "true").await;

    let mut magnet =
        url::Url::parse(&format!("magnet:?xt=urn:btih:{}", torrent.info_hash)).unwrap();
    magnet.query_pairs_mut().append_pair("tr", &tracker);
    let (_, magnet) = bt::magnet(magnet.as_str()).unwrap();
    let meta_gid = engine.rpc.call("aria2.addUri", vec![json!([magnet]), json!({"dir":meta_dir, "bt-metadata-only":"true", "bt-save-metadata":"true", "seed-time":"0"})]).await.unwrap().as_str().unwrap().to_string();
    until(&engine.rpc, &meta_gid, |v| v["status"] == "complete").await;
    let meta_path = meta_dir.join(format!("{}.torrent", torrent.info_hash));
    assert_eq!(
        bt::parse(&bt::read(&meta_path).unwrap()).unwrap().files,
        torrent.files
    );
    assert!(
        !meta_dir.join("fixture").exists(),
        "metadata preview wrote payload files"
    );
    engine.rpc.stop_for_removal(&meta_gid).await.unwrap();

    let mut task = task(&target, &torrent);
    bt::mark(&task).unwrap();
    // Limit payload traffic long enough to verify real partial-file continuation.
    engine
        .rpc
        .call(
            "aria2.changeGlobalOption",
            vec![json!({"max-overall-download-limit":"64K"})],
        )
        .await
        .unwrap();
    let gid = engine.rpc.add_bt(&data, &task).await.unwrap();
    task.gid = Some(gid.clone());
    let partial = until(&engine.rpc, &gid, |v| {
        v["completedLength"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|n| n > 0 && n < payload.len() as u64 / 2)
    })
    .await;
    let (size, done) = crate::bt_progress(task.bt.as_ref().unwrap(), &partial);
    assert_eq!(size, payload.len() as u64 - 98309);
    assert!(done <= size);
    engine
        .rpc
        .call("aria2.forcePause", vec![json!(gid)])
        .await
        .unwrap();
    until(&engine.rpc, &gid, |v| v["status"] == "paused").await;
    assert!(target.join("fixture.aria2").is_file());
    drop(engine);
    let engine = local_engine(port());
    ready(&engine.rpc).await;
    let gid = engine.rpc.add_bt(&data, &task).await.unwrap();
    let complete = until(&engine.rpc, &gid, |v| v["status"] == "complete").await;
    assert_eq!(
        crate::bt_progress(task.bt.as_ref().unwrap(), &complete),
        (size, size)
    );
    assert_eq!(
        std::fs::read(target.join("fixture/b.bin")).unwrap(),
        payload[98309..]
    );
    let unselected = target.join("fixture/a.bin");
    if unselected.exists() {
        let bytes = std::fs::read(&unselected).unwrap();
        assert_eq!(bytes.len(), 98309);
        assert!(bytes[..98304].iter().all(|b| *b == 0));
        assert_eq!(&bytes[98304..], &payload[98304..98309]);
    }
    engine.rpc.stop_for_removal(&gid).await.unwrap();
    task.bt.as_mut().unwrap().seed_minutes = 30;
    let gid = engine.rpc.add_bt(&data, &task).await.unwrap();
    until(&engine.rpc, &gid, |v| {
        v["status"] == "active" && crate::bt_progress(task.bt.as_ref().unwrap(), v) == (size, size)
    })
    .await;
    let options = engine
        .rpc
        .call("aria2.getOption", vec![json!(gid)])
        .await
        .unwrap();
    assert_eq!(options["seed-time"], "30");
    engine
        .rpc
        .call("aria2.forcePause", vec![json!(gid)])
        .await
        .unwrap();
    until(&engine.rpc, &gid, |v| v["status"] == "paused").await;
    engine.rpc.stop_for_removal(&gid).await.unwrap();
    seeder.rpc.stop_for_removal(&seed_gid).await.unwrap();
    drop(engine);
    drop(seeder);
    tracker_task.abort();
    // Explicit task cleanup preserves extra files and the original imported seed.
    std::fs::write(target.join("user-note.txt"), b"keep").unwrap();
    let store = crate::store::Store::open(&root.join("tasks.sqlite3")).unwrap();
    store.save(&task).unwrap();
    crate::task_removal::remove_record(&store, &task, true).unwrap();
    assert!(target.join("user-note.txt").is_file());
    assert!(!target.join("fixture/b.bin").exists());
    assert!(meta_path.is_file());
    drop(store);
    // This uniquely created test directory contains fixture data only.
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("clouddownloader-bt-transfer-"));
    std::fs::remove_dir_all(root).unwrap();
}
