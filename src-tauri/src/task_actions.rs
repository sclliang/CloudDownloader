use crate::{model::Task, rpc, start_task, task_removal, trusted, AppState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use tauri::{AppHandle, State, WebviewWindow};

#[cfg(all(test, windows))]
#[path = "task_actions_tests.rs"]
mod tests;

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Pause,
    Resume,
    Remove,
}

impl Action {
    fn applies_to(self, status: &str) -> bool {
        match self {
            Self::Pause => matches!(status, "active" | "waiting" | "seeding"),
            Self::Resume => matches!(status, "paused" | "error" | "cancelled"),
            Self::Remove => true,
        }
    }
}

#[derive(Serialize)]
struct Failure {
    id: String,
    name: String,
    message: String,
}

#[derive(Default, Serialize)]
pub struct BatchResult {
    changed: usize,
    failures: Vec<Failure>,
}

async fn stop_download(state: &AppState, task: &mut Task) -> Result<(), String> {
    if let Some(gid) = &task.gid {
        rpc(state)?.stop_for_removal(gid).await?;
        task.gid = None;
    }
    Ok(())
}

async fn apply_action(
    app: &AppHandle,
    state: &AppState,
    id: &str,
    action: Action,
    delete_files: bool,
) -> Result<bool, String> {
    // Re-read under the same lock used for enqueue. An earlier item in a batch
    // can take long enough for this task to finish or for another action to run.
    let _guard = state.resolving.lock().await;
    let Some(mut task) = state.store.lock().unwrap().task(id)? else {
        return Ok(false);
    };
    if !action.applies_to(&task.status) {
        return Ok(false);
    }
    match action {
        Action::Pause => {
            if let Some(gid) = &task.gid {
                rpc(state)?
                    .call("aria2.forcePause", vec![json!(gid)])
                    .await?;
            }
        }
        Action::Resume => {
            stop_download(state, &mut task).await?;
            let restarted = start_task(app, state, task, None).await?;
            if restarted.status == "error" {
                return Err(restarted.error);
            }
            return Ok(true);
        }
        Action::Remove => stop_download(state, &mut task).await?,
    }
    if task.status != "complete" {
        task.status = "paused".into();
    }
    task.speed = 0;
    task.upload_speed = 0;
    task.connections = 0;
    let store = state.store.lock().unwrap();
    // Keep the stopped task retryable if local file cleanup fails.
    store.save(&task)?;
    if action == Action::Remove {
        task_removal::remove_record(&store, &task, delete_files)?;
    }
    Ok(true)
}

async fn apply_batch(
    app: &AppHandle,
    state: &AppState,
    ids: Option<HashSet<String>>,
    action: Action,
    delete_files: bool,
) -> Result<BatchResult, String> {
    let tasks = state.store.lock().unwrap().tasks()?;
    let mut result = BatchResult::default();
    for task in tasks {
        if !action.applies_to(&task.status)
            || ids.as_ref().is_some_and(|ids| !ids.contains(&task.id))
        {
            continue;
        }
        match apply_action(app, state, &task.id, action, delete_files).await {
            Ok(true) => result.changed += 1,
            Ok(false) => {}
            Err(message) => result.failures.push(Failure {
                id: task.id,
                name: task.name,
                message,
            }),
        }
    }
    Ok(result)
}

#[tauri::command]
pub async fn task_action(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    action: Action,
    delete_files: Option<bool>,
) -> Result<(), String> {
    trusted(&window)?;
    if apply_action(&app, &state, &id, action, delete_files.unwrap_or(false)).await? {
        Ok(())
    } else {
        Err("任务已移除或当前状态不支持该操作".into())
    }
}

#[tauri::command]
pub async fn task_action_all(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    action: Action,
) -> Result<BatchResult, String> {
    trusted(&window)?;
    if action == Action::Remove {
        return Err("请先选择要移除的任务".into());
    }
    apply_batch(&app, &state, None, action, false).await
}

#[tauri::command]
pub async fn task_action_selected(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    ids: HashSet<String>,
    action: Action,
    delete_files: Option<bool>,
) -> Result<BatchResult, String> {
    trusted(&window)?;
    apply_batch(
        &app,
        &state,
        Some(ids),
        action,
        delete_files.unwrap_or(false),
    )
    .await
}
