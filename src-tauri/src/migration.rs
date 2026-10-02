use rusqlite::{Connection, OpenFlags};
use std::{fs, path::Path};

/// Publish a complete migration atomically. The legacy directory is never modified.
pub fn migrate(new_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if new_dir.exists() {
        return Ok(());
    }
    let parent = new_dir.parent().ok_or("应用数据目录无效")?;
    let legacy = parent.join("com.sixunited.quarkdownloader");
    if !legacy.join("tasks.sqlite3").is_file() {
        return Ok(());
    }
    let staging = parent.join(format!(
        ".clouddownloader-migration-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&staging)?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let source = Connection::open_with_flags(
            legacy.join("tasks.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        source.backup(
            rusqlite::DatabaseName::Main,
            staging.join("tasks.sqlite3"),
            None,
        )?;
        let profile = legacy.join("quark-profile");
        if profile.is_dir() {
            copy_profile(&profile, &staging.join("quark-profile"))?;
        }
        fs::rename(&staging, new_dir)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn copy_profile(source: &Path, dest: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let skip = matches!(
            name.to_str(),
            Some(
                "Cache"
                    | "Code Cache"
                    | "GPUCache"
                    | "ShaderCache"
                    | "Crashpad"
                    | "SingletonLock"
                    | "SingletonCookie"
                    | "SingletonSocket"
                    | "lockfile"
                    | "LOCK"
            )
        );
        if skip || entry.file_type()?.is_symlink() {
            continue;
        }
        if entry.file_type()?.is_dir() {
            copy_profile(&entry.path(), &dest.join(name))?;
        } else {
            fs::copy(entry.path(), dest.join(name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_wal_settings_profile_and_legacy_without_overwriting() {
        let root =
            std::env::temp_dir().join(format!("cloud-migration-test-{}", uuid::Uuid::new_v4()));
        let old = root.join("com.sixunited.quarkdownloader");
        let new = root.join("com.sixunited.clouddownloader");
        fs::create_dir_all(old.join("quark-profile/Default/Local Storage")).unwrap();
        fs::write(
            old.join("quark-profile/Default/Local Storage/sample"),
            b"profile-fixture",
        )
        .unwrap();
        let db = Connection::open(old.join("tasks.sqlite3")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT); INSERT INTO settings VALUES ('download_dir','D:/existing');").unwrap();
        migrate(&new).unwrap();
        let check = Connection::open(new.join("tasks.sqlite3")).unwrap();
        let value: String = check
            .query_row("SELECT value FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "D:/existing");
        assert_eq!(
            fs::read(new.join("quark-profile/Default/Local Storage/sample")).unwrap(),
            b"profile-fixture"
        );
        check
            .execute("UPDATE settings SET value='new'", [])
            .unwrap();
        migrate(&new).unwrap();
        assert_eq!(
            check
                .query_row::<String, _, _>("SELECT value FROM settings", [], |r| r.get(0))
                .unwrap(),
            "new"
        );
        assert_eq!(
            db.query_row::<String, _, _>("SELECT value FROM settings", [], |r| r.get(0))
                .unwrap(),
            "D:/existing"
        );
        drop(check);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }
}
