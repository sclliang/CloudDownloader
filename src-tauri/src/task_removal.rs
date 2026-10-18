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
    if task.bt.is_some() && directory.exists() {
        crate::bt::validate_output(task)?;
    }
    let directory = match fs::canonicalize(directory) {
        Ok(path) => path,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(cleanup_error(directory, error)),
    };
    let files = output_files(task, &directory)?;
    // Legacy or overlapping task records must not let one task delete another's output.
    for other in store.tasks()? {
        if other.id == task.id {
            continue;
        }
        let Ok(other_dir) = fs::canonicalize(&other.directory) else {
            continue;
        };
        for other_file in output_files(&other, &other_dir)? {
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
    for file in &files {
        match fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(cleanup_error(&file, error)),
        }
    }
    if task.bt.is_some() {
        let mut directories: Vec<_> = files
            .iter()
            .flat_map(|file| file.parent().into_iter().flat_map(|p| p.ancestors()))
            .filter(|p| p.starts_with(&directory) && *p != directory)
            .map(Path::to_path_buf)
            .collect();
        directories.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        directories.dedup();
        for dir in directories {
            let _ = fs::remove_dir(dir);
        }
        // Preserve any extra user files. Only empty directories are removed.
        let _ = fs::remove_dir(&directory);
    }
    Ok(())
}

fn output_files(task: &Task, directory: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    if task.bt.is_some() {
        let mut files: Vec<_> = crate::bt::outputs(task)?
            .iter()
            .map(|p| directory.join(p.strip_prefix(&task.directory).unwrap()))
            .collect();
        files.push(crate::bt::marker(directory));
        Ok(files)
    } else {
        Ok(vec![
            directory.join(&task.name),
            directory.join(format!("{}.aria2", task.name)),
        ])
    }
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
                bt: None,
                upload_speed: 0,
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

    fn bt_task(fixture: &Fixture) -> Task {
        let mut task = fixture.task("bundle");
        task.bt = Some(crate::model::BtSource {
            info_hash: "a".repeat(40),
            piece_length: 16384,
            files: vec![
                crate::model::BtFile {
                    index: 1,
                    path: "bundle/a.bin".into(),
                    size: 10,
                },
                crate::model::BtFile {
                    index: 2,
                    path: "bundle/sub/b.bin".into(),
                    size: 10,
                },
            ],
            selected: vec![2],
            max_peers: 64,
            upload_limit: 512,
            seed_minutes: 0,
        });
        crate::bt::mark(&task).unwrap();
        fs::create_dir_all(fixture.directory.join("bundle/sub")).unwrap();
        for name in ["bundle/a.bin", "bundle/sub/b.bin", "bundle.aria2"] {
            fs::write(fixture.directory.join(name), b"payload").unwrap();
        }
        fixture.store().save(&task).unwrap();
        task
    }

    #[test]
    fn bt_keeps_files_by_default_and_protects_overlaps_and_ownership() {
        let fixture = Fixture::new();
        let task = bt_task(&fixture);
        remove_record(fixture.store(), &task, false).unwrap();
        assert!(fixture.directory.join("bundle/sub/b.bin").is_file());
        fixture.store().save(&task).unwrap();
        let mut other = fixture.task("b.bin");
        other.directory = fixture
            .directory
            .join("bundle/sub")
            .to_string_lossy()
            .into();
        fixture.store().save(&other).unwrap();
        assert!(remove_record(fixture.store(), &task, true).is_err());
        assert!(fixture.directory.join("bundle/a.bin").is_file());
        fixture.store().remove(&other.id).unwrap();
        fs::write(crate::bt::marker(&fixture.directory), b"different owner").unwrap();
        assert!(remove_record(fixture.store(), &task, true).is_err());
        assert!(fixture.directory.join("bundle.aria2").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn bt_refuses_junction_parents_before_deleting_any_files() {
        use std::os::windows::fs::symlink_dir;
        let fixture = Fixture::new();
        let task = bt_task(&fixture);
        fs::remove_file(fixture.directory.join("bundle/sub/b.bin")).unwrap();
        fs::remove_dir(fixture.directory.join("bundle/sub")).unwrap();
        let outside = fixture.directory.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("b.bin"), b"keep").unwrap();
        let link = fixture.directory.join("bundle/sub");
        // Developer Mode is not guaranteed; create a junction via native PowerShell
        // in the integration environment if symlink creation is unavailable.
        if symlink_dir(&outside, &link).is_err() {
            use std::os::windows::process::CommandExt;
            let status = std::process::Command::new("powershell.exe")
                .creation_flags(0x08000000)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &format!(
                        "New-Item -ItemType Junction -Path '{}' -Target '{}' | Out-Null",
                        link.display(),
                        outside.display()
                    ),
                ])
                .status()
                .unwrap();
            assert!(status.success());
        }
        let result = remove_record(fixture.store(), &task, true);
        // Remove only the link itself before the fixture's ordinary teardown.
        fs::remove_dir(&link).unwrap();
        assert!(result.is_err());
        assert!(fixture.directory.join("bundle/a.bin").is_file());
        assert_eq!(fs::read(outside.join("b.bin")).unwrap(), b"keep");
    }
}
