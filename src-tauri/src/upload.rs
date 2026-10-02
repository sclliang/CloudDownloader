use crate::{model::*, providers::Session, session, AppState};
use futures_util::{stream, StreamExt};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tokio::sync::{watch, Semaphore};

pub const BLOCK_SIZE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadRecord {
    pub id: String,
    pub name: String,
    pub local_path: String,
    pub remote_path: String,
    pub account_id: String,
    pub size: u64,
    pub modified_ns: String,
    pub concurrency: usize,
    pub status: String,
    pub error: String,
    pub completed: u64,
    pub speed: u64,
    pub created_at: u64,
    #[serde(default)]
    pub hashes: Vec<String>,
    #[serde(default)]
    pub done: Vec<bool>,
    #[serde(default)]
    pub upload_id: String,
    #[serde(default)]
    pub remote_file: Option<DriveFile>,
    #[serde(default)]
    pub confirmed_requests: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadView {
    pub id: String,
    pub name: String,
    pub local_path: String,
    pub remote_path: String,
    pub size: u64,
    pub concurrency: usize,
    pub status: String,
    pub error: String,
    pub completed: u64,
    pub speed: u64,
    pub created_at: u64,
    pub completed_parts: usize,
    pub total_parts: usize,
}
impl From<UploadRecord> for UploadView {
    fn from(t: UploadRecord) -> Self {
        Self {
            completed_parts: t.done.iter().filter(|v| **v).count(),
            total_parts: t.hashes.len(),
            id: t.id,
            name: t.name,
            local_path: t.local_path,
            remote_path: t.remote_path,
            size: t.size,
            concurrency: t.concurrency,
            status: t.status,
            error: t.error,
            completed: t.completed,
            speed: t.speed,
            created_at: t.created_at,
        }
    }
}

pub struct UploadManager {
    running: Mutex<HashMap<String, watch::Sender<bool>>>,
    slots: Arc<Semaphore>,
    parts: Arc<Semaphore>,
}
impl Default for UploadManager {
    fn default() -> Self {
        Self {
            running: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(2)),
            parts: Arc::new(Semaphore::new(8)),
        }
    }
}

pub fn fingerprint(path: &Path) -> Result<(u64, String), String> {
    let meta = path
        .metadata()
        .map_err(|_| "无法读取本地文件，请检查文件是否存在")?;
    if !meta.is_file() {
        return Err("暂不支持上传文件夹，请选择文件夹内的文件".into());
    }
    let modified = meta
        .modified()
        .map_err(|_| "无法读取文件修改时间")?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "文件时间无效")?
        .as_nanos()
        .to_string();
    Ok((meta.len(), modified))
}
fn check_local(t: &UploadRecord) -> Result<(), String> {
    let (size, modified) = fingerprint(Path::new(&t.local_path))?;
    if size != t.size || modified != t.modified_ns {
        return Err("本地文件已修改，请移除该任务并重新选择文件".into());
    }
    Ok(())
}
pub fn hash_file(path: &Path, cancelled: &AtomicBool) -> Result<Vec<String>, String> {
    let mut file = std::fs::File::open(path).map_err(|_| "无法打开上传文件")?;
    let mut buffer = vec![0u8; BLOCK_SIZE as usize];
    let mut hashes = Vec::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("已暂停".into());
        }
        let mut length = 0;
        while length < buffer.len() {
            let n = file
                .read(&mut buffer[length..])
                .map_err(|_| "读取上传文件失败")?;
            if n == 0 {
                break;
            }
            length += n;
        }
        if length == 0 && !hashes.is_empty() {
            break;
        }
        hashes.push(format!("{:x}", Md5::digest(&buffer[..length])));
        if length < buffer.len() {
            break;
        }
    }
    Ok(hashes)
}
fn confirmed_size(t: &UploadRecord) -> u64 {
    t.done
        .iter()
        .enumerate()
        .filter(|(_, done)| **done)
        .map(|(i, _)| t.size.saturating_sub(i as u64 * BLOCK_SIZE).min(BLOCK_SIZE))
        .sum()
}
fn save(app: &tauri::AppHandle, task: &UploadRecord) -> Result<(), String> {
    app.state::<AppState>()
        .store
        .lock()
        .unwrap()
        .save_upload(task)?;
    let _ = app.emit_to("main", "tasks-changed", ());
    Ok(())
}

impl UploadManager {
    pub fn is_running(&self, id: &str) -> bool {
        self.running.lock().unwrap().contains_key(id)
    }
    pub fn pause(&self, id: &str) {
        if let Some(cancel) = self.running.lock().unwrap().get(id) {
            let _ = cancel.send(true);
        }
    }
    pub fn start(&self, app: tauri::AppHandle, task: UploadRecord) -> Result<(), String> {
        let (cancel, mut receiver) = watch::channel(false);
        {
            let mut running = self.running.lock().unwrap();
            if running.contains_key(&task.id) {
                return Err("上传任务正在运行或暂停中".into());
            }
            running.insert(task.id.clone(), cancel);
        }
        let slots = self.slots.clone();
        let parts = self.parts.clone();
        tauri::async_runtime::spawn(async move {
            let id = task.id.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let result = tokio::select! {
                result = run_upload(&app, task, slots, parts, stop.clone()) => result,
                _ = receiver.changed() => { stop.store(true, Ordering::Relaxed); Err("__paused".into()) }
            };
            let state = app.state::<AppState>();
            finish_state(&app, &id, result);
            state.uploads.running.lock().unwrap().remove(&id);
            let _ = app.emit_to("main", "tasks-changed", ());
        });
        Ok(())
    }
}

