//! Optional native SQLite download queue and local-library records. No filesystem deletion.
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("invalid library input")]
    Invalid,
    #[error("library record not found")]
    NotFound,
    #[error("library operation failed")]
    Storage,
}
type Result<T> = std::result::Result<T, LibraryError>;
impl From<rusqlite::Error> for LibraryError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DownloadTask {
    pub id: String,
    pub wall: Value,
    pub state: String,
    pub received: u64,
    pub total: Option<u64>,
    pub uri: Option<String>,
    pub created: u64,
    pub updated: u64,
    pub error: Option<String>,
}
pub struct Library {
    connection: Mutex<Connection>,
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub fn valid_id(id: &str) -> bool {
    id.len() == 6 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}
fn clean(value: &Value) -> bool {
    match value {
        Value::Object(o) => o.iter().all(|(k, v)| {
            ![
                "apikey",
                "api_key",
                "cookie",
                "cookies",
                "authorization",
                "password",
                "token",
            ]
            .contains(&k.to_ascii_lowercase().as_str())
                && clean(v)
        }),
        Value::Array(a) => a.iter().all(clean),
        Value::String(s) if s.starts_with("http://") || s.starts_with("https://") => {
            reqwest::Url::parse(s).is_ok_and(|u| {
                u.username().is_empty() && u.password().is_none() && u.query().is_none()
            })
        }
        _ => true,
    }
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadTask> {
    let wall: String = r.get(1)?;
    let uint = |index| -> rusqlite::Result<u64> {
        let n: i64 = r.get(index)?;
        u64::try_from(n).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, n))
    };
    let total: Option<i64> = r.get(4)?;
    Ok(DownloadTask {
        id: r.get(0)?,
        wall: serde_json::from_str(&wall).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?,
        state: r.get(2)?,
        received: uint(3)?,
        total: total
            .map(|n| u64::try_from(n).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(4, n)))
            .transpose()?,
        uri: r.get(5)?,
        created: uint(6)?,
        updated: uint(7)?,
        error: r.get(8)?,
    })
}
const COLUMNS: &str = "id,wall,state,received,total,uri,created,updated,error";
impl Library {
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir).map_err(|_| LibraryError::Storage)?;
        let connection = Connection::open(data_dir.join("library-v1.sqlite3"))?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 1 {
            return Err(LibraryError::Storage);
        }
        connection.execute_batch(
            "BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY NOT NULL, wall TEXT NOT NULL, state TEXT NOT NULL,
                received INTEGER NOT NULL DEFAULT 0, total INTEGER, uri TEXT,
                created INTEGER NOT NULL, updated INTEGER NOT NULL, error TEXT);
            CREATE TABLE IF NOT EXISTS library (
                id TEXT PRIMARY KEY NOT NULL, wall TEXT NOT NULL, state TEXT NOT NULL,
                received INTEGER NOT NULL DEFAULT 0, total INTEGER, uri TEXT,
                created INTEGER NOT NULL, updated INTEGER NOT NULL, error TEXT);
            PRAGMA user_version=1; COMMIT;",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
    fn db(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| LibraryError::Storage)
    }
    pub fn get(&self, id: &str) -> Result<DownloadTask> {
        self.db()?
            .query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE id=?1"),
                [id],
                row,
            )
            .optional()?
            .ok_or(LibraryError::NotFound)
    }
    pub fn enqueue(&self, wall: Value) -> Result<DownloadTask> {
        let id = wall
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| valid_id(s))
            .ok_or(LibraryError::Invalid)?
            .to_owned();
        if !wall.is_object() || !clean(&wall) {
            return Err(LibraryError::Invalid);
        }
        let wall = serde_json::to_string(&wall).map_err(|_| LibraryError::Invalid)?;
        let mut db = self.db()?;
        let tx = db.transaction()?;
        // A queue-only removal must not cause a second export of a retained library item.
        tx.execute(
            &format!(
                "INSERT OR IGNORE INTO tasks ({COLUMNS}) SELECT {COLUMNS} FROM library WHERE id=?1"
            ),
            [&id],
        )?;
        tx.execute(
            "INSERT INTO tasks (id,wall,state,created,updated) VALUES (?1,?2,'queued',?3,?3)
            ON CONFLICT(id) DO UPDATE SET state='queued',error=NULL,updated=excluded.updated
            WHERE tasks.state IN ('cancelled','failed','missing')",
            params![id, wall, now()],
        )?;
        let task = tx.query_row(
            &format!("SELECT {COLUMNS} FROM tasks WHERE id=?1"),
            [&id],
            row,
        )?;
        tx.commit()?;
        Ok(task)
    }
    pub fn tasks(&self) -> Result<Vec<DownloadTask>> {
        self.list(false)
    }
    pub fn library(&self) -> Result<Vec<DownloadTask>> {
        self.list(true)
    }
    fn list(&self, library: bool) -> Result<Vec<DownloadTask>> {
        let db = self.db()?;
        let mut stmt = db.prepare(&format!(
            "SELECT {COLUMNS} FROM {} ORDER BY created,id",
            if library { "library" } else { "tasks" }
        ))?;
        let rows = stmt
            .query_map([], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn set_state(&self, id: &str, state: &str) -> Result<()> {
        if !["queued", "paused", "cancelled"].contains(&state) {
            return Err(LibraryError::Invalid);
        }
        let db = self.db()?;
        let n = db.execute("UPDATE tasks SET state=?2,updated=?3,error=NULL WHERE id=?1 AND state!='completed' AND (state!='missing' OR ?2='queued')", params![id, state, now()])?;
        if n == 0
            && !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Err(LibraryError::NotFound);
        }
        Ok(())
    }
    /// Atomic worker claim: constructing another Library never resets live workers.
    pub fn claim(&self, id: &str) -> Result<bool> {
        Ok(self.db()?.execute(
            "UPDATE tasks SET state='running',updated=?2,error=NULL WHERE id=?1 AND state='queued'",
            params![id, now()],
        )? == 1)
    }
    pub fn progress(&self, id: &str, received: u64, total: Option<u64>) -> Result<()> {
        if received > i64::MAX as u64 || total.is_some_and(|n| n > i64::MAX as u64) {
            return Err(LibraryError::Invalid);
        }
        self.db()?.execute(
            "UPDATE tasks SET received=?2,total=?3,updated=?4 WHERE id=?1 AND state IN ('running','paused','cancelled')",
            params![id, received as i64, total.map(|n| n as i64), now()],
        )?;
        Ok(())
    }
    pub fn pause(&self, id: &str) -> Result<()> {
        self.db()?.execute(
            "UPDATE tasks SET state='paused',updated=?2 WHERE id=?1 AND state='running'",
            params![id, now()],
        )?;
        Ok(())
    }
    pub fn recover(&self) -> Result<()> {
        self.db()?.execute(
            "UPDATE tasks SET state='queued',updated=?1 WHERE state='running'",
            [now()],
        )?;
        Ok(())
    }
    pub fn complete(&self, id: &str, uri: &str) -> Result<()> {
        let parsed = reqwest::Url::parse(uri).map_err(|_| LibraryError::Invalid)?;
        if uri.chars().any(char::is_control)
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !(parsed.scheme() == "content"
                && parsed.host_str().is_some()
                && parsed.path() != "/"
                && parsed.path().starts_with('/')
                && parsed.path().len() > 1
                || parsed.scheme() == "file"
                    && parsed.host_str().is_none()
                    && parsed.path().starts_with('/')
                    && parsed.path().len() > 1)
        {
            return Err(LibraryError::Invalid);
        }
        let mut db = self.db()?;
        let tx = db.transaction()?;
        if tx.execute("UPDATE tasks SET state='completed',uri=?2,error=NULL,updated=CASE WHEN state='completed' AND uri=?2 THEN updated ELSE ?3 END WHERE id=?1", params![id, uri, now()])? == 0 { return Err(LibraryError::NotFound); }
        tx.execute(&format!("INSERT OR REPLACE INTO library ({COLUMNS}) SELECT {COLUMNS} FROM tasks WHERE id=?1"), [id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn fail(&self, id: &str) -> Result<()> {
        self.db()?.execute("UPDATE tasks SET state='failed',error='download failed',updated=?2 WHERE id=?1 AND state IN ('queued','running')", params![id, now()])?;
        Ok(())
    }
    pub fn mark_missing(&self, id: &str) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        for table in ["tasks", "library"] {
            tx.execute(&format!("UPDATE {table} SET state='missing',updated=?2 WHERE id=?1 AND state IN ('completed','missing')"), params![id, now()])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn remove(&self, id: &str, remove_library: bool) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        tx.execute("DELETE FROM tasks WHERE id=?1", [id])?;
        if remove_library {
            tx.execute("DELETE FROM library WHERE id=?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn wall() -> Value {
        serde_json::json!({"id":"abc123","path":"https://w.wallhaven.cc/full/ab/wallhaven-abc123.png","resolution":"1x1","tags":[{"name":"test"}]})
    }
    #[test]
    fn durable_dedup_recover_idempotence_and_record_only_remove() {
        let dir = tempfile::tempdir().unwrap();
        let db = Library::open(dir.path()).unwrap();
        let first = db.enqueue(wall()).unwrap();
        assert_eq!(db.enqueue(wall()).unwrap(), first);
        assert!(db.claim("abc123").unwrap());
        assert!(!db.claim("abc123").unwrap());
        let other = Library::open(dir.path()).unwrap();
        assert_eq!(other.get("abc123").unwrap().state, "running");
        db.progress("abc123", 12, Some(20)).unwrap();
        drop(db);
        other.recover().unwrap();
        assert_eq!(other.get("abc123").unwrap().state, "queued");
        let file = dir.path().join("export.png");
        std::fs::write(&file, b"keep").unwrap();
        other
            .complete("abc123", "content://media/external/images/123")
            .unwrap();
        let completed = other.get("abc123").unwrap();
        other
            .complete("abc123", "content://media/external/images/123")
            .unwrap();
        assert_eq!(other.enqueue(wall()).unwrap(), completed);
        assert_eq!(other.tasks().unwrap().len(), 1);
        assert_eq!(other.library().unwrap().len(), 1);
        other.mark_missing("abc123").unwrap();
        other.remove("abc123", false).unwrap();
        assert!(other.tasks().unwrap().is_empty());
        assert_eq!(other.library().unwrap()[0].state, "missing");
        other.remove("abc123", true).unwrap();
        assert!(other.library().unwrap().is_empty());
        assert_eq!(std::fs::read(file).unwrap(), b"keep");
        let conn = Connection::open(dir.path().join("library-v1.sqlite3")).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn rejects_invalid_inputs_and_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let db = Library::open(dir.path()).unwrap();
        db.enqueue(wall()).unwrap();
        for uri in [
            "",
            "/private/image",
            "https://example.org/image",
            "file://remote/a",
            "content:///a",
            "content://media/",
            "content://media",
            "content://media/a?apikey=secret",
            "file:///a\n",
        ] {
            assert!(db.complete("abc123", uri).is_err(), "{uri}");
        }
        db.complete("abc123", "file:///data/user/0/app/files/a.png")
            .unwrap();
        assert!(db.set_state("abc123", "running").is_err());
        for w in [
            serde_json::json!({"id":"../bad"}),
            serde_json::json!({"id":"abc123","apiKey":"secret"}),
            serde_json::json!({"id":"abc123","path":"https://x/a?apikey=secret"}),
        ] {
            assert!(db.enqueue(w).is_err());
        }
    }
    #[test]
    fn retry_cancelled_failed_and_retained_library_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let db = Library::open(dir.path()).unwrap();
        let first = db.enqueue(wall()).unwrap();
        for state in ["cancelled", "failed"] {
            if state == "cancelled" {
                db.set_state("abc123", state).unwrap();
            } else {
                db.fail("abc123").unwrap();
            }
            assert_eq!(db.enqueue(wall()).unwrap().state, "queued");
            assert_eq!(db.get("abc123").unwrap().created, first.created);
        }
        db.complete("abc123", "content://media/images/1").unwrap();
        let saved = db.get("abc123").unwrap();
        db.remove("abc123", false).unwrap();
        assert_eq!(db.enqueue(wall()).unwrap(), saved);
        assert_eq!(db.library().unwrap().len(), 1);
    }
    #[test]
    fn missing_original_can_requeue_without_losing_library_identity() {
        let dir = tempfile::tempdir().unwrap();
        let db = Library::open(dir.path()).unwrap();
        db.enqueue(wall()).unwrap();
        db.complete("abc123", "content://media/images/1").unwrap();
        db.mark_missing("abc123").unwrap();
        db.set_state("abc123", "queued").unwrap();
        assert_eq!(db.get("abc123").unwrap().state, "queued");
        assert_eq!(db.library().unwrap()[0].state, "missing");
        db.complete("abc123", "content://media/images/1").unwrap();
        db.mark_missing("abc123").unwrap();
        assert_eq!(db.enqueue(wall()).unwrap().state, "queued");
        db.complete("abc123", "content://media/images/2").unwrap();
        assert_eq!(
            db.library().unwrap()[0].uri.as_deref(),
            Some("content://media/images/2")
        );
    }
}
