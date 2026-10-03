//! Opt-in development verification. Does not export browser credentials.
use crate::{drive_commands, model::*, session, AppState};
use md5::Digest;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tauri::Manager;

pub fn start(app: tauri::AppHandle, mode: String) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let result = if mode == "--verify-baidu-probe" {
            async {
            let session = session::connect(&app, Provider::Baidu).await?;
            let listing = session.drive_list("/", 1, "").await?;
            Ok::<_, String>(json!({ "rootCount": listing.files.len(), "hasMore": listing.has_more, "authenticated": !listing.account_id.is_empty() }))
        }.await
        } else if mode == "--verify-baidu-download-probe" {
            download_probe(&app).await
        } else if mode == "--verify-baidu-checks" {
            extra_checks(&app).await
        } else if mode == "--verify-baidu-permanent-share" {
            permanent_share_check(&app).await
        } else {
            smoke(&app, mode == "--verify-baidu-resume").await
        };
        let dir = output_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            dir.join(if mode == "--verify-baidu-download-probe" {
                "baidu-download-probe.json"
            } else if mode == "--verify-baidu-permanent-share" {
                "baidu-permanent-share.json"
            } else if mode == "--verify-baidu-checks" {
                "baidu-checks.json"
            } else if mode == "--verify-baidu-probe" {
                "baidu-probe.json"
            } else if mode == "--verify-baidu-resume" {
                "baidu-resume.json"
            } else {
                "baidu-smoke.json"
            }),
            serde_json::to_vec_pretty(&result).unwrap(),
        );
        if mode != "--verify-baidu-probe" {
            app.exit(if result.is_ok() { 0 } else { 1 });
        }
    });
}

fn output_dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.verification")
}
fn record_step(step: &str) {
    let _ = std::fs::write(output_dir().join("current-step.txt"), step);
}

// Read-only cloud verification; the optional engine check uses an isolated local
// directory and does not insert, edit, or delete user task records.
async fn download_probe(app: &tauri::AppHandle) -> Result<Value, String> {
    let name = std::env::var("CLOUD_VERIFY_DOWNLOAD_NAME")
        .map_err(|_| "Set CLOUD_VERIFY_DOWNLOAD_NAME to the explicitly requested file")?;
    let session = session::connect(app, Provider::Baidu).await?;
    let listing = session.drive_list("/", 1, &name).await?;
    let files: Vec<_> = listing
        .files
        .into_iter()
        .filter(|f| !f.is_dir && f.name == name)
        .collect();
    if files.len() != 1 {
        return Err(format!("Expected one matching file; found {}", files.len()));
    }
    let file = files.into_iter().next().unwrap();
    if file.size < 1024 {
        return Err("Probe requires at least 1024 bytes".into());
    }
    let source = OwnSource {
        account_id: listing.account_id,
        file,
    };
    let descriptor = session.drive_download(&source).await?;
    let mut offsets = vec![0];
    if source.file.size > (1_u64 << 32) + 1024 {
        offsets.push(1_u64 << 32);
    }
    offsets.push(source.file.size - 1024);
    let mut ranges = Vec::new();
    for offset in offsets {
        ranges
            .push(download_range_probe(&session, &descriptor.url, source.file.size, offset).await?);
    }
    let engine = if std::env::var("CLOUD_VERIFY_DOWNLOAD_ENGINE").as_deref() == Ok("1") {
        Some(download_engine_probe(&session, &source).await?)
    } else {
        None
    };
    Ok(json!({"name":source.file.name,"size":source.file.size,"ranges":ranges,"engine":engine}))
}

