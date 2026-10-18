use crate::{direct, model::*, start_task, trusted, AppState};
use std::path::PathBuf;
use tauri::{State, WebviewWindow};

#[tauri::command]
pub async fn enqueue_direct(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    url: String,
    filename: String,
    connections: u8,
) -> Result<Task, String> {
    trusted(&window)?;
    direct::validate_connections(connections)?;
    let url = direct::validate_url(&url)?.to_string();
    let _guard = state.resolving.lock().await;
    let directory = state
        .store
        .lock()
        .unwrap()
        .setting("download_dir")
        .ok_or("请先选择下载目录")?;
    let existing = state.store.lock().unwrap().tasks()?;
    if existing.iter().any(|t| {
        t.direct.as_ref().is_some_and(|s| s.url == url)
            && t.directory.eq_ignore_ascii_case(&directory)
            && !matches!(t.status.as_str(), "complete" | "error" | "cancelled")
    }) {
        return Err("该链接已在下载列表中，可继续现有任务".into());
    }
    let metadata = direct::probe(&url).await?;
    let name = direct::available_name(
        &PathBuf::from(&directory),
        if filename.trim().is_empty() {
            &metadata.name
        } else {
            filename.trim()
        },
        &existing,
    );
    let task = Task {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.clone(),
        size: metadata.size,
        completed: 0,
        speed: 0,
        status: "resolving".into(),
        error: String::new(),
        directory,
        created_at: now(),
        gid: None,
        // Kept for compatibility with earlier task records; direct sources are resolved first.
        share: ShareLink {
            provider: Provider::Quark,
            id: String::new(),
            passcode: String::new(),
        },
        file: ShareFile {
            id: String::new(),
            name,
            size: metadata.size,
            is_dir: false,
            parent_id: String::new(),
            token: String::new(),
        },
        own: None,
        direct: Some(DirectSource {
            url,
            connections,
            range_supported: metadata.range_supported,
            etag: metadata.etag.clone(),
            last_modified: metadata.last_modified.clone(),
        }),
        connections: 0,
        bt: None,
        upload_speed: 0,
    };
    start_task(&app, &state, task, Some(metadata)).await
}
