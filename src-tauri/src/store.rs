use crate::model::Task;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Store(Connection);

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").map_err(|e| e.to_string())?;
        Ok(Self(conn))
    }
    pub fn tasks(&self) -> Result<Vec<Task>, String> {
        let mut statement = self
            .0
            .prepare("SELECT payload FROM tasks ORDER BY rowid DESC")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            r.map_err(|e| e.to_string())
                .and_then(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
        })
        .collect()
    }
    pub fn task(&self, id: &str) -> Result<Option<Task>, String> {
        let payload: Option<String> = self
            .0
            .query_row("SELECT payload FROM tasks WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        payload
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|e| e.to_string())
    }
    pub fn save(&self, task: &Task) -> Result<(), String> {
        let payload = serde_json::to_string(task).map_err(|e| e.to_string())?;
        self.0.execute("INSERT INTO tasks(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![task.id,payload]).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn save_batch<'a>(&self, tasks: impl IntoIterator<Item = &'a Task>) -> Result<(), String> {
        let transaction = self.0.unchecked_transaction().map_err(|e| e.to_string())?;
        for task in tasks {
            self.save(task)?;
        }
        transaction.commit().map_err(|e| e.to_string())
    }
    pub fn remove(&self, id: &str) -> Result<(), String> {
        self.0
            .execute("DELETE FROM tasks WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn setting(&self, key: &str) -> Option<String> {
        self.0
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .ok()
    }
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn uploads(&self) -> Result<Vec<crate::upload::UploadRecord>, String> {
        self.read_records("uploads")
    }
    pub fn save_upload(&self, task: &crate::upload::UploadRecord) -> Result<(), String> {
        self.save_record("uploads", &task.id, task)
    }
    pub fn remove_upload(&self, id: &str) -> Result<(), String> {
        self.0
            .execute("DELETE FROM uploads WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn shares(&self) -> Result<Vec<crate::model::CreatedShare>, String> {
        self.read_records("shares")
    }
    pub fn save_share(&self, share: &crate::model::CreatedShare) -> Result<(), String> {
        self.save_record("shares", &share.id, share)
    }
    fn read_records<T: serde::de::DeserializeOwned>(&self, table: &str) -> Result<Vec<T>, String> {
        // Table names are internal constants, never user input.
        self.0
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {table} (id TEXT PRIMARY KEY, payload TEXT NOT NULL)"
            ))
            .map_err(|e| e.to_string())?;
        let mut stmt = self
            .0
            .prepare(&format!("SELECT payload FROM {table} ORDER BY rowid DESC"))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
    }
    fn save_record<T: serde::Serialize>(
        &self,
        table: &str,
        id: &str,
        record: &T,
    ) -> Result<(), String> {
        self.0
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {table} (id TEXT PRIMARY KEY, payload TEXT NOT NULL)"
            ))
            .map_err(|e| e.to_string())?;
        let payload = serde_json::to_string(record).map_err(|e| e.to_string())?;
        self.0.execute(&format!("INSERT INTO {table}(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload"), params![id, payload]).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reserving_a_batch_rolls_back_all_tasks_when_one_write_fails() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let task = |id: &str| -> Task {
            serde_json::from_value(json!({
                "id": id,
                "name": "sample.bin",
                "size": 4,
                "completed": 0,
                "speed": 0,
                "status": "resolving",
                "error": "",
                "directory": "C:/fixture",
                "createdAt": 0,
                "gid": null,
                "share": {"provider":"quark","id":"fixture","passcode":""},
                "file": {"id":id,"name":"sample.bin","size":4,"isDir":false,"parentId":"0"}
            }))
            .unwrap()
        };
        let mut original = task("existing");
        original.status = "paused".into();
        store.save(&original).unwrap();
        store.0.execute_batch("CREATE TRIGGER reject_fixture BEFORE INSERT ON tasks WHEN NEW.id = 'reject' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
        let batch = [task("existing"), task("new"), task("reject")];
        assert!(store.save_batch(batch.iter()).is_err());
        assert_eq!(store.tasks().unwrap().len(), 1);
        assert_eq!(store.task("existing").unwrap().unwrap().status, "paused");
        assert!(store.task("new").unwrap().is_none());
    }
}
