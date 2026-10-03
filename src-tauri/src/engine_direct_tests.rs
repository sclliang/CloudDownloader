use super::*;
use crate::model::DirectSource;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn ready(rpc: &Rpc) {
    for _ in 0..50 {
        if rpc.call("aria2.getVersion", vec![]).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("aria2 was not ready");
}
async fn status(rpc: &Rpc, gid: &str) -> Value {
    rpc.call("aria2.tellStatus", vec![json!(gid)])
        .await
        .unwrap()
}
async fn complete(rpc: &Rpc, gid: &str) {
    for _ in 0..300 {
        let value = status(rpc, gid).await;
        assert_ne!(value["status"], "error", "{value}");
        if value["status"] == "complete" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("direct download did not finish");
}

// Real network transfers verify concurrent ranges and byte identity after a
// non-range source restarts from a clean, task-owned partial file.
#[tokio::test]
async fn direct_parallel_ranges_and_non_range_restart_preserve_bytes() {
    let bytes = Arc::new(
        (0..24 * 1024 * 1024)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let fixture = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = fixture.local_addr().unwrap();
    let peak = Arc::new(AtomicUsize::new(0));
    let live = Arc::new(AtomicUsize::new(0));
    let ranges = Arc::new(AtomicUsize::new(0));
    let plain_resumes = Arc::new(AtomicUsize::new(0));
    let invalid_headers = Arc::new(AtomicUsize::new(0));
    let shared = (
        bytes.clone(),
        peak.clone(),
        live.clone(),
        ranges.clone(),
        plain_resumes.clone(),
        invalid_headers.clone(),
    );
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = fixture.accept().await else {
                break;
            };
            let (bytes, peak, live, ranges, plain_resumes, invalid_headers) = shared.clone();
            tokio::spawn(async move {
                let mut input = Vec::new();
                let mut buffer = [0u8; 4096];
                while !input.windows(4).any(|w| w == b"\r\n\r\n") {
                    let Ok(n) = socket.read(&mut buffer).await else {
                        return;
                    };
                    if n == 0 || input.len() > 65536 {
                        return;
                    }
                    input.extend_from_slice(&buffer[..n]);
                }
                let headers = String::from_utf8_lossy(&input).to_lowercase();
                if headers.contains("cookie:")
                    || headers.contains("authorization:")
                    || !headers.contains("if-match: \"direct-v1\"")
                {
                    invalid_headers.fetch_add(1, Ordering::SeqCst);
                }
                let plain = headers.starts_with("get /plain ");
                let range = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="));
                if plain && range.is_some_and(|r| !r.starts_with("0-")) {
                    plain_resumes.fetch_add(1, Ordering::SeqCst);
                }
                let range = if plain { None } else { range };
                let (start, end) = range
                    .map(|r| {
                        let (start, end) = r.trim().split_once('-').unwrap();
                        (
                            start.parse::<usize>().unwrap(),
                            end.parse::<usize>()
                                .unwrap_or(bytes.len() - 1)
                                .min(bytes.len() - 1),
                        )
                    })
                    .unwrap_or((0, bytes.len() - 1));
                if range.is_some() {
                    ranges.fetch_add(1, Ordering::SeqCst);
                }
                let response = if range.is_some() {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\n",
                        bytes.len()
                    )
                } else {
                    "HTTP/1.1 200 OK\r\n".into()
                };
                let response=format!("{response}Content-Length: {}\r\nAccept-Ranges: {}\r\nETag: \"direct-v1\"\r\nConnection: close\r\n\r\n",end-start+1,if plain {"none"} else {"bytes"});
                let count = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                async {
                    if socket.write_all(response.as_bytes()).await.is_err() {
                        return;
                    }
                    for chunk in bytes[start..=end].chunks(65536) {
                        if socket.write_all(chunk).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(8)).await;
                    }
                }
                .await;
                live.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    let directory =
        std::env::temp_dir().join(format!("clouddownloader-direct-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let source = DirectSource {
        url: format!("http://{address}/range"),
        connections: 8,
        range_supported: true,
        etag: Some("\"direct-v1\"".into()),
        last_modified: None,
    };
    let engine = Engine::start().unwrap();
    ready(&engine.rpc).await;
    let gid = engine
        .rpc
        .add_direct(&source, &directory.to_string_lossy(), "parallel.bin")
        .await
        .unwrap();
    let options = engine
        .rpc
        .call("aria2.getOption", vec![json!(gid)])
        .await
        .unwrap();
    assert_eq!(options["split"], "8");
    assert_eq!(options["max-connection-per-server"], "8");
    assert_eq!(options["no-netrc"], "true");
    complete(&engine.rpc, &gid).await;
    assert!(
        peak.load(Ordering::SeqCst) >= 2,
        "Expected simultaneous HTTP connections"
    );
    assert!(
        ranges.load(Ordering::SeqCst) >= 2,
        "Expected multiple ranged requests"
    );
    assert_eq!(
        std::fs::read(directory.join("parallel.bin")).unwrap(),
        *bytes
    );

    engine
        .rpc
        .call(
            "aria2.changeGlobalOption",
            vec![json!({"max-overall-download-limit":"1M"})],
        )
        .await
        .unwrap();
    let plain = DirectSource {
        url: format!("http://{address}/plain"),
        range_supported: false,
        ..source
    };
    let gid = engine
        .rpc
        .add_direct(&plain, &directory.to_string_lossy(), "plain.bin")
        .await
        .unwrap();
    let options = engine
        .rpc
        .call("aria2.getOption", vec![json!(gid)])
        .await
        .unwrap();
    assert_eq!(options["max-connection-per-server"], "1");
    let mut partial = false;
    for _ in 0..100 {
        let value = status(&engine.rpc, &gid).await;
        assert_ne!(value["status"], "error", "{value}");
        let n = value["completedLength"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        if n >= 512 * 1024 && n < bytes.len() as u64 {
            partial = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(partial);
    engine
        .rpc
        .call("aria2.forcePause", vec![json!(gid)])
        .await
        .unwrap();
    for _ in 0..50 {
        if status(&engine.rpc, &gid).await["status"] == "paused" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(status(&engine.rpc, &gid).await["status"], "paused");
    assert!(directory.join("plain.bin.aria2").is_file());
    drop(engine);
    let resumed = Engine::start().unwrap();
    ready(&resumed.rpc).await;
    let gid = resumed
        .rpc
        .add_direct(&plain, &directory.to_string_lossy(), "plain.bin")
        .await
        .unwrap();
    complete(&resumed.rpc, &gid).await;
    assert_eq!(
        plain_resumes.load(Ordering::SeqCst),
        0,
        "A non-range source must restart from byte zero"
    );
    assert_eq!(std::fs::read(directory.join("plain.bin")).unwrap(), *bytes);
    assert_eq!(
        invalid_headers.load(Ordering::SeqCst),
        0,
        "Direct requests must not contain cloud credentials"
    );
    drop(resumed);
    server.abort();
    std::fs::remove_file(directory.join("parallel.bin")).unwrap();
    std::fs::remove_file(directory.join("plain.bin")).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
