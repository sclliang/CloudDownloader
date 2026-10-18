mod cloud_commands;
mod direct;
mod direct_commands;
mod download_errors;
mod drive_commands;
mod engine;
mod migration;
mod model;
mod pan115_session;
mod providers;
mod quark;
mod session;
mod store;
mod task_removal;
mod tray;
mod upload;
#[cfg(debug_assertions)]
mod verification;

use engine::{Engine, Rpc};
use model::*;
use serde_json::json;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};
use tauri::{Emitter, Manager, State, WebviewWindow};

struct AppState {
    store: Mutex<store::Store>,
    engine: Mutex<Option<Engine>>,
    engine_error: Mutex<String>,
    accounts: Mutex<Vec<Account>>,
    resolving: tokio::sync::Mutex<()>,
    uploads: upload::UploadManager,
    session_cache_error: Mutex<String>,
    exiting: AtomicBool,
}

fn trusted(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("此窗口无权执行本地操作".into());
    }
    Ok(())
}

fn rpc(state: &AppState) -> Result<Rpc, String> {
    state
        .engine
        .lock()
        .unwrap()
        .as_ref()
        .map(|e| e.rpc.clone())
        .ok_or_else(|| state.engine_error.lock().unwrap().clone())
}

#[tauri::command]
async fn open_login(
    window: WebviewWindow,
    app: tauri::AppHandle,
    provider: Provider,
) -> Result<(), String> {
    trusted(&window)?;
    session::window(&app, provider, true)?;
    Ok(())
}