fn finish_state(app: &tauri::AppHandle, id: &str, result: Result<(), String>) {
    let state = app.state::<AppState>();
    let store = state.store.lock().unwrap();
    if let Ok(records) = store.uploads() {
        if let Some(mut latest) = records.into_iter().find(|t| t.id == id) {
            if let Err(error) = result {
                latest.status = if error == "__paused" {
                    "paused"
                } else {
                    "error"
                }
                .into();
                latest.error = if error == "__paused" {
                    String::new()
                } else {
                    error
                };
                if latest.error.contains("上传会话已失效") {
                    latest.upload_id.clear();
                    latest.done.fill(false);
                    latest.completed = 0;
                }
                latest.speed = 0;
                let _ = store.save_upload(&latest);
            }
        }
    }
}

async fn run_upload(
    app: &tauri::AppHandle,
    mut t: UploadRecord,
    slots: Arc<Semaphore>,
    parts: Arc<Semaphore>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let _slot = slots.acquire().await.map_err(|_| "上传队列已关闭")?;
    let session = session::connect(app, Provider::Baidu).await?;
    let identity = session.baidu_identity(Some(&t.account_id)).await?;
    check_local(&t)?;
    if session.drive_find(&t.remote_path).await?.is_some() {
        return Err("目标位置已有同名文件，请修改本地文件名或选择其他目录上传".into());
    }
    t.status = "hashing".into();
    save(app, &t)?;
    let path = t.local_path.clone();
    let hashes = tokio::task::spawn_blocking(move || hash_file(Path::new(&path), &stop))
        .await
        .map_err(|_| "文件校验任务失败")??;
    check_local(&t)?;
    if !t.hashes.is_empty() && t.hashes != hashes {
        return Err("本地文件内容已改变，请重新创建上传任务".into());
    }
    t.hashes = hashes;
    let pre = session
        .upload_precreate(&identity, &t.remote_path, t.size, &t.hashes, &t.upload_id)
        .await?;
    #[cfg(debug_assertions)]
    if std::env::args().any(|a| a.starts_with("--verify-baidu-")) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.verification");
        let _ = std::fs::write(dir.join("resume-checkpoint.json"), serde_json::json!({"savedParts": t.done.iter().filter(|v| **v).count(), "totalParts": t.hashes.len(), "serverMissing": pre.missing.len(), "sameSession": t.upload_id == pre.upload_id, "confirmedRequests": t.confirmed_requests}).to_string());
    }
    t.upload_id = pre.upload_id;
    t.done = vec![true; t.hashes.len()];
    for index in &pre.missing {
        t.done[*index] = false;
    }
    t.completed = confirmed_size(&t);
    t.status = "active".into();
    save(app, &t)?;
    let baseline = t.completed;
    let started = Instant::now();
    let pending = t.clone();
    let requests = stream::iter(pre.missing)
        .map(|index| {
            let session = &session;
            let token = &identity.token;
            let pending = &pending;
            let parts = &parts;
            async move {
                let _permit = parts.acquire().await.map_err(|_| "上传已停止")?;
                upload_one(session, token, pending, index)
                    .await
                    .map(|_| index)
            }
        })
        .buffer_unordered(t.concurrency);
    tokio::pin!(requests);
    while let Some(result) = requests.next().await {
        let index = result?;
        t.done[index] = true;
        t.confirmed_requests += 1;
        t.completed = confirmed_size(&t);
        t.speed =
            ((t.completed - baseline) as f64 / started.elapsed().as_secs_f64().max(0.001)) as u64;
        save(app, &t)?;
    }
    check_local(&t)?;
    t.status = "merging".into();
    t.speed = 0;
    save(app, &t)?;
    let remote = if pre.complete {
        session
            .drive_meta(std::slice::from_ref(&t.remote_path))
            .await?
            .into_iter()
            .next()
            .ok_or("上传完成但未查到文件")?
    } else {
        session
            .upload_finish(&identity, &t.remote_path, t.size, &t.hashes, &t.upload_id)
            .await?
    };
    if remote.size != t.size || remote.path != t.remote_path {
        return Err("云端文件信息与上传任务不一致，请检查目标目录".into());
    }
    t.remote_file = Some(remote);
    t.status = "complete".into();
    t.completed = t.size;
    t.upload_id.clear();
    save(app, &t)?;
    Ok(())
}

async fn upload_one(
    session: &Session,
    token: &str,
    task: &UploadRecord,
    index: usize,
) -> Result<(), String> {
    upload_one_with(task, index, Duration::from_secs(1), |bytes| {
        session.upload_part(token, &task.remote_path, &task.upload_id, index, bytes)
    })
    .await
}