async fn download_range_probe(
    session: &crate::providers::Session,
    address: &str,
    size: u64,
    offset: u64,
) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|e| e.to_string())?;
    let mut current = url::Url::parse(address).map_err(|_| "Invalid download URL")?;
    let end = offset + 1023;
    let expected = format!("bytes {offset}-{end}/{size}");
    let mut hops = Vec::new();
    for _ in 0..8 {
        // Validate each redirect and select cookies for that destination's domain.
        let descriptor = session.download_descriptor(current.to_string())?;
        let mut request = client
            .get(current.clone())
            .header("User-Agent", descriptor.user_agent)
            .header("Range", format!("bytes={offset}-{end}"))
            .header("Accept-Encoding", "identity");
        for header in &descriptor.headers {
            if let Some((key, value)) = header.split_once(':') {
                request = request.header(key.trim(), value.trim());
            }
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| "Download probe connection failed")?;
        let status = response.status();
        hops.push(json!({"host":current.host_str(),"status":status.as_u16()}));
        if status.is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or("Redirect without location")?;
            current = current.join(location).map_err(|_| "Invalid redirect")?;
            continue;
        }
        let content_range = response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        if status.as_u16() != 206 || content_range != expected {
            return Err(format!(
                "Range probe failed at {offset}: HTTP {}, range {content_range}",
                status.as_u16()
            ));
        }
        let mut bytes = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Download probe body failed")?
        {
            bytes += chunk.len();
            if bytes > 1024 {
                return Err("Range response exceeded requested length".into());
            }
        }
        if bytes != 1024 {
            return Err("Range response was incomplete".into());
        }
        return Ok(json!({"offset":offset,"contentRange":content_range,"bytes":bytes,"hops":hops}));
    }
    Err("Too many download redirects".into())
}