#[tauri::command]
async fn refresh_account(
    provider: Provider,
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Account, String> {
    trusted(&window)?;
    let result = match session::connect(&app, provider).await {
        Ok(session) => session.account().await,
        Err(error) => Err(error),
    };
    let account = match result {
        Ok(name) => Account {
            provider,
            logged_in: true,
            name,
            message: if provider == Provider::Pan115 {
                let warning = state.session_cache_error.lock().unwrap().clone();
                if warning.is_empty() {
                    "账号已连接".into()
                } else {
                    format!("账号已连接；{warning}")
                }
            } else {
                "账号已连接".into()
            },
        },
        Err(message) => Account {
            provider,
            message,
            ..Account::default()
        },
    };
    if let Some(current) = state
        .accounts
        .lock()
        .unwrap()
        .iter_mut()
        .find(|a| a.provider == provider)
    {
        *current = account.clone();
    }
    Ok(account)
}

#[tauri::command]
async fn parse_link(
    window: WebviewWindow,
    app: tauri::AppHandle,
    text: String,
    code: String,
    parent_id: Option<String>,
) -> Result<ShareListing, String> {
    trusted(&window)?;
    let share = providers::parse_share(&text, &code)?;
    session::connect(&app, share.provider)
        .await?
        .list(&share, parent_id.as_deref().unwrap_or("0"))
        .await
}

#[tauri::command]
fn snapshot(window: WebviewWindow, state: State<'_, AppState>) -> Result<Snapshot, String> {
    trusted(&window)?;
    let store = state.store.lock().unwrap();
    let engine_running = state
        .engine
        .lock()
        .unwrap()
        .as_mut()
        .is_some_and(Engine::alive);
    Ok(Snapshot {
        accounts: state.accounts.lock().unwrap().clone(),
        tasks: store.tasks()?,
        download_dir: store.setting("download_dir").unwrap_or_default(),
        engine_running,
        engine_error: state.engine_error.lock().unwrap().clone(),
        uploads: store.uploads()?.into_iter().map(Into::into).collect(),
        shares: store.shares()?,
    })
}

#[tauri::command]
fn set_directory(
    window: WebviewWindow,
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    trusted(&window)?;
    let directory = PathBuf::from(&path);
    if !directory.is_absolute() || !directory.is_dir() {
        return Err("请选择已存在的本地文件夹".into());
    }
    state
        .store
        .lock()
        .unwrap()
        .set_setting("download_dir", &path)
}

async fn start_task(
    app: &tauri::AppHandle,
    state: &AppState,
    mut task: Task,
    prepared_direct: Option<direct::Metadata>,
) -> Result<Task, String> {
    task.status = "resolving".into();
    task.error.clear();
    task.speed = 0;
    state.store.lock().unwrap().save(&task)?;
    let result = async {
        std::fs::create_dir_all(&task.directory)
            .map_err(|_| "无法创建下载目录，请选择可写的文件夹")?;
        if let Some(source) = &mut task.direct {
            let metadata = match prepared_direct {
                Some(metadata) => metadata,
                None => direct::probe(&source.url).await?,
            };
            let partial = task.completed > 0
                || PathBuf::from(&task.directory)
                    .join(&task.name)
                    .metadata()
                    .is_ok_and(|m| m.len() > 0);
            direct::update_source(source, task.size, partial, &metadata)?;
            task.size = metadata.size;
            task.file.size = metadata.size;
            return rpc(state)?
                .add_direct(source, &task.directory, &task.name)
                .await;
        }
        let session = session::connect(app, task.share.provider).await?;
        session.account().await?;
        let download = if let Some(source) = &task.own {
            if task.share.provider == Provider::Baidu {
                session.drive_download(source).await?
            } else {
                session.cloud_download(source).await?
            }
        } else {
            let listing = session.list(&task.share, &task.file.parent_id).await?;
            let fresh = listing
                .files
                .into_iter()
                .find(|f| f.id == task.file.id)
                .ok_or("分享中的文件已经被移除")?;
            if task.completed > 0 && fresh.size != task.size {
                return Err("源文件大小已变化，请重新解析分享并创建新任务".into());
            }
            session
                .download(&task.share, &fresh, &listing.stoken)
                .await?
        };
        let gid = rpc(state)?
            .add(
                &download.url,
                &task.directory,
                &task.name,
                &download.headers,
                &download.user_agent,
            )
            .await?;
        Ok::<String, String>(gid)
    }
    .await;
    match result {
        Ok(gid) => {
            task.gid = Some(gid);
            task.status = "waiting".into();
        }
        Err(error) => {
            task.status = "error".into();
            task.error = error;
        }
    }
    state.store.lock().unwrap().save(&task)?;
    let _ = app.emit_to("main", "tasks-changed", ());
    Ok(task)
}

#[tauri::command]
async fn enqueue(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    share: ShareLink,
    parent_id: String,
    ids: Vec<String>,
) -> Result<Vec<Task>, String> {
    trusted(&window)?;
    let _guard = state.resolving.lock().await;
    let session = session::connect(&app, share.provider).await?;
    let name = session.account().await?;
    let account = Account {
        provider: share.provider,
        logged_in: true,
        name,
        message: "账号已连接".into(),
    };
    if let Some(current) = state
        .accounts
        .lock()
        .unwrap()
        .iter_mut()
        .find(|a| a.provider == share.provider)
    {
        *current = account;
    }
    if ids.is_empty() {
        return Err("请先选择文件".into());
    }
    let directory = state
        .store
        .lock()
        .unwrap()
        .setting("download_dir")
        .ok_or("请先选择下载目录")?;
    let listing = session.list(&share, &parent_id).await?;
    let selected: Vec<_> = listing
        .files
        .into_iter()
        .filter(|f| ids.contains(&f.id))
        .collect();
    if selected.len() != ids.len() {
        return Err("文件列表已变化，请重新解析分享".into());
    }
    let mut pending: Vec<(ShareFile, PathBuf)> = selected
        .into_iter()
        .map(|f| (f, PathBuf::from(&directory)))
        .collect();
    let mut files = Vec::new();
    let mut visited = HashSet::new();
    while let Some((file, dir)) = pending.pop() {
        if !visited.insert(file.id.clone()) {
            continue;
        }
        if visited.len() > 10000 {
            return Err("所选文件过多，请分批下载".into());
        }
        if file.is_dir {
            let listing = session.list(&share, &file.id).await?;
            let subdir = dir.join(safe_name(&file.name));
            pending.extend(listing.files.into_iter().map(|f| (f, subdir.clone())));
        } else {
            files.push((file, dir));
        }
    }
    let mut created = Vec::new();
    for (file, dir) in files {
        let existing = state.store.lock().unwrap().tasks()?;
        if existing.iter().any(|t| {
            t.share.provider == share.provider
                && t.share.id == share.id
                && t.file.id == file.id
                && t.directory == dir.to_string_lossy()
                && !matches!(t.status.as_str(), "error" | "cancelled")
        }) {
            continue;
        }
        let base_name = safe_name(&file.name);
        let mut name = base_name.clone();
        let mut number = 1;
        while dir.join(&name).exists()
            || existing.iter().any(|t| {
                t.directory.eq_ignore_ascii_case(&dir.to_string_lossy())
                    && t.name.eq_ignore_ascii_case(&name)
            })
        {
            let p = PathBuf::from(&base_name);
            let stem = p.file_stem().unwrap_or_default().to_string_lossy();
            let ext = p
                .extension()
                .map(|v| format!(".{}", v.to_string_lossy()))
                .unwrap_or_default();
            name = format!("{stem} ({number}){ext}");
            number += 1;
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
            share: share.clone(),
            file,
            own: None,
            direct: None,
            connections: 0,
        };
        created.push(start_task(&app, &state, task, None).await?);
    }
    if created.is_empty() {
        return Err("所选文件已在下载列表中，或文件夹为空".into());
    }
    Ok(created)
}

#[tauri::command]
async fn task_action(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    action: String,
    delete_files: Option<bool>,
) -> Result<(), String> {
    trusted(&window)?;
    let _guard = state.resolving.lock().await;
    let mut task = state
        .store
        .lock()
        .unwrap()
        .tasks()?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or("下载任务不存在")?;
    match action.as_str() {
        "pause" => {
            if let Some(gid) = &task.gid {
                rpc(&state)?
                    .call("aria2.forcePause", vec![json!(gid)])
                    .await?;
            }
            task.status = "paused".into();
            task.speed = 0;
            task.connections = 0;
            state.store.lock().unwrap().save(&task)?;
        }
        "resume" => {
            if !matches!(task.status.as_str(), "paused" | "error" | "cancelled") {
                return Err("任务当前不能恢复".into());
            }
            if let Some(gid) = &task.gid {
                let rpc = rpc(&state)?;
                let _ = rpc.call("aria2.forceRemove", vec![json!(gid)]).await;
                let _ = rpc
                    .call("aria2.removeDownloadResult", vec![json!(gid)])
                    .await;
            }
            task.gid = None;
            start_task(&app, &state, task, None).await?;
        }
        "remove" => {
            if let Some(gid) = &task.gid {
                rpc(&state)?.stop_for_removal(gid).await?;
            }
            task.gid = None;
            task.speed = 0;
            task.connections = 0;
            if task.status != "complete" {
                task.status = "paused".into();
            }
            let store = state.store.lock().unwrap();
            // Keep a stopped, retryable record if filesystem cleanup fails.
            store.save(&task)?;
            task_removal::remove_record(&store, &task, delete_files.unwrap_or(false))?;
        }
        _ => return Err("未知任务操作".into()),
    }
    Ok(())
}

#[tauri::command]
fn open_directory(
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: Option<String>,
) -> Result<(), String> {
    trusted(&window)?;
    let store = state.store.lock().unwrap();
    let path = if let Some(id) = id {
        store
            .tasks()?
            .into_iter()
            .find(|t| t.id == id)
            .map(|t| t.directory)
            .ok_or("任务不存在")?
    } else {
        store.setting("download_dir").ok_or("未设置下载目录")?
    };
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

async fn poll_engine(app: tauri::AppHandle) {
    loop {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let state = app.state::<AppState>();
        let Ok(rpc) = rpc(&state) else {
            continue;
        };
        let tasks = state.store.lock().unwrap().tasks().unwrap_or_default();
        for mut task in tasks {
            if !matches!(task.status.as_str(), "active" | "waiting" | "paused") {
                continue;
            }
            let Some(gid) = &task.gid else {
                continue;
            };
            let before_status = task.status.clone();
            let Ok(value) = rpc
                .call(
                    "aria2.tellStatus",
                    vec![
                        json!(gid),
                        json!([
                            "status",
                            "totalLength",
                            "completedLength",
                            "downloadSpeed",
                            "connections",
                            "errorCode",
                            "errorMessage"
                        ]),
                    ],
                )
                .await
            else {
                continue;
            };
            let number = |key: &str| {
                value[key]
                    .as_str()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            task.status = value["status"].as_str().unwrap_or("waiting").into();
            task.completed = number("completedLength");
            task.speed = if task.status == "active" {
                number("downloadSpeed")
            } else {
                0
            };
            task.connections = number("connections") as u32;
            if number("totalLength") > 0 {
                task.size = number("totalLength");
            }
            if task.status == "error" {
                let code = value["errorCode"].as_str().unwrap_or("未知");
                task.error = download_errors::message(
                    code,
                    value["errorMessage"].as_str().unwrap_or_default(),
                    task.direct.is_some(),
                );
            }
            let store = state.store.lock().unwrap();
            if store
                .tasks()
                .unwrap_or_default()
                .iter()
                .any(|t| t.id == task.id && t.gid == task.gid && t.status == before_status)
            {
                let _ = store.save(&task);
            }
        }
        let _ = app.emit_to("main", "tasks-changed", ());
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            tray::restore(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            tray::setup(app)?;
            let dir = app.path().app_data_dir()?;
            migration::migrate(&dir)?;
            std::fs::create_dir_all(&dir)?;
            let store = store::Store::open(&dir.join("tasks.sqlite3"))?;
            if store.setting("download_dir").is_none() {
                let downloads = app
                    .path()
                    .download_dir()
                    .unwrap_or(dir.clone())
                    .join("CloudDownloader");
                std::fs::create_dir_all(&downloads)?;
                store.set_setting("download_dir", &downloads.to_string_lossy())?;
            }
            for mut task in store.tasks()? {
                if matches!(task.status.as_str(), "active" | "waiting" | "resolving") {
                    task.status = "paused".into();
                }
                task.gid = None;
                task.speed = 0;
                task.connections = 0;
                store.save(&task)?;
            }
            for mut upload in store.uploads()? {
                if !matches!(upload.status.as_str(), "complete" | "error" | "paused") {
                    upload.status = "paused".into();
                }
                upload.speed = 0;
                store.save_upload(&upload)?;
            }
            let (engine, engine_error) = match Engine::start() {
                Ok(v) => (Some(v), String::new()),
                Err(e) => (None, e),
            };
            app.manage(AppState {
                store: Mutex::new(store),
                engine: Mutex::new(engine),
                engine_error: Mutex::new(engine_error),
                accounts: Mutex::new(
                    Provider::ALL
                        .into_iter()
                        .map(|provider| Account {
                            provider,
                            ..Account::default()
                        })
                        .collect(),
                ),
                resolving: tokio::sync::Mutex::new(()),
                uploads: upload::UploadManager::default(),
                session_cache_error: Mutex::new(String::new()),
                exiting: AtomicBool::new(false),
            });
            if let Some(main) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                main.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.hide();
                        }
                    }
                });
            }
            tauri::async_runtime::spawn(poll_engine(app.handle().clone()));
            #[cfg(debug_assertions)]
            if let Some(mode) = std::env::args().find(|a| {
                a.starts_with("--verify-baidu-")
                    || a == "--verify-cloud-diagnostics"
                    || matches!(
                        a.as_str(),
                        "--verify-pan115-cache-seed"
                            | "--verify-pan115-cache-restore"
                            | "--verify-pan115-session"
                    )
            }) {
                verification::start(app.handle().clone(), mode);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_login,
            refresh_account,
            parse_link,
            snapshot,
            set_directory,
            enqueue,
            direct_commands::enqueue_direct,
            task_action,
            open_directory,
            drive_commands::drive_list,
            cloud_commands::cloud_list,
            cloud_commands::cloud_enqueue,
            drive_commands::drive_enqueue,
            drive_commands::drive_mkdir,
            drive_commands::drive_share,
            drive_commands::drive_cancel_share,
            drive_commands::upload_enqueue,
            drive_commands::upload_action
        ])
        .build(tauri::generate_context!())
        .expect("程序初始化失败");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = &event {
            if let Some(state) = app.try_state::<AppState>() {
                if !state.exiting.swap(true, Ordering::SeqCst) {
                    api.prevent_exit();
                    let app = app.clone();
                    let code = code.unwrap_or(0);
                    tauri::async_runtime::spawn(async move {
                        let save_app = app.clone();
                        let save = tauri::async_runtime::spawn_blocking(move || {
                            session::save_before_exit(&save_app)
                        });
                        let _ = tokio::time::timeout(Duration::from_secs(3), save).await;
                        app.exit(code);
                    });
                }
            }
        }
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = app.try_state::<AppState>() {
                state.engine.lock().unwrap().take();
            }
        }
    });
}
