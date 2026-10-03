use crate::{
    model::*,
    providers::baidu_drive::remote_path,
    session, start_task, trusted,
    upload::{self, UploadRecord, UploadView},
    AppState,
};
use std::{collections::HashSet, path::PathBuf};
use tauri::{State, WebviewWindow};

#[tauri::command]
pub async fn drive_list(
    window: WebviewWindow,
    app: tauri::AppHandle,
    path: String,
    page: u32,
    keyword: String,
) -> Result<DriveListing, String> {
    trusted(&window)?;
    session::connect(&app, Provider::Baidu)
        .await?
        .drive_list(&path, page, &keyword)
        .await
}

#[tauri::command]
pub async fn drive_mkdir(
    window: WebviewWindow,
    app: tauri::AppHandle,
    account_id: String,
    parent: String,
    name: String,
) -> Result<DriveFile, String> {
    trusted(&window)?;
    let path = child_path(&parent, &name)?;
    let session = session::connect(&app, Provider::Baidu).await?;
    let identity = session.baidu_identity(Some(&account_id)).await?;
    session.drive_mkdir(&identity, &path).await
}

fn child_path(parent: &str, name: &str) -> Result<String, String> {
    remote_path(parent)?;
    if name.trim().is_empty()
        || name != name.trim()
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
        || matches!(name, "." | "..")
    {
        return Err("文件夹或文件名无效".into());
    }
    let path = format!("{}/{}", parent.trim_end_matches('/'), name);
    remote_path(&path)?;
    Ok(path)
}

#[tauri::command]
pub async fn drive_enqueue(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    files: Vec<DriveFile>,
) -> Result<Vec<Task>, String> {
    trusted(&window)?;
    let _guard = state.resolving.lock().await;
    let session = session::connect(&app, Provider::Baidu).await?;
    session.baidu_identity(Some(&account_id)).await?;
    let fresh = session
        .drive_meta(&files.iter().map(|f| f.path.clone()).collect::<Vec<_>>())
        .await?;
    if fresh.len() != files.len()
        || files
            .iter()
            .any(|f| !fresh.iter().any(|g| f.id == g.id && f.path == g.path))
    {
        return Err("文件列表已变化，请刷新后重新选择".into());
    }
    let directory = state
        .store
        .lock()
        .unwrap()
        .setting("download_dir")
        .ok_or("请先选择下载目录")?;
    let mut pending = fresh
        .into_iter()
        .map(|f| (f, PathBuf::from(&directory)))
        .collect::<Vec<_>>();
    let mut visited = HashSet::new();
    let mut downloads = Vec::new();
    while let Some((file, dir)) = pending.pop() {
        if !visited.insert(file.id.clone()) {
            continue;
        }
        if visited.len() > 10000 {
            return Err("所选文件超过 10000 项，请分批下载".into());
        }
        if file.is_dir {
            for page in 1..=10000 {
                let listing = session.drive_list(&file.path, page, "").await?;
                if listing.account_id != account_id {
                    return Err("百度账号已切换，请重新选择文件".into());
                }
                let subdir = dir.join(safe_name(&file.name));
                pending.extend(listing.files.into_iter().map(|f| (f, subdir.clone())));
                if pending.len() + visited.len() > 10000 {
                    return Err("所选文件过多，请分批下载".into());
                }
                if !listing.has_more {
                    break;
                }
            }
        } else {
            downloads.push((file, dir));
        }
    }
    let mut created = Vec::new();
    for (file, dir) in downloads {
        let existing = state.store.lock().unwrap().tasks()?;
        if existing.iter().any(|t| {
            t.own
                .as_ref()
                .is_some_and(|s| s.account_id == account_id && s.file.id == file.id)
                && t.directory == dir.to_string_lossy()
                && !matches!(t.status.as_str(), "error" | "cancelled")
        }) {
            continue;
        }
        let base = safe_name(&file.name);
        let mut name = base.clone();
        let mut n = 1;
        while dir.join(&name).exists()
            || existing.iter().any(|t| {
                t.directory.eq_ignore_ascii_case(&dir.to_string_lossy())
                    && t.name.eq_ignore_ascii_case(&name)
            })
        {
            let p = PathBuf::from(&base);
            name = format!(
                "{} ({n}){}",
                p.file_stem().unwrap_or_default().to_string_lossy(),
                p.extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default()
            );
            n += 1;
        }
        let task = Task {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            size: file.size,
            completed: 0,
            speed: 0,
            status: "resolving".into(),
            error: String::new(),
            directory: dir.to_string_lossy().into(),
            created_at: now(),
            gid: None,
            share: ShareLink {
                provider: Provider::Baidu,
                id: String::new(),
                passcode: String::new(),
            },
            file: ShareFile {
                id: file.id.clone(),
                name: file.name.clone(),
                size: file.size,
                is_dir: false,
                parent_id: String::new(),
                token: String::new(),
            },
            own: Some(OwnSource {
                account_id: account_id.clone(),
                file,
            }),
            direct: None,
            connections: 0,
        };
        created.push(start_task(&app, &state, task, None).await?);
    }
    if created.is_empty() {
        return Err("所选文件已在任务列表中，或文件夹为空".into());
    }
    Ok(created)
}

