use crate::{bt, direct, model::*, providers, rpc, session, start_task, trusted, AppState};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tauri::{Manager, State, WebviewWindow};

pub struct MetadataJob {
    gid: String,
    directory: PathBuf,
    hash: String,
    started: Instant,
}
pub type Jobs = tokio::sync::Mutex<HashMap<String, MetadataJob>>;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Inspection {
    Share {
        listing: ShareListing,
    },
    Direct {
        url: String,
        name: String,
        size: u64,
        #[serde(rename = "rangeSupported")]
        range_supported: bool,
    },
    Bt {
        torrent: bt::Torrent,
    },
    BtPending {
        id: String,
        message: String,
    },
}

pub fn candidate(text: &str) -> Result<String, String> {
    if text.len() > 32768 {
        return Err("链接内容过长".into());
    }
    let literal = text.trim();
    if literal
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("magnet:"))
        && !literal.chars().any(char::is_whitespace)
    {
        return Ok(literal.to_string());
    }
    // A plain URL may legitimately contain parentheses. Only strip Markdown
    // punctuation when extracting a URL from surrounding share text.
    if !literal.contains(['，', '。', '；', '）']) && direct::validate_url(literal).is_ok() {
        return Ok(literal.to_string());
    }
    let pattern = regex::Regex::new(r#"(?i)(?:https?://|magnet:\?)[^\s<>\"，。；）)\]]+"#).unwrap();
    let mut links: Vec<_> = pattern.find_iter(text).map(|m| m.as_str()).collect();
    links.dedup();
    if links.len() != 1 {
        return Err("请每次粘贴一个文件链接、网盘分享或磁力链接，也可导入 .torrent 文件".into());
    }
    Ok(links[0].to_string())
}

// A cloud download failure must never silently download its HTML share page.
pub fn cloud_route(input: &str) -> bool {
    let Ok(u) = url::Url::parse(input) else {
        return false;
    };
    let host = u.host_str().unwrap_or_default();
    let path = u.path();
    let known = [
        "pan.quark.cn",
        "pan.baidu.com",
        "115.com",
        "www.115.com",
        "115cdn.com",
        "www.115cdn.com",
        "115rc.com",
        "www.115rc.com",
        "anxia.com",
        "123pan.com",
        "www.123pan.com",
        "123pan.cn",
        "www.123pan.cn",
        "123865.com",
        "www.123865.com",
        "123912.com",
        "www.123912.com",
        "yun.139.com",
        "drive.uc.cn",
        "fast.uc.cn",
        "ilanzou.com",
        "www.ilanzou.com",
    ]
    .contains(&host)
        || host_matches(host, "share.123pan.cn");
    lanzou_host(host)
        || (known
            && (path == "/"
                || path.starts_with("/s/")
                || path.starts_with("/share")
                || path.starts_with("/123pan/")
                || path == "/api/srr"
                || providers::parse_share(input, "").is_ok()))
}

