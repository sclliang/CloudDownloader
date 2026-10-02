use crate::model::Task;
use rusqlite::{params, Connection};
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
    pub fn save(&self, task: &Task) -> Result<(), String> {
        let payload = serde_json::to_string(task).map_err(|e| e.to_string())?;
        self.0.execute("INSERT INTO tasks(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![task.id,payload]).map_err(|e| e.to_string())?;
        Ok(())
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