#[tauri::command]
pub async fn drive_share(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    files: Vec<DriveFile>,
    period: u32,
    password: String,
) -> Result<CreatedShare, String> {
    trusted(&window)?;
    let share = session::connect(&app, Provider::Baidu)
        .await?
        .drive_share(&account_id, &files, period, &password)
        .await?;
    state.store.lock().unwrap().save_share(&share)?;
    Ok(share)
}

#[tauri::command]
pub async fn drive_cancel_share(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    trusted(&window)?;
    let mut share = state
        .store
        .lock()
        .unwrap()
        .shares()?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or("分享记录不存在")?;
    if !share.cancelled {
        session::connect(&app, Provider::Baidu)
            .await?
            .drive_cancel_share(&share)
            .await?;
        share.cancelled = true;
        state.store.lock().unwrap().save_share(&share)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn upload_enqueue(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    directory: String,
    paths: Vec<String>,
    concurrency: usize,
) -> Result<Vec<UploadView>, String> {
    trusted(&window)?;
    if paths.is_empty() || paths.len() > 100 || !matches!(concurrency, 1 | 2 | 4 | 8) {
        return Err("请选择 1 至 100 个文件，并使用 1、2、4 或 8 路并发".into());
    }
    remote_path(&directory)?;
    let _guard = state.resolving.lock().await;
    let session = session::connect(&app, Provider::Baidu).await?;
    session.baidu_identity(Some(&account_id)).await?;
    if directory != "/" {
        let meta = session.drive_meta(std::slice::from_ref(&directory)).await?;
        if !meta.iter().any(|f| f.is_dir && f.path == directory) {
            return Err("上传目标目录不存在".into());
        }
    }
    let existing = state.store.lock().unwrap().uploads()?;
    let mut targets = HashSet::new();
    let mut records = Vec::new();
    for path in paths {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("本地文件路径必须为绝对路径".into());
        }
        let (size, modified_ns) = upload::fingerprint(&path)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("本地文件名无效")?
            .to_string();
        let remote_path = child_path(&directory, &name)?;
        if !targets.insert(remote_path.clone())
            || existing.iter().any(|t| {
                t.account_id == account_id
                    && t.remote_path == remote_path
                    && !matches!(t.status.as_str(), "complete" | "error")
            })
        {
            return Err(format!("{name} 已在上传队列中"));
        }
        records.push(UploadRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            local_path: path.to_string_lossy().into(),
            remote_path,
            account_id: account_id.clone(),
            size,
            modified_ns,
            concurrency,
            status: "waiting".into(),
            error: String::new(),
            completed: 0,
            speed: 0,
            created_at: now(),
            hashes: Vec::new(),
            done: Vec::new(),
            upload_id: String::new(),
            remote_file: None,
            confirmed_requests: 0,
        });
    }
    // Check the chosen names once before enqueueing the batch; the worker checks
    // again before sending data, and create uses the server's no-overwrite mode.
    for page in 1..=200 {
        let listing = session.drive_list(&directory, page, "").await?;
        if let Some(existing_file) = listing.files.iter().find(|f| targets.contains(&f.path)) {
            return Err(format!(
                "目标位置已有同名文件：{}。请修改本地文件名或选择其他目录。",
                existing_file.name
            ));
        }
        if !listing.has_more {
            break;
        }
        if page == 200 {
            return Err("目标目录超过 20000 项，请选择较小的目录上传".into());
        }
    }
    for task in &records {
        state.store.lock().unwrap().save_upload(task)?;
        state.uploads.start(app.clone(), task.clone())?;
    }
    Ok(records.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn upload_action(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    action: String,
) -> Result<(), String> {
    trusted(&window)?;
    let _guard = state.resolving.lock().await;
    let mut task = state
        .store
        .lock()
        .unwrap()
        .uploads()?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or("上传任务不存在")?;
    match action.as_str() {
        "pause" => {
            if task.status == "merging" {
                return Err("正在提交云端文件，请等待提交完成".into());
            }
            state.uploads.pause(&id);
        }
        "resume" => {
            if !matches!(task.status.as_str(), "paused" | "error") {
                return Err("任务当前不能恢复".into());
            }
            if state.uploads.is_running(&id) {
                return Err("任务正在暂停，请稍后重试".into());
            }
            task.status = "waiting".into();
            task.error.clear();
            state.store.lock().unwrap().save_upload(&task)?;
            state.uploads.start(app, task)?;
        }
        "remove" => {
            if state.uploads.is_running(&id) {
                return Err("请先暂停上传，再移除任务".into());
            }
            state.store.lock().unwrap().remove_upload(&id)?;
        }
        _ => return Err("未知上传操作".into()),
    }
    Ok(())
}