async fn fetch_torrent(input: &str) -> Result<Vec<u8>, String> {
    let url = direct::validate_url(input)?;
    let client = reqwest::Client::builder()
        .user_agent(direct::USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 10 || direct::validate_url(a.url().as_str()).is_err() {
                a.error("Invalid torrent redirect")
            } else {
                a.follow()
            }
        }))
        .build()
        .map_err(|_| "无法初始化种子下载")?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "获取种子文件失败，请检查网络和链接")?;
    if !response.status().is_success() {
        return Err(format!(
            "获取种子文件失败（HTTP {}）",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "种子文件读取失败")? {
        if bytes.len() + chunk.len() > bt::MAX_TORRENT {
            return Err("种子文件超过 4 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[tauri::command]
pub async fn inspect_download(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    text: String,
    code: String,
    local_file: Option<String>,
) -> Result<Inspection, String> {
    trusted(&window)?;
    let base = app.path().app_data_dir().map_err(|_| "无法定位种子缓存")?;
    if let Some(local) = local_file {
        let path = Path::new(&local);
        if !path.is_absolute()
            || !path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("torrent"))
        {
            return Err("请选择本地 .torrent 文件".into());
        }
        return Ok(Inspection::Bt {
            torrent: bt::cache(&base, &bt::read(path)?)?,
        });
    }
    let link = candidate(&text)?;
    if link
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("magnet:"))
    {
        let (hash, magnet) = bt::magnet(&link)?;
        if let Ok((torrent, _)) = bt::load(&base, &hash) {
            return Ok(Inspection::Bt { torrent });
        }
        let mut jobs = state.bt_jobs.lock().await;
        if jobs.len() >= 3 {
            return Err("正在解析的磁力链接过多，请取消其他解析后重试".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let directory = base.join("bt-staging").join(&id);
        std::fs::create_dir_all(&directory).map_err(|_| "无法创建磁力解析目录")?;
        let result = rpc(&state)?.call("aria2.addUri", vec![json!([magnet]), json!({"dir": directory, "bt-metadata-only":"true", "bt-save-metadata":"true", "bt-max-peers":"64", "seed-time":"0", "follow-torrent":"false"})]).await;
        let gid = match result {
            Ok(v) => v.as_str().map(String::from).ok_or("磁力解析任务创建失败")?,
            Err(e) => {
                let _ = std::fs::remove_dir(&directory);
                return Err(e);
            }
        };
        jobs.insert(
            id.clone(),
            MetadataJob {
                gid,
                directory,
                hash,
                started: Instant::now(),
            },
        );
        return Ok(Inspection::BtPending {
            id,
            message: "正在向节点获取种子信息，尚未下载文件…".into(),
        });
    }
    if cloud_route(&link) {
        let share = providers::parse_share(&text, &code)?;
        let listing = session::connect(&app, share.provider)
            .await?
            .list(&share, "0")
            .await?;
        return Ok(Inspection::Share { listing });
    }
    let url = direct::validate_url(&link)?.to_string();
    inspect_http(&base, &url).await
}

async fn inspect_http(base: &Path, url: &str) -> Result<Inspection, String> {
    if url::Url::parse(&url).is_ok_and(|v| v.path().to_ascii_lowercase().ends_with(".torrent")) {
        return Ok(Inspection::Bt {
            torrent: bt::cache(&base, &fetch_torrent(&url).await?)?,
        });
    }
    let metadata = direct::probe(&url).await?;
    if metadata.name.to_ascii_lowercase().ends_with(".torrent")
        || metadata.content_type.as_deref().is_some_and(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/x-bittorrent")
        })
    {
        return Ok(Inspection::Bt {
            torrent: bt::cache(&base, &fetch_torrent(&url).await?)?,
        });
    }
    Ok(Inspection::Direct {
        url: url.to_string(),
        name: metadata.name,
        size: metadata.size,
        range_supported: metadata.range_supported,
    })
}

async fn clean(job: &MetadataJob, rpc: &crate::engine::Rpc) -> Result<(), String> {
    rpc.stop_for_removal(&job.gid).await?;
    let _ = std::fs::remove_file(job.directory.join(format!("{}.torrent", job.hash)));
    // Never recurse: only the metadata file generated by our own job belongs here.
    let _ = std::fs::remove_dir(&job.directory);
    Ok(())
}

#[tauri::command]
pub async fn cancel_bt_inspection(
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    trusted(&window)?;
    let mut jobs = state.bt_jobs.lock().await;
    if let Some(job) = jobs.get(&id) {
        clean(job, &rpc(&state)?).await?;
        jobs.remove(&id);
    }
    Ok(())
}

pub async fn expire_jobs(state: &AppState, rpc: &crate::engine::Rpc) {
    let Ok(mut jobs) = state.bt_jobs.try_lock() else {
        return;
    };
    let ids: Vec<_> = jobs
        .iter()
        .filter(|(_, job)| job.started.elapsed() > Duration::from_secs(180))
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        if clean(&jobs[&id], rpc).await.is_ok() {
            jobs.remove(&id);
        }
    }
}

#[tauri::command]
pub async fn poll_bt_inspection(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Inspection, String> {
    trusted(&window)?;
    let mut jobs = state.bt_jobs.lock().await;
    let job = jobs.get(&id).ok_or("磁力解析已取消或超时，请重新解析")?;
    let rpc = rpc(&state)?;
    let status = rpc
        .call(
            "aria2.tellStatus",
            vec![json!(job.gid), json!(["status", "errorCode"])],
        )
        .await?;
    let path = job.directory.join(format!("{}.torrent", job.hash));
    if status["status"] == "complete" && path.is_file() {
        let result = (|| {
            let data = bt::read(&path)?;
            if bt::parse(&data)?.info_hash != job.hash {
                return Err("磁力链接返回的种子哈希不一致".into());
            }
            let base = app.path().app_data_dir().map_err(|_| "无法定位种子缓存")?;
            bt::cache(&base, &data)
        })();
        clean(job, &rpc).await?;
        jobs.remove(&id);
        return result.map(|torrent| Inspection::Bt { torrent });
    }
    if matches!(status["status"].as_str(), Some("error" | "removed"))
        || job.started.elapsed() > Duration::from_secs(175)
    {
        clean(job, &rpc).await?;
        jobs.remove(&id);
        return Err("未获取到种子信息，请检查网络、Tracker 或资源是否有在线节点，也可直接导入 .torrent 文件".into());
    }
    Ok(Inspection::BtPending {
        id,
        message: if status["status"] == "waiting" {
            "等待下载引擎空闲以获取种子信息…"
        } else {
            "正在向节点获取种子信息，尚未下载文件…"
        }
        .into(),
    })
}

#[tauri::command]
pub async fn enqueue_bt(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    info_hash: String,
    selected: Vec<u32>,
    max_peers: u32,
    upload_limit: u32,
    seed_minutes: u32,
) -> Result<Task, String> {
    trusted(&window)?;
    if !(1..=500).contains(&max_peers) || upload_limit > 1_048_576 || seed_minutes > 1440 {
        return Err("BT 参数无效：节点 1–500、上传上限 0–1048576 KiB/s、做种 0–1440 分钟".into());
    }
    let _guard = state.resolving.lock().await;
    let base = app.path().app_data_dir().map_err(|_| "无法定位种子缓存")?;
    let (torrent, _) = bt::load(&base, &info_hash)?;
    let mut selected = selected;
    selected.sort_unstable();
    selected.dedup();
    if selected.is_empty()
        || selected
            .iter()
            .any(|i| *i == 0 || *i > torrent.files.len() as u32)
    {
        return Err("请至少选择一个有效的种子文件".into());
    }
    let directory = {
        let store = state.store.lock().unwrap();
        if store.tasks()?.iter().any(|t| {
            t.bt.as_ref().is_some_and(|b| b.info_hash == info_hash)
                && !matches!(t.status.as_str(), "complete" | "error" | "cancelled")
        }) {
            return Err("该种子已在任务列表中，请继续现有任务或先移除旧任务".into());
        }
        store.setting("download_dir").ok_or("请先选择下载目录")?
    };
    let root = Path::new(&directory);
    if !root.is_absolute() || !root.is_dir() {
        return Err("下载目录不存在，请重新选择".into());
    }
    bt::plain_path(root)?;
    let id = uuid::Uuid::new_v4().to_string();
    let directory = root.join(format!(
        "{} (BT {})",
        torrent.name.chars().take(60).collect::<String>(),
        &id[..8]
    ));
    std::fs::create_dir(&directory).map_err(|_| "无法创建 BT 任务独立目录")?;
    let size = torrent
        .files
        .iter()
        .filter(|f| selected.contains(&f.index))
        .map(|f| f.size)
        .sum();
    let task = Task {
        id,
        name: torrent.name.clone(),
        size,
        completed: 0,
        speed: 0,
        status: "resolving".into(),
        error: String::new(),
        directory: directory.to_string_lossy().into(),
        created_at: now(),
        gid: None,
        share: ShareLink {
            provider: Provider::Quark,
            id: String::new(),
            passcode: String::new(),
        },
        file: ShareFile {
            id: String::new(),
            name: torrent.name,
            size,
            is_dir: false,
            parent_id: String::new(),
            token: String::new(),
        },
        own: None,
        direct: None,
        connections: 0,
        upload_speed: 0,
        bt: Some(BtSource {
            info_hash,
            piece_length: torrent.piece_length,
            files: torrent.files,
            selected,
            max_peers,
            upload_limit,
            seed_minutes,
        }),
    };
    bt::mark(&task)?;
    start_task(&app, &state, task, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_source_classification_preserves_cloud_failures() {
        for link in [
            "https://pan.baidu.com/s/1test",
            "https://yun.139.com/shareweb/#/w/i/test",
            "https://a.lanzouo.com/itest",
            "https://drive.uc.cn/s/abc",
            "https://www.ilanzou.com/s/abc",
            "https://115.com/s/abc?password=bad",
        ] {
            assert!(cloud_route(link), "{link}");
        }
        for link in [
            "https://example.com/a.zip",
            "https://pan.baidu.com/file.zip",
            "https://evil.com/s/abc",
        ] {
            assert!(!cloud_route(link));
        }
        assert!(candidate("https://a.com/a https://b.com/b").is_err());
        assert_eq!(
            candidate("magnet:?dn=file(1)&xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .unwrap(),
            "magnet:?dn=file(1)&xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            candidate("https://example.com/file_(1).zip").unwrap(),
            "https://example.com/file_(1).zip"
        );
        assert_eq!(
            candidate("[文件](https://example.com/file.zip)").unwrap(),
            "https://example.com/file.zip"
        );
        assert_eq!(
            candidate("分享 https://pan.quark.cn/s/abcd 提取码：1234").unwrap(),
            "https://pan.quark.cn/s/abcd"
        );
    }

    #[tokio::test]
    async fn actual_http_inspection_detects_torrent_by_path_or_mime_and_bounds_metadata() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut torrent =
            b"d4:infod6:lengthi3e4:name5:a.txt12:piece lengthi16384e6:pieces20:".to_vec();
        torrent.extend([0u8; 20]);
        torrent.extend(b"ee");
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let torrent = torrent.clone();
                tokio::spawn(async move {
                    let mut input = vec![0; 8192];
                    let n = socket.read(&mut input).await.unwrap();
                    let request = String::from_utf8_lossy(&input[..n]).to_lowercase();
                    assert!(!request.contains("cookie:") && !request.contains("authorization:"));
                    let huge = request.starts_with("get /huge.torrent ");
                    let ordinary = request.starts_with("get /file.zip ");
                    let range = request.contains("range: bytes=0-0");
                    // Explicit .torrent URLs must work even when Range is rejected.
                    if range && request.starts_with("get /seed.torrent ") {
                        socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                        return;
                    }
                    let body = if huge {
                        vec![0u8; bt::MAX_TORRENT + 1]
                    } else if ordinary {
                        b"archive".to_vec()
                    } else {
                        torrent
                    };
                    let mime = if ordinary {
                        "application/zip"
                    } else {
                        "application/x-bittorrent"
                    };
                    let head = if range {
                        format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/{}\r\nContent-Length: 1\r\n", body.len())
                    } else {
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", body.len())
                    };
                    let head = format!("{head}Content-Type: {mime}\r\nConnection: close\r\n\r\n");
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket
                        .write_all(if range { &body[..1] } else { &body })
                        .await;
                });
            }
        });
        let base = std::env::temp_dir().join(format!(
            "clouddownloader-http-inspection-{}",
            uuid::Uuid::new_v4()
        ));
        for route in ["seed.torrent", "download?id=1"] {
            assert!(matches!(
                inspect_http(&base, &format!("http://{address}/{route}"))
                    .await
                    .unwrap(),
                Inspection::Bt { .. }
            ));
        }
        assert!(matches!(
            inspect_http(&base, &format!("http://{address}/file.zip"))
                .await
                .unwrap(),
            Inspection::Direct {
                range_supported: true,
                size: 7,
                ..
            }
        ));
        assert!(
            inspect_http(&base, &format!("http://{address}/huge.torrent"))
                .await
                .unwrap_err()
                .contains("4 MiB")
        );
        server.abort();
        assert!(base
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("clouddownloader-http-inspection-"));
        std::fs::remove_dir_all(base).unwrap();
    }
}