async fn download_engine_probe(
    session: &crate::providers::Session,
    source: &OwnSource,
) -> Result<Value, String> {
    let directory = output_dir().join(format!("download-probe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let mut engine = Some(crate::engine::Engine::start()?);
    let result = async {
        let rpc = engine.as_ref().unwrap().rpc.clone();
        engine_probe_ready(&rpc).await?;
        // Bound network and disk usage. HTTP range probes separately verify >4 GiB
        // and EOF offsets; here the real engine verifies startup and checkpoint reuse.
        let first = session.drive_download(source).await?;
        let gid = rpc
            .add(
                &first.url,
                &directory.to_string_lossy(),
                "sample.bin",
                &first.headers,
                first.user_agent,
            )
            .await?;
        engine_probe_progress(&rpc, &gid, source.file.size, 1024 * 1024).await?;
        rpc.call("aria2.forcePause", vec![json!(gid)]).await?;
        let mut paused = 0;
        for _ in 0..40 {
            let value = rpc
                .call(
                    "aria2.tellStatus",
                    vec![json!(gid), json!(["status", "completedLength"])],
                )
                .await?;
            if value["status"] == "paused" {
                paused = value["completedLength"]
                    .as_str()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if paused == 0 {
            return Err("Engine did not pause".to_string());
        }
        drop(engine.take());
        if !directory.join("sample.bin.aria2").is_file() {
            return Err("Resume checkpoint missing".into());
        }
        engine = Some(crate::engine::Engine::start()?);
        let rpc = engine.as_ref().unwrap().rpc.clone();
        engine_probe_ready(&rpc).await?;
        let fresh = session.drive_download(source).await?;
        let resumed = rpc
            .add(
                &fresh.url,
                &directory.to_string_lossy(),
                "sample.bin",
                &fresh.headers,
                fresh.user_agent,
            )
            .await?;
        let progress =
            engine_probe_progress(&rpc, &resumed, source.file.size, paused + 1024 * 1024).await?;
        rpc.call("aria2.forcePause", vec![json!(resumed)]).await?;
        Ok::<_, String>(json!({"totalLength":source.file.size,"pausedBytes":paused,
            "resumedBytes":progress,"refreshedLink":true,"engineRestarted":true,"temporaryTaskOnly":true}))
    }
    .await;
    drop(engine);
    // This fresh UUID directory belongs only to this opt-in probe.
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    let root = output_dir().canonicalize().map_err(|e| e.to_string())?;
    if directory.parent() != Some(root.as_path()) {
        return Err("Probe cleanup target is outside the verification directory".into());
    }
    std::fs::remove_dir_all(&directory).map_err(|e| format!("Probe cleanup failed: {e}"))?;
    result
}

async fn engine_probe_ready(rpc: &crate::engine::Rpc) -> Result<(), String> {
    for _ in 0..50 {
        if rpc.call("aria2.getVersion", vec![]).await.is_ok() {
            rpc.call(
                "aria2.changeGlobalOption",
                vec![json!({
                    "split":"1","max-connection-per-server":"1","max-download-limit":"1M"
                })],
            )
            .await?;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("Probe engine did not start".into())
}

async fn engine_probe_progress(
    rpc: &crate::engine::Rpc,
    gid: &str,
    size: u64,
    minimum: u64,
) -> Result<u64, String> {
    for _ in 0..240 {
        let value = rpc
            .call(
                "aria2.tellStatus",
                vec![
                    json!(gid),
                    json!([
                        "status",
                        "totalLength",
                        "completedLength",
                        "errorCode",
                        "errorMessage"
                    ]),
                ],
            )
            .await?;
        if value["status"] == "error" {
            return Err(crate::download_errors::message(
                value["errorCode"].as_str().unwrap_or_default(),
                value["errorMessage"].as_str().unwrap_or_default(),
                false,
            ));
        }
        let total = value["totalLength"]
            .as_str()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let completed = value["completedLength"]
            .as_str()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        if total != 0 && total != size {
            return Err("Engine total size differs".into());
        }
        if completed >= minimum {
            return Ok(completed);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("Engine transfer timed out".into())
}

async fn permanent_share_check(app: &tauri::AppHandle) -> Result<Value, String> {
    // Reuse only the synthetic file from the opt-in upload verification.
    let fixture: Value = serde_json::from_slice(
        &std::fs::read(output_dir().join("baidu-run.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let upload = app
        .state::<AppState>()
        .store
        .lock()
        .unwrap()
        .uploads()?
        .into_iter()
        .find(|t| Some(t.id.as_str()) == fixture["id"].as_str())
        .ok_or("generated upload fixture missing")?;
    let file = upload
        .remote_file
        .ok_or("generated upload fixture incomplete")?;
    let window = app
        .get_webview_window("main")
        .ok_or("main window missing")?;
    let session = session::connect(app, Provider::Baidu).await?;
    let share = drive_commands::drive_share(
        window.clone(),
        app.clone(),
        app.state(),
        upload.account_id,
        vec![file.clone()],
        0,
        "cd26".into(),
    )
    .await?;
    std::fs::write(
        output_dir().join("permanent-fixture-share-id.txt"),
        &share.id,
    )
    .map_err(|e| e.to_string())?;
    let verification = async {
        let link = crate::providers::parse_share(&share.url, &share.password)?;
        let listing = session.list(&link, "0").await?;
        if !listing
            .files
            .iter()
            .any(|f| f.id == file.id && f.size == file.size)
        {
            return Err("permanent share contents differ from fixture".to_string());
        }
        Ok::<_, String>(
            json!({"shareCreated": true, "shareReadable": true, "requestedPeriod": share.period}),
        )
    }
    .await;
    // Cancel our test link even when the verification fails; never touch user shares.
    drive_commands::drive_cancel_share(window, app.clone(), app.state(), share.id.clone()).await?;
    let mut result = verification?;
    let saved = app
        .state::<AppState>()
        .store
        .lock()
        .unwrap()
        .shares()?
        .into_iter()
        .find(|s| s.id == share.id)
        .ok_or("saved fixture share missing")?;
    if saved.period != 0 || !saved.cancelled {
        return Err("permanent share persistence or cancellation mismatch".into());
    }
    result["shareCancelled"] = json!(true);
    result["persistedPeriod"] = json!(saved.period);
    Ok(result)
}

async fn extra_checks(app: &tauri::AppHandle) -> Result<Value, String> {
    let record: Value = serde_json::from_slice(
        &std::fs::read(output_dir().join("baidu-run.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let window = app
        .get_webview_window("main")
        .ok_or("main window missing")?;
    let session = session::connect(app, Provider::Baidu).await?;
    let identity = session.baidu_identity(None).await?;
    let original = app
        .state::<AppState>()
        .store
        .lock()
        .unwrap()
        .uploads()?
        .into_iter()
        .find(|t| t.id == record["id"].as_str().unwrap_or_default())
        .ok_or("fixture task missing")?;
    let remote = original.remote_file.clone().ok_or("fixture incomplete")?;
    record_step("verify account guard and search");
    if session
        .baidu_identity(Some("wrong-account-for-test"))
        .await
        .is_ok()
    {
        return Err("account mismatch was accepted".into());
    }
    let search = session
        .drive_list(record["remoteDir"].as_str().unwrap(), 1, &remote.name)
        .await?;
    if !search.files.iter().any(|f| f.id == remote.id) {
        return Err("search did not find uploaded fixture".into());
    }
    record_step("download through the normal aria2 queue");
    let previous = app
        .state::<AppState>()
        .store
        .lock()
        .unwrap()
        .tasks()?
        .into_iter()
        .find(|t| {
            t.own
                .as_ref()
                .is_some_and(|s| s.file.id == remote.id && s.account_id == identity.account_id)
        });
    let tasks = if let Some(task) = previous {
        vec![task]
    } else {
        drive_commands::drive_enqueue(
            window.clone(),
            app.clone(),
            app.state(),
            identity.account_id.clone(),
            vec![remote.clone()],
        )
        .await?
    };
    let id = &tasks[0].id;
    let mut downloaded = None;
    for _ in 0..300 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let task = app
            .state::<AppState>()
            .store
            .lock()
            .unwrap()
            .tasks()?
            .into_iter()
            .find(|t| &t.id == id)
            .ok_or("download missing")?;
        if task.status == "error" {
            return Err(task.error);
        }
        if task.status == "complete" {
            downloaded = Some(PathBuf::from(task.directory).join(task.name));
            break;
        }
    }
    let downloaded = downloaded.ok_or("aria2 task timeout")?;
    let original_bytes = std::fs::read(&original.local_path).map_err(|e| e.to_string())?;
    if std::fs::read(downloaded).map_err(|e| e.to_string())? != original_bytes {
        return Err("aria2 bytes differ".into());
    }
    record_step("verify same-name protection without overwriting fixture");
    let collision = drive_commands::upload_enqueue(
        window.clone(),
        app.clone(),
        app.state(),
        identity.account_id.clone(),
        record["remoteDir"].as_str().unwrap().into(),
        vec![original.local_path.clone()],
        4,
    )
    .await;
    let collision_rejected = collision.is_err();
    if !collision_rejected {
        return Err("same-name file was accepted into upload queue".into());
    }
    let fresh = session
        .drive_meta(std::slice::from_ref(&remote.path))
        .await?
        .into_iter()
        .next()
        .ok_or("fixture vanished")?;
    if fresh.id != remote.id || fresh.size != remote.size {
        return Err("same-name precreate changed existing file".into());
    }
    record_step("verify server-side final commit conflict protection");
    let guard_path = format!(
        "{}/同名保护-{}.txt",
        record["remoteDir"].as_str().unwrap(),
        &uuid::Uuid::new_v4().simple().to_string()[..6]
    );
    let guard_bytes = b"collision-original".to_vec();
    let guard_hashes = vec![format!("{:x}", md5::Md5::digest(&guard_bytes))];
    let pre = session
        .upload_precreate(
            &identity,
            &guard_path,
            guard_bytes.len() as u64,
            &guard_hashes,
            "",
        )
        .await?;
    if !pre.complete {
        session
            .upload_part(
                &identity.token,
                &guard_path,
                &pre.upload_id,
                0,
                guard_bytes.clone(),
            )
            .await?;
        session
            .upload_finish(
                &identity,
                &guard_path,
                guard_bytes.len() as u64,
                &guard_hashes,
                &pre.upload_id,
            )
            .await?;
    }
    let replacement = b"replacement".to_vec();
    let hashes = vec![format!("{:x}", md5::Md5::digest(&replacement))];
    let pre = session
        .upload_precreate(
            &identity,
            &guard_path,
            replacement.len() as u64,
            &hashes,
            "",
        )
        .await;
    if let Ok(pre) = pre {
        if pre.complete {
            return Err("server precreate replaced collision fixture".into());
        }
        session
            .upload_part(
                &identity.token,
                &guard_path,
                &pre.upload_id,
                0,
                replacement.clone(),
            )
            .await?;
        if session
            .upload_finish(
                &identity,
                &guard_path,
                replacement.len() as u64,
                &hashes,
                &pre.upload_id,
            )
            .await
            .is_ok()
        {
            return Err("server commit did not reject same-name fixture".into());
        }
    }
    let original_guard = session.drive_meta(&[guard_path]).await?;
    if original_guard[0].size != guard_bytes.len() as u64 {
        return Err("collision fixture was overwritten".into());
    }
    record_step("upload empty and small files concurrently");
    let suffix = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let empty = output_dir().join(format!("空文件-{suffix}.txt"));
    let small = output_dir().join(format!("中文内容-{suffix}.txt"));
    std::fs::write(&empty, []).map_err(|e| e.to_string())?;
    std::fs::write(&small, "CloudDownloader 上传验证\n").map_err(|e| e.to_string())?;
    let tasks = drive_commands::upload_enqueue(
        window,
        app.clone(),
        app.state(),
        identity.account_id,
        record["remoteDir"].as_str().unwrap().into(),
        vec![
            empty.to_string_lossy().into(),
            small.to_string_lossy().into(),
        ],
        2,
    )
    .await?;
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let records = app.state::<AppState>().store.lock().unwrap().uploads()?;
        let ours = records
            .iter()
            .filter(|r| tasks.iter().any(|t| r.id == t.id))
            .collect::<Vec<_>>();
        if let Some(failed) = ours.iter().find(|t| t.status == "error") {
            return Err(format!("{}: {}", failed.name, failed.error));
        }
        if ours.len() == 2 && ours.iter().all(|t| t.status == "complete") {
            record_step("extra checks verified");
            return Ok(
                json!({"aria2QueueBytesIdentical": true, "accountGuard": true, "search": true, "sameNameRejected": collision_rejected, "emptyUpload": true, "smallUpload": true, "resumeConfirmedRequests": original.confirmed_requests}),
            );
        }
    }
    Err("empty/small uploads timed out".into())
}

async fn smoke(app: &tauri::AppHandle, resume: bool) -> Result<Value, String> {
    let dir = output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let window = app
        .get_webview_window("main")
        .ok_or("main window missing")?;
    let session = session::connect(app, Provider::Baidu).await?;
    let identity = session.baidu_identity(None).await?;
    if !resume {
        record_step("create generated fixture");
        let suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let remote_dir = format!("/CloudDownloader-验证-{suffix}");
        let path = dir.join(format!("分片验证-{suffix}.bin"));
        let mut bytes = vec![0u8; 25 * 1024 * 1024 + 133];
        let seed = uuid::Uuid::new_v4();
        let mut x = u64::from_le_bytes(seed.as_bytes()[..8].try_into().unwrap());
        for byte in &mut bytes {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *byte = x as u8;
        }
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        drive_commands::drive_mkdir(
            window.clone(),
            app.clone(),
            identity.account_id.clone(),
            "/".into(),
            remote_dir.trim_start_matches('/').into(),
        )
        .await?;
        record_step("enqueue multipart upload");
        let tasks = drive_commands::upload_enqueue(
            window.clone(),
            app.clone(),
            app.state(),
            identity.account_id,
            remote_dir.clone(),
            vec![path.to_string_lossy().into()],
            4,
        )
        .await?;
        let id = tasks[0].id.clone();
        std::fs::write(
            dir.join("baidu-run.json"),
            json!({"id": id, "remoteDir": remote_dir, "localPath": path}).to_string(),
        )
        .map_err(|e| e.to_string())?;
        for _ in 0..1200 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let task = app
                .state::<AppState>()
                .store
                .lock()
                .unwrap()
                .uploads()?
                .into_iter()
                .find(|t| t.id == id)
                .ok_or("upload missing")?;
            if task.status == "error" {
                return Err(task.error);
            }
            if task.status == "complete" {
                return Err("Fixture finished before pause; retry with larger fixture".into());
            }
            if task.status == "active"
                && task.done.iter().any(|d| *d)
                && task.done.iter().any(|d| !*d)
            {
                drive_commands::upload_action(
                    window,
                    app.clone(),
                    app.state(),
                    id.clone(),
                    "pause".into(),
                )
                .await?;
                for _ in 0..40 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let task = app
                        .state::<AppState>()
                        .store
                        .lock()
                        .unwrap()
                        .uploads()?
                        .into_iter()
                        .find(|t| t.id == id)
                        .unwrap();
                    if task.status == "paused" {
                        record_step("paused; restart with --verify-baidu-resume");
                        return Ok(
                            json!({"paused": true, "completedParts": task.done.iter().filter(|v| **v).count(), "totalParts": task.hashes.len(), "completedBytes": task.completed, "size": task.size}),
                        );
                    }
                }
                return Err("pause timed out".into());
            }
        }
        return Err("upload timed out".into());
    }
    let record: Value = serde_json::from_slice(
        &std::fs::read(dir.join("baidu-run.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let id = record["id"].as_str().ok_or("missing id")?.to_string();
    record_step("resume persisted upload after application restart");
    drive_commands::upload_action(
        window.clone(),
        app.clone(),
        app.state(),
        id.clone(),
        "resume".into(),
    )
    .await?;
    let mut remote = None;
    for _ in 0..900 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let task = app
            .state::<AppState>()
            .store
            .lock()
            .unwrap()
            .uploads()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or("upload missing")?;
        if task.status == "error" {
            return Err(task.error);
        }
        if task.status == "complete" {
            remote = task.remote_file;
            break;
        }
    }
    let remote = remote.ok_or("resume did not complete")?;
    record_step("download owned file and compare bytes");
    let source = OwnSource {
        account_id: identity.account_id.clone(),
        file: remote.clone(),
    };
    let download = session.drive_download(&source).await?;
    let mut request = session
        .transport
        .client
        .get(&download.url)
        .header("User-Agent", download.user_agent)
        .header("Referer", "https://pan.baidu.com/")
        .timeout(Duration::from_secs(300));
    for header in &download.headers {
        if let Some((key, value)) = header.split_once(':') {
            request = request.header(key, value.trim());
        }
    }
    let response = request
        .send()
        .await
        .map_err(|_| "fixture download request failed")?;
    if !response.status().is_success() {
        return Err(format!("fixture download HTTP {}", response.status()));
    }
    let received = response
        .bytes()
        .await
        .map_err(|_| "fixture download body failed")?;
    let local = std::fs::read(record["localPath"].as_str().ok_or("missing fixture path")?)
        .map_err(|e| e.to_string())?;
    if received.as_ref() != local.as_slice() {
        return Err("downloaded bytes differ from generated fixture".into());
    }
    record_step("create temporary share of generated fixture");
    let share = drive_commands::drive_share(
        window.clone(),
        app.clone(),
        app.state(),
        identity.account_id,
        vec![remote],
        1,
        "cd26".into(),
    )
    .await?;
    // Persist the generated fixture share for cancellation even if the next check fails.
    std::fs::write(dir.join("fixture-share-id.txt"), &share.id).map_err(|e| e.to_string())?;
    let link = crate::providers::parse_share(&share.url, &share.password)?;
    let listing_result = session.list(&link, "0").await;
    record_step("cancel temporary fixture share");
    drive_commands::drive_cancel_share(window, app.clone(), app.state(), share.id).await?;
    let listing = listing_result?;
    if listing.files.len() != 1 || listing.files[0].size != received.len() as u64 {
        return Err("shared fixture listing mismatch".into());
    }
    record_step("verified");
    Ok(
        json!({"resumed": true, "bytesIdentical": true, "size": received.len(), "shareCreated": true, "shareReadable": true, "shareCancelled": true, "fixtureFolder": record["remoteDir"]}),
    )
}
