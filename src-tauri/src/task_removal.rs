use crate::{
    model::{safe_name, Task},
    store::Store,
};
use std::{fs, io::ErrorKind, path::Path};

/// Called only after the engine has released this task's output files.
pub fn remove_record(store: &Store, task: &Task, delete_files: bool) -> Result<(), String> {
    if delete_files {
        delete_local_files(store, task)?;
    }
    store.remove(&task.id)
}

fn delete_local_files(store: &Store, task: &Task) -> Result<(), String> {
    let directory = Path::new(&task.directory);
    if !directory.is_absolute() || safe_name(&task.name) != task.name {
        return Err("任务的本地文件路径无效，未删除文件。可取消勾选，仅移除任务记录。".into());
    }
    let directory = match fs::canonicalize(directory) {
        Ok(path) => path,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(cleanup_error(directory, error)),
    };
    let files = [
        directory.join(&task.name),
        directory.join(format!("{}.aria2", task.name)),
    ];
    // Legacy or overlapping task records must not let one task delete another's output.
    for other in store.tasks()? {
        if other.id == task.id {
            continue;
        }
        let Ok(other_dir) = fs::canonicalize(&other.directory) else {
            continue;
        };
        for name in [&other.name, &format!("{}.aria2", other.name)] {
            let other_file = other_dir.join(name);
            if files.iter().any(|file| same_path(file, &other_file)) {
                return Err("该文件或续传信息还被其他下载任务使用。请先处理其他任务，或取消勾选，仅移除当前任务记录。".into());
            }
        }
    }
    // Validate both entries before deleting either. Never recurse into a directory
    // or follow a file symlink/reparse point when cleaning a download record.
    for file in &files {
        match fs::symlink_metadata(file) {
            Ok(metadata) => {
                #[cfg(windows)]
                let reparse_point = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse_point = false;
                if !metadata.is_file() || metadata.file_type().is_symlink() || reparse_point {
                    return Err(format!(
                        "本地路径不是普通文件，未执行删除：{}。任务记录已保留。",
                        file.display()
                    ));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(cleanup_error(file, error)),
        }
    }
    // Remove the output first: if it is locked, preserve its resume information.
    for file in files {
        match fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(cleanup_error(&file, error)),
        }
    }
    Ok(())
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

fn cleanup_error(path: &Path, error: std::io::Error) -> String {
    format!(
        "本地文件清理未完成：{}（{}）。任务记录已保留，请检查文件占用和目录权限后重试。",
        path.display(),
        error
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Provider, ShareFile, ShareLink};
    use std::path::PathBuf;

    struct Fixture {
        directory: PathBuf,
        store: Option<Store>,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir()
                .join(format!("clouddownloader-removal-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&directory).unwrap();
            let store = Store::open(&directory.join("tasks.sqlite3")).unwrap();
            Self {
                directory,
                store: Some(store),
            }
        }
        fn store(&self) -> &Store {
            self.store.as_ref().unwrap()
        }
        fn task(&self, name: &str) -> Task {
            let task = Task {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.into(),
                size: 10,
                completed: 5,
                speed: 0,
                status: "paused".into(),
                error: String::new(),
                directory: self.directory.to_string_lossy().into(),
                created_at: 1,
                gid: None,
                share: ShareLink {
                    provider: Provider::Quark,
                    id: String::new(),
                    passcode: String::new(),
                },
                file: ShareFile {
                    id: String::new(),
                    name: name.into(),
                    size: 10,
                    is_dir: false,
                    parent_id: String::new(),
                    token: String::new(),
                },
                own: None,
                direct: None,
                connections: 0,
            };
            self.store().save(&task).unwrap();
            task
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.store.take();
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }

    #[test]
    fn keeping_files_preserves_output_and_checkpoint() {
        let fixture = Fixture::new();
        let task = fixture.task("keep.zip");
        fs::write(fixture.directory.join("keep.zip"), b"partial").unwrap();
        fs::write(fixture.directory.join("keep.zip.aria2"), b"resume").unwrap();
        remove_record(fixture.store(), &task, false).unwrap();
        assert!(fixture.store().tasks().unwrap().is_empty());
        assert_eq!(
            fs::read(fixture.directory.join("keep.zip")).unwrap(),
            b"partial"
        );
        assert_eq!(
            fs::read(fixture.directory.join("keep.zip.aria2")).unwrap(),
            b"resume"
        );
    }

    #[test]
    fn deleting_cleans_only_task_files_and_tolerates_missing_files() {
        let fixture = Fixture::new();
        let task = fixture.task("delete.zip");
        fs::write(fixture.directory.join("delete.zip"), b"partial").unwrap();
        fs::write(fixture.directory.join("delete.zip.aria2"), b"resume").unwrap();
        fs::write(fixture.directory.join("other.zip"), b"keep").unwrap();
        remove_record(fixture.store(), &task, true).unwrap();
        assert!(!fixture.directory.join("delete.zip").exists());
        assert!(!fixture.directory.join("delete.zip.aria2").exists());
        assert!(fixture.directory.join("other.zip").is_file());
        let completed = fixture.task("complete.zip");
        fs::write(fixture.directory.join("complete.zip"), b"complete").unwrap();
        remove_record(fixture.store(), &completed, true).unwrap();
        assert!(!fixture.directory.join("complete.zip").exists());
        let missing = fixture.task("missing.zip");
        remove_record(fixture.store(), &missing, true).unwrap();
        assert!(fixture.store().tasks().unwrap().is_empty());
    }

    #[test]
    fn invalid_paths_and_directories_keep_the_record_and_files() {
        let fixture = Fixture::new();
        let mut task = fixture.task("file.zip");
        fs::write(fixture.directory.join("file.zip"), b"keep").unwrap();
        task.name = "../outside.zip".into();
        assert!(remove_record(fixture.store(), &task, true).is_err());
        task.name = "file.zip".into();
        fs::create_dir(fixture.directory.join("file.zip.aria2")).unwrap();
        fs::write(fixture.directory.join("file.zip.aria2/inside.txt"), b"keep").unwrap();
        assert!(remove_record(fixture.store(), &task, true).is_err());
        assert_eq!(
            fs::read(fixture.directory.join("file.zip")).unwrap(),
            b"keep"
        );
        assert!(fixture
            .directory
            .join("file.zip.aria2/inside.txt")
            .is_file());
        assert_eq!(fixture.store().tasks().unwrap().len(), 1);
    }

    #[test]
    fn overlapping_task_paths_are_not_deleted() {
        let fixture = Fixture::new();
        let task = fixture.task("file.zip");
        let other = fixture.task("FILE.ZIP.aria2");
        fs::write(fixture.directory.join(&other.name), b"other task").unwrap();
        assert!(remove_record(fixture.store(), &task, true).is_err());
        assert_eq!(fixture.store().tasks().unwrap().len(), 2);
        assert_eq!(
            fs::read(fixture.directory.join(&other.name)).unwrap(),
            b"other task"
        );
    }

    #[cfg(windows)]
    #[test]
    fn locked_output_preserves_record_and_checkpoint_until_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        let task = fixture.task("locked.zip");
        let output = fixture.directory.join(&task.name);
        fs::write(&output, b"partial").unwrap();
        fs::write(fixture.directory.join("locked.zip.aria2"), b"resume").unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&output)
            .unwrap();
        assert!(remove_record(fixture.store(), &task, true).is_err());
        assert_eq!(fixture.store().tasks().unwrap().len(), 1);
        assert!(fixture.directory.join("locked.zip.aria2").is_file());
        drop(lock);
        remove_record(fixture.store(), &task, true).unwrap();
        assert!(fixture.store().tasks().unwrap().is_empty());
        assert!(!output.exists());
    }
}
