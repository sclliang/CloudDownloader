use reqwest::Client;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

#[cfg(test)]
#[path = "engine_direct_tests.rs"]
mod direct_tests;

#[derive(Clone)]
pub struct Rpc {
    client: Client,
    endpoint: String,
    secret: String,
}
pub struct Engine {
    pub rpc: Rpc,
    child: Child,
}

impl Engine {
    pub fn start() -> Result<Self, String> {
        let executable = find_binary()?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let secret = uuid::Uuid::new_v4().to_string();
        let mut cmd = Command::new(executable);
        cmd.args([
            "--no-conf=true",
            "--enable-rpc=true",
            "--rpc-listen-all=false",
            "--rpc-allow-origin-all=false",
            "--quiet=true",
            "--summary-interval=0",
            "--download-result=hide",
            "--file-allocation=none",
            "--max-concurrent-downloads=3",
            "--split=4",
            "--max-connection-per-server=4",
            "--min-split-size=4M",
            "--max-tries=3",
            "--retry-wait=2",
            "--continue=true",
            "--allow-overwrite=false",
            "--auto-file-renaming=false",
            "--auto-save-interval=2",
            "--check-certificate=true",
            "--http-accept-gzip=false",
        ])
        .arg(format!("--stop-with-process={}", std::process::id()))
        .arg(format!("--rpc-listen-port={port}"))
        .arg(format!("--rpc-secret={secret}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        drop(listener);
        let child = cmd.spawn().map_err(|e| format!("下载引擎启动失败：{e}"))?;
        let rpc = Rpc {
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .no_proxy()
                .build()
                .map_err(|e| e.to_string())?,
            endpoint: format!("http://127.0.0.1:{port}/jsonrpc"),
            secret,
        };
        Ok(Self { rpc, child })
    }
    pub fn alive(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find_binary() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut candidates = vec![current
        .parent()
        .unwrap_or(Path::new("."))
        .join("aria2c.exe")];
    if cfg!(debug_assertions) {
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("binaries/aria2c-x86_64-pc-windows-msvc.exe"),
        );
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or("缺少下载引擎 aria2c.exe，请重新安装完整程序".into())
}

impl Rpc {
    pub async fn call(&self, method: &str, mut params: Vec<Value>) -> Result<Value, String> {
        params.insert(0, json!(format!("token:{}", self.secret)));
        let response = self
            .client
            .post(&self.endpoint)
            .json(&json!({"jsonrpc":"2.0","id":"clouddownloader","method":method,"params":params}))
            .send()
            .await
            .map_err(|_| "下载引擎连接失败，请重新启动程序")?;
        let data: Value = response.json().await.map_err(|_| "下载引擎响应无效")?;
        if data.get("error").is_some() {
            return Err(data["error"]["message"]
                .as_str()
                .unwrap_or("下载引擎操作失败")
                .into());
        }
        Ok(data["result"].clone())
    }
    pub async fn add(
        &self,
        url: &str,
        directory: &str,
        name: &str,
        headers: &[String],
        user_agent: &str,
    ) -> Result<String, String> {
        self.add_with_options(url, directory, name, headers, user_agent, json!({}))
            .await
    }

    pub async fn add_direct(
        &self,
        source: &crate::model::DirectSource,
        directory: &str,
        name: &str,
    ) -> Result<String, String> {
        crate::direct::validate_url(&source.url)?;
        crate::direct::validate_connections(source.connections)?;
        if !source.range_supported {
            crate::direct::restart_without_ranges(Path::new(directory), name)?;
        }
        let connections = if source.range_supported {
            source.connections
        } else {
            1
        };
        let mut headers = Vec::new();
        if let Some(etag) = &source.etag {
            headers.push(format!("If-Match: {etag}"));
        } else if let Some(modified) = &source.last_modified {
            headers.push(format!("If-Unmodified-Since: {modified}"));
        }
        self.add_with_options(&source.url, directory, name, &headers, crate::direct::USER_AGENT,
            json!({"split": connections.to_string(), "max-connection-per-server": connections.to_string(),
                "min-split-size":"1M", "always-resume":"true",
                "no-netrc":"true", "follow-torrent":"false", "follow-metalink":"false"})).await
    }

    async fn add_with_options(
        &self,
        url: &str,
        directory: &str,
        name: &str,
        headers: &[String],
        user_agent: &str,
        mut options: Value,
    ) -> Result<String, String> {
        if headers.iter().any(|v| v.contains(['\r', '\n'])) || user_agent.contains(['\r', '\n']) {
            return Err("下载请求头无效".into());
        }
        options["dir"] = json!(directory);
        options["out"] = json!(name);
        options["header"] = json!(headers);
        options["user-agent"] = json!(user_agent);
        let result = self
            .call("aria2.addUri", vec![json!([url]), options])
            .await?;
        result
            .as_str()
            .map(String::from)
            .ok_or("下载任务创建失败".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        panic!("aria2 did not become ready");
    }

    async fn status(rpc: &Rpc, gid: &str) -> Value {
        rpc.call("aria2.tellStatus", vec![json!(gid)])
            .await
            .unwrap()
    }

    // Real aria2 + a local HTTP Range fixture; no cloud account or network required.
    #[tokio::test(flavor = "current_thread")]
    async fn downloads_pause_and_resume_across_engine_restart() {
        let bytes: Arc<Vec<u8>> =
            Arc::new((0..12 * 1024 * 1024).map(|i| (i % 251) as u8).collect());
        let fixture = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = fixture.local_addr().unwrap();
        let range_requests = Arc::new(AtomicUsize::new(0));
        let fixture_bytes = bytes.clone();
        let fixture_ranges = range_requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = fixture.accept().await else {
                    break;
                };
                let bytes = fixture_bytes.clone();
                let ranges = fixture_ranges.clone();
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
                    let range = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("range: bytes="));
                    let (start, end) = range
                        .map(|value| {
                            let (start, end) = value.trim().split_once('-').unwrap();
                            (
                                start.parse::<usize>().unwrap(),
                                end.parse::<usize>()
                                    .unwrap_or(bytes.len() - 1)
                                    .min(bytes.len() - 1),
                            )
                        })
                        .unwrap_or((0, bytes.len() - 1));
                    let response = if range.is_some() {
                        ranges.fetch_add(1, Ordering::SeqCst);
                        format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\n", bytes.len())
                    } else {
                        "HTTP/1.1 200 OK\r\n".into()
                    };
                    let response = format!("{response}Content-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Type: application/octet-stream\r\nETag: \"stable-fixture\"\r\nConnection: close\r\n\r\n", end-start+1);
                    if socket.write_all(response.as_bytes()).await.is_err()
                        || headers.starts_with("head ")
                    {
                        return;
                    }
                    for chunk in bytes[start..=end].chunks(65536) {
                        if socket.write_all(chunk).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                });
            }
        });
        let directory =
            std::env::temp_dir().join(format!("clouddownloader-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("fixture.bin");
        let engine = Engine::start().unwrap();
        ready(&engine.rpc).await;
        engine
            .rpc
            .call(
                "aria2.changeGlobalOption",
                vec![json!({"max-overall-download-limit":"1M"})],
            )
            .await
            .unwrap();
        let gid = engine
            .rpc
            .add(
                &format!("http://{address}/fixture?revision=1"),
                &directory.to_string_lossy(),
                "fixture.bin",
                &[],
                "CloudDownloader-test",
            )
            .await
            .unwrap();
        let mut partial = 0;
        for _ in 0..100 {
            let value = status(&engine.rpc, &gid).await;
            partial = value["completedLength"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            if partial >= 512 * 1024 {
                break;
            }
            assert_ne!(value["status"], "error", "{value}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(partial >= 512 * 1024 && partial < bytes.len() as u64);
        engine
            .rpc
            .call("aria2.forcePause", vec![json!(gid)])
            .await
            .unwrap();
        for _ in 0..30 {
            if status(&engine.rpc, &gid).await["status"] == "paused" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let paused = status(&engine.rpc, &gid).await;
        assert_eq!(paused["status"], "paused");
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(
            status(&engine.rpc, &gid).await["completedLength"],
            paused["completedLength"]
        );
        assert!(directory.join("fixture.bin.aria2").is_file());
        drop(engine);
        let resumed = Engine::start().unwrap();
        ready(&resumed.rpc).await;
        let new_gid = resumed
            .rpc
            .add(
                &format!("http://{address}/fixture?revision=2"),
                &directory.to_string_lossy(),
                "fixture.bin",
                &[],
                "CloudDownloader-test",
            )
            .await
            .unwrap();
        let mut complete = false;
        for _ in 0..300 {
            let value = status(&resumed.rpc, &new_gid).await;
            assert_ne!(value["status"], "error", "{value}");
            if value["status"] == "complete" {
                complete = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(complete, "resumed transfer timed out");
        assert!(range_requests.load(Ordering::SeqCst) > 0);
        assert_eq!(std::fs::read(&path).unwrap(), *bytes);
        drop(resumed);
        server.abort();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&directory).unwrap();
    }
}
