use crate::{model::*, providers::cloud::Listing, session, start_task, trusted, AppState};
use std::{collections::HashSet, path::PathBuf};
use tauri::{State, WebviewWindow};

#[tauri::command]
pub async fn cloud_list(
    window: WebviewWindow,
    app: tauri::AppHandle,
    provider: Provider,
    path: String,
    page: u32,
    cursor: Option<String>,
) -> Result<Listing, String> {
    trusted(&window)?;
    let session = session::connect(&app, provider).await?;
    let account_id = session.cloud_identity().await?.0;
    let batch = session.cloud_page(&path, page, cursor.as_deref()).await?;
    Ok(Listing {
        account_id,
        path,
        files: batch.entries.into_iter().map(|entry| entry.file).collect(),
        next_cursor: batch.next,
    })
}

#[tauri::command]
pub async fn cloud_enqueue(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    provider: Provider,
    account_id: String,
    files: Vec<DriveFile>,
) -> Result<Vec<Task>, String> {
    trusted(&window)?;
    if files.is_empty() || files.len() > 100 {
        return Err("一次请选择 1 至 100 项下载".into());
    }
    let _guard = state.resolving.lock().await;
    let session = session::connect(&app, provider).await?;
    if session.cloud_identity().await?.0 != account_id {
        return Err("网盘账号已切换，请刷新后重新选择".into());
    }
    let directory = PathBuf::from(
        state
            .store
            .lock()
            .unwrap()
            .setting("download_dir")
            .ok_or("请先选择下载目录")?,
    );
    let mut pending = Vec::new();
    for selected in files {
        let fresh = session.cloud_find(&selected).await?.file;
        if fresh.size != selected.size
            || fresh.is_dir != selected.is_dir
            || fresh.name != selected.name
        {
            return Err("文件列表已变化，请刷新后重新选择".into());
        }
        pending.push((fresh, directory.clone()));
    }
    let mut visited = HashSet::new();
    let mut downloads = Vec::new();
    while let Some((file, directory)) = pending.pop() {
        if !visited.insert(file.id.clone()) {
            continue;
        }
        if visited.len() + pending.len() > 10000 {
            return Err("所选文件超过 10000 项，请分批下载".into());
        }
        if file.is_dir {
            let mut cursor = None;
            for page in 1..=200 {
                let batch = session
                    .cloud_page(&file.path, page, cursor.as_deref())
                    .await?;
                pending.extend(
                    batch
                        .entries
                        .into_iter()
                        .map(|entry| (entry.file, directory.join(safe_name(&file.name)))),
                );
                if visited.len() + pending.len() > 10000 {
                    return Err("所选文件超过 10000 项，请分批下载".into());
                }
                cursor = batch.next;
                if cursor.is_none() {
                    break;
                }
                if page == 200 {
                    return Err("所选目录超过分页上限，请分批下载".into());
                }
            }
        } else {
            downloads.push((file, directory))
        }
    }
    let mut tasks = Vec::new();
    for (file, directory) in downloads {
        let existing = state.store.lock().unwrap().tasks()?;
        if existing.iter().any(|task| {
            task.share.provider == provider
                && task.own.as_ref().is_some_and(|source| {
                    source.account_id == account_id && source.file.id == file.id
                })
                && task.directory == directory.to_string_lossy()
                && !matches!(task.status.as_str(), "error" | "cancelled")
        }) {
            continue;
        }
        let name = crate::direct::available_name(&directory, &file.name, &existing);
        let task = Task {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            size: file.size,
            completed: 0,
            speed: 0,
            status: "resolving".into(),
            error: String::new(),
            directory: directory.to_string_lossy().into(),
            created_at: now(),
            gid: None,
            share: ShareLink {
                provider,
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
        tasks.push(start_task(&app, &state, task, None).await?);
    }
    if tasks.is_empty() {
        return Err("文件夹为空，或所选文件已在下载列表中".into());
    }
    Ok(tasks)
}