async fn upload_one_with<F, Fut>(
    task: &UploadRecord,
    index: usize,
    retry_delay: Duration,
    send: F,
) -> Result<(), String>
where
    F: Fn(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut last_error = String::new();
    for attempt in 0..4 {
        if attempt > 0 {
            tokio::time::sleep(retry_delay * (1 << (attempt - 1))).await;
        }
        check_local(task)?;
        let path = task.local_path.clone();
        let size = task
            .size
            .saturating_sub(index as u64 * BLOCK_SIZE)
            .min(BLOCK_SIZE) as usize;
        let bytes = tokio::task::spawn_blocking(move || {
            let mut f = std::fs::File::open(path).map_err(|_| "读取分片失败")?;
            f.seek(SeekFrom::Start(index as u64 * BLOCK_SIZE))
                .map_err(|_| "读取分片位置失败")?;
            let mut data = vec![0; size];
            f.read_exact(&mut data).map_err(|_| "本地文件内容已改变")?;
            Ok::<_, String>(data)
        })
        .await
        .map_err(|_| "分片读取任务失败")??;
        if format!("{:x}", Md5::digest(&bytes)) != task.hashes[index] {
            return Err("本地文件内容已改变，请重新创建上传任务".into());
        }
        match send(bytes).await {
            Ok(md5) if md5.eq_ignore_ascii_case(&task.hashes[index]) => return Ok(()),
            Ok(_) => last_error = "百度返回的分片校验值不匹配".into(),
            Err(e) => last_error = e,
        }
    }
    Err(format!("第 {} 个分片上传失败：{last_error}", index + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn fixture(bytes: &[u8]) -> (PathBufGuard, UploadRecord) {
        let path = std::env::temp_dir().join(format!(
            "clouddownloader-upload-test-{}.bin",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, bytes).unwrap();
        let (size, modified_ns) = fingerprint(&path).unwrap();
        let hashes = hash_file(&path, &AtomicBool::new(false)).unwrap();
        let record = UploadRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: "test.bin".into(),
            local_path: path.to_string_lossy().into(),
            remote_path: "/test.bin".into(),
            account_id: "account".into(),
            size,
            modified_ns,
            concurrency: 4,
            status: "paused".into(),
            error: String::new(),
            completed: 0,
            speed: 0,
            created_at: now(),
            done: vec![false; hashes.len()],
            hashes,
            upload_id: "session-private".into(),
            remote_file: None,
            confirmed_requests: 0,
        };
        (PathBufGuard(path), record)
    }
    struct PathBufGuard(std::path::PathBuf);
    impl Drop for PathBufGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[tokio::test]
    async fn retry_resends_only_the_failed_slice_and_checks_response_hash() {
        let mut data = vec![37u8; BLOCK_SIZE as usize];
        data.extend_from_slice(b"final slice");
        let (_path, record) = fixture(&data);
        let calls = AtomicUsize::new(0);
        upload_one_with(&record, 1, Duration::ZERO, |bytes| {
            assert_eq!(bytes, b"final slice");
            let attempt = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                match attempt {
                    0 => Err("transient connection failure".into()),
                    1 => Ok("bad-md5".into()),
                    _ => Ok(format!("{:x}", Md5::digest(&bytes))),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let calls = AtomicUsize::new(0);
        assert!(upload_one_with(&record, 1, Duration::ZERO, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err("offline".into()) }
        })
        .await
        .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn refuses_changed_source_before_resending_or_resuming() {
        let (path, record) = fixture(b"original");
        let calls = AtomicUsize::new(0);
        let result = upload_one_with(&record, 0, Duration::ZERO, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::fs::write(&path.0, b"modified and larger").unwrap();
            async { Err("transient".into()) }
        })
        .await;
        assert!(result.unwrap_err().contains("本地文件已修改"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn checkpoints_survive_reopen_and_private_fields_stay_out_of_ui() {
        let mut data = vec![4; BLOCK_SIZE as usize];
        data.extend_from_slice(b"tail");
        let (_path, mut record) = fixture(&data);
        record.done[0] = true;
        record.completed = confirmed_size(&record);
        let db_path = std::env::temp_dir().join(format!(
            "clouddownloader-upload-test-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        {
            let store = crate::store::Store::open(&db_path).unwrap();
            store.save_upload(&record).unwrap();
        }
        {
            let store = crate::store::Store::open(&db_path).unwrap();
            let resumed = store.uploads().unwrap().remove(0);
            assert_eq!(resumed.upload_id, record.upload_id);
            assert_eq!(resumed.done, vec![true, false]);
            assert_eq!(resumed.completed, BLOCK_SIZE);
            assert_eq!(resumed.hashes, record.hashes);
            let view = serde_json::to_string(&UploadView::from(resumed)).unwrap();
            assert!(!view.contains("session-private"));
            assert!(!view.contains(&record.hashes[0]));
        }
        std::fs::remove_file(db_path).unwrap();
        let (_path, empty) = fixture(&[]);
        assert_eq!(empty.hashes, vec!["d41d8cd98f00b204e9800998ecf8427e"]);
    }
}
