//! Opt-in live downloads through the normal commands, using a fresh task database.
use crate::{cloud_commands, model::*, providers, session, AppState};
use md5::Digest;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::Read,
    path::PathBuf,
    sync::OnceLock,
    time::Duration,
};
use tauri::Manager;

pub(super) fn directory() -> &'static PathBuf {
    static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
    DIRECTORY
        .get_or_init(|| super::output_dir().join(format!("downloads-{}", uuid::Uuid::new_v4())))
}

pub(super) async fn run(app: &tauri::AppHandle) -> Result<Value, String> {
    let selected = std::env::var("CLOUD_VERIFY_CASES").unwrap_or_default();
    let selected = selected
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    if selected.iter().any(|case| {
        ![
            "pan123-personal",
            "pan123-share",
            "quark-personal",
            "quark-share",
        ]
        .contains(case)
    }) {
        return Err("未知的下载验证项目".into());
    }
    let state = app.state::<AppState>();
    let output = directory().join("files");
    std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    state
        .store
        .lock()
        .unwrap()
        .set_setting("download_dir", &output.to_string_lossy())?;
    for provider in [Provider::Pan123, Provider::Quark] {
        session::window(app, provider, false)?;
    }
    tokio::time::sleep(Duration::from_secs(6)).await;
    let mut results = serde_json::Map::new();
    for provider in [Provider::Pan123, Provider::Quark] {
        for share in [false, true] {
            let label = format!(
                "{}-{}",
                provider.key(),
                if share { "share" } else { "personal" }
            );
            if !selected.is_empty() && !selected.contains(&label.as_str()) {
                continue;
            }
            let result = if share {
                shared(app, provider).await
            } else {
                personal(app, provider).await
            };
            results.insert(
                label,
                serde_json::to_value(result).map_err(|e| e.to_string())?,
            );
            std::fs::write(
                directory().join("progress.json"),
                serde_json::to_vec_pretty(&results).unwrap(),
            )
            .map_err(|e| e.to_string())?;
            std::fs::write(
                super::output_dir().join("download-optimizations-progress.json"),
                serde_json::to_vec_pretty(&results).unwrap(),
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(json!({"directory":directory(),"checks":results}))
}

async fn personal(app: &tauri::AppHandle, provider: Provider) -> Result<Value, String> {
    let session = ready_session(app, provider).await?;
    let account = session.cloud_identity().await?.0;
    let requested_name = std::env::var(format!(
        "CLOUD_VERIFY_{}_NAME",
        provider.key().to_uppercase()
    ))
    .ok();
    let mut files = find_personal_files(&session, requested_name.as_deref()).await?;
    files.sort_by_key(|file| file.size);
    files.truncate(2);
    if files.is_empty() {
        return Err("没有找到符合条件的个人网盘测试文件".into());
    }
    let hashes = files
        .iter()
        .map(|file| (file.id.clone(), file.md5.clone()))
        .collect();
    let main = app.get_webview_window("main").ok_or("主窗口缺失")?;
    let started = std::time::Instant::now();
    let tasks = cloud_commands::cloud_enqueue(
        main,
        app.clone(),
        app.state::<AppState>(),
        provider,
        account,
        files,
    )
    .await?;
    let preparation = started.elapsed().as_millis();
    let checked = wait_and_check(app, tasks, hashes).await?;
    Ok(json!({"preparationMs":preparation,"files":checked}))
}

async fn find_personal_files(
    session: &providers::Session,
    name: Option<&str>,
) -> Result<Vec<DriveFile>, String> {
    let normalize = |s: &str| s.replace([' ', '_'], "").to_lowercase();
    let expected = name.map(normalize);
    let mut pending = VecDeque::from(["/".to_string()]);
    for _ in 0..50 {
        let Some(path) = pending.pop_front() else {
            break;
        };
        let mut cursor = None;
        let mut files = Vec::new();
        for page in 1..=200 {
            let batch = session.cloud_page(&path, page, cursor.as_deref()).await?;
            for entry in batch.entries {
                let file = entry.file;
                if file.is_dir {
                    if name.is_some() {
                        pending.push_back(file.path);
                    }
                } else if file.size > 0
                    && file.size <= 64 * 1024 * 1024
                    && expected
                        .as_ref()
                        .is_none_or(|expected| normalize(&file.name) == *expected)
                {
                    files.push(file);
                }
            }
            cursor = batch.next;
            if cursor.is_none() {
                break;
            }
        }
        if !files.is_empty() {
            return Ok(files);
        }
    }
    Ok(Vec::new())
}

async fn shared(app: &tauri::AppHandle, provider: Provider) -> Result<Value, String> {
    let address = std::env::var(format!(
        "CLOUD_VERIFY_{}_SHARE",
        provider.key().to_uppercase()
    ))
    .map_err(|_| "未提供测试分享链接")?;
    let share = providers::parse_share(&address, "")?;
    if share.provider != provider {
        return Err("测试分享来源不匹配".into());
    }
    let session = ready_session(app, provider).await?;
    let requested_name = std::env::var(format!(
        "CLOUD_VERIFY_{}_SHARE_NAME",
        provider.key().to_uppercase()
    ))
    .ok();
    let listing = find_share_files(&session, &share, requested_name.as_deref()).await?;
    let mut files = listing
        .files
        .iter()
        .filter(|file| !file.is_dir && file.size > 0 && file.size <= 1024 * 1024 * 1024)
        .filter(|file| {
            requested_name
                .as_ref()
                .is_none_or(|name| file.name == *name)
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|file| file.size);
    files.truncate(2);
    if files.is_empty() {
        return Err("分享根目录没有符合条件的文件".into());
    }
    let mut hashes = HashMap::new();
    if provider == Provider::Pan123 {
        for file in &files {
            let metadata: Value =
                serde_json::from_str(&file.token).map_err(|_| "文件元数据无效")?;
            hashes.insert(file.id.clone(), providers::text(&metadata["Etag"]));
        }
    } else {
        let response = session
            .json(
                session
                    .transport
                    .client
                    .get("https://drive-pc.quark.cn/1/clouddrive/share/sharepage/detail")
                    .query(&[
                        ("pr", "ucpro"),
                        ("fr", "pc"),
                        ("pwd_id", &share.id),
                        ("stoken", &listing.stoken),
                        ("pdir_fid", &listing.parent_id),
                        ("_page", "1"),
                        ("_size", "100"),
                    ])
                    .header("Referer", "https://pan.quark.cn/"),
            )
            .await?;
        if let Some(rows) = response["data"]["list"].as_array() {
            for row in rows {
                hashes.insert(providers::text(&row["fid"]), providers::text(&row["md5"]));
            }
        }
    }
    let ids = files.iter().map(|file| file.id.clone()).collect();
    let main = app.get_webview_window("main").ok_or("主窗口缺失")?;
    let started = std::time::Instant::now();
    let tasks = crate::enqueue(
        main,
        app.clone(),
        app.state::<AppState>(),
        share,
        listing.parent_id,
        ids,
    )
    .await?;
    let preparation = started.elapsed().as_millis();
    let checked = wait_and_check(app, tasks, hashes).await?;
    Ok(json!({"preparationMs":preparation,"files":checked}))
}

async fn ready_session(
    app: &tauri::AppHandle,
    provider: Provider,
) -> Result<providers::Session, String> {
    for attempt in 0..100 {
        let session = session::connect(app, provider).await?;
        if provider != Provider::Pan123 || !session.authorization.is_empty() {
            if attempt >= 5 {
                let _ = session::window(app, provider, false)?.hide();
            }
            return Ok(session);
        }
        if attempt == 5 {
            session::window(app, provider, true)?;
            std::fs::write(
                super::output_dir().join("download-optimizations-status.json"),
                serde_json::to_vec(&json!({"needsLogin":provider.key()})).unwrap(),
            )
            .map_err(|e| e.to_string())?;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    Err("等待官方窗口登录超时".into())
}

async fn find_share_files(
    session: &providers::Session,
    share: &ShareLink,
    name: Option<&str>,
) -> Result<ShareListing, String> {
    let mut pending = VecDeque::from(["0".to_string()]);
    for _ in 0..50 {
        let Some(parent) = pending.pop_front() else {
            break;
        };
        let listing = session.list(share, &parent).await?;
        if listing.files.iter().any(|file| {
            !file.is_dir
                && file.size > 0
                && file.size <= 1024 * 1024 * 1024
                && name.is_none_or(|name| file.name == name)
        }) {
            return Ok(listing);
        }
        pending.extend(
            listing
                .files
                .iter()
                .filter(|file| file.is_dir)
                .map(|file| file.id.clone()),
        );
    }
    Err("在分享目录中没有找到符合条件的测试文件".into())
}

async fn wait_and_check(
    app: &tauri::AppHandle,
    tasks: Vec<Task>,
    hashes: HashMap<String, String>,
) -> Result<Vec<Value>, String> {
    let state = app.state::<AppState>();
    for _ in 0..900 {
        let current = tasks
            .iter()
            .map(|task| {
                state
                    .store
                    .lock()
                    .unwrap()
                    .task(&task.id)
                    .and_then(|task| task.ok_or("测试任务缺失".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(task) = current.iter().find(|task| task.status == "error") {
            return Err(task.error.clone());
        }
        if current.iter().all(|task| task.status == "complete") {
            let mut checked = Vec::new();
            for task in current {
                let path = PathBuf::from(&task.directory).join(&task.name);
                let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
                if size != task.size || size != task.file.size {
                    return Err("完整下载文件大小不匹配".into());
                }
                let digest = tauri::async_runtime::spawn_blocking(move || {
                    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                    let mut digest = md5::Md5::new();
                    let mut buffer = vec![0; 256 * 1024];
                    loop {
                        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
                        if n == 0 {
                            break;
                        }
                        digest.update(&buffer[..n]);
                    }
                    Ok::<_, String>(format!("{:x}", digest.finalize()))
                })
                .await
                .map_err(|e| e.to_string())??;
                let expected = hashes.get(&task.file.id).filter(|hash| !hash.is_empty());
                if expected.is_some_and(|hash| !hash.eq_ignore_ascii_case(&digest)) {
                    return Err("完整下载文件 MD5 不匹配".into());
                }
                checked.push(
                    json!({"bytes":size,"sizeMatched":true,"md5Matched":expected.map(|_|true)}),
                );
            }
            return Ok(checked);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err("下载验证超时".into())
}
