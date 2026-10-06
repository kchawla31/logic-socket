//! SQLite-backed document store: one generic `docs` table holding typed JSON
//! documents in a parent/child tree; see docs/ARCHITECTURE.md.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::model::{Doc, Meta, Model};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{kind} {id} not found")]
    NotFound { kind: &'static str, id: String },
    #[error("document {id} is a {actual}, expected {expected}")]
    WrongType {
        id: String,
        expected: &'static str,
        actual: String,
    },
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Untyped document, used for tree listings and ancestor walks.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RawDoc {
    #[serde(flatten)]
    pub meta: Meta,
    #[serde(flatten)]
    pub data: Value,
}

impl RawDoc {
    pub fn typed<T: Model>(&self) -> Result<Doc<T>> {
        if self.meta.kind != T::TYPE {
            return Err(StoreError::WrongType {
                id: self.meta.id.clone(),
                expected: T::TYPE,
                actual: self.meta.kind.clone(),
            });
        }
        Ok(Doc {
            meta: self.meta.clone(),
            body: serde_json::from_value(self.data.clone())?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "op")]
pub enum Change {
    Upsert {
        id: String,
        kind: String,
        parent_id: Option<String>,
    },
    Delete {
        id: String,
        kind: String,
    },
}

type Listener = Box<dyn Fn(&[Change]) + Send + Sync>;

const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE docs (
  id        TEXT PRIMARY KEY,
  type      TEXT NOT NULL,
  parent_id TEXT,
  sort_key  REAL NOT NULL DEFAULT 0,
  created   INTEGER NOT NULL,
  modified  INTEGER NOT NULL,
  data      TEXT NOT NULL
);
CREATE INDEX docs_parent ON docs(parent_id, type);
CREATE INDEX docs_type ON docs(type);
"#];

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    conn: Mutex<Connection>,
    listeners: Mutex<Vec<Listener>>,
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(dir) = path.as_ref().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);",
        )?;
        let current: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [i as i64 + 1],
            )?;
        }
        Ok(Self {
            inner: Arc::new(Inner {
                conn: Mutex::new(conn),
                listeners: Mutex::new(vec![]),
            }),
        })
    }

    /// Register a listener called once per committed write (or batch).
    pub fn subscribe(&self, f: impl Fn(&[Change]) + Send + Sync + 'static) {
        self.inner.listeners.lock().unwrap().push(Box::new(f));
    }

    fn emit(&self, changes: &[Change]) {
        if changes.is_empty() {
            return;
        }
        for l in self.inner.listeners.lock().unwrap().iter() {
            l(changes);
        }
    }

    /// Run several writes in one transaction with one change notification.
    pub fn batch<R>(&self, f: impl FnOnce(&mut Tx<'_>) -> Result<R>) -> Result<R> {
        let (out, changes) = {
            let mut conn = self.inner.conn.lock().unwrap();
            let txn = conn.transaction()?;
            let mut tx = Tx {
                conn: &txn,
                changes: vec![],
            };
            let out = f(&mut tx)?;
            let changes = std::mem::take(&mut tx.changes);
            txn.commit()?;
            (out, changes)
        };
        self.emit(&changes);
        Ok(out)
    }

    fn read<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let conn = self.inner.conn.lock().unwrap();
        f(&conn)
    }

    pub fn insert<T: Model>(&self, parent_id: Option<&str>, body: T) -> Result<Doc<T>> {
        self.batch(|tx| tx.insert(parent_id, body))
    }

    pub fn update<T: Model>(&self, doc: &Doc<T>) -> Result<Doc<T>> {
        self.batch(|tx| tx.update(doc))
    }

    pub fn delete(&self, id: &str) -> Result<usize> {
        self.batch(|tx| tx.delete(id))
    }

    pub fn get<T: Model>(&self, id: &str) -> Result<Doc<T>> {
        self.raw(id)?
            .ok_or_else(|| StoreError::NotFound {
                kind: T::TYPE,
                id: id.to_string(),
            })?
            .typed()
    }

    pub fn raw(&self, id: &str) -> Result<Option<RawDoc>> {
        self.read(|c| raw(c, id))
    }

    pub fn children<T: Model>(&self, parent_id: &str) -> Result<Vec<Doc<T>>> {
        self.read(|c| {
            query(
                c,
                "WHERE parent_id = ?1 AND type = ?2",
                params![parent_id, T::TYPE],
            )
        })?
        .iter()
        .map(RawDoc::typed)
        .collect()
    }

    pub fn all_children(&self, parent_id: &str) -> Result<Vec<RawDoc>> {
        self.read(|c| query(c, "WHERE parent_id = ?1", params![parent_id]))
    }

    pub fn all_of<T: Model>(&self) -> Result<Vec<Doc<T>>> {
        self.read(|c| query(c, "WHERE type = ?1", params![T::TYPE]))?
            .iter()
            .map(RawDoc::typed)
            .collect()
    }

    /// Every descendant of `root_id` (not including the root), breadth-first.
    pub fn descendants(&self, root_id: &str) -> Result<Vec<RawDoc>> {
        self.read(|c| descendants(c, root_id))
    }

    /// Ancestors of `id`, nearest first (e.g. folder, parent folder, workspace).
    pub fn ancestors(&self, id: &str) -> Result<Vec<RawDoc>> {
        self.read(|c| {
            let mut out = vec![];
            let mut cur = raw(c, id)?.and_then(|d| d.meta.parent_id);
            while let Some(pid) = cur {
                match raw(c, &pid)? {
                    Some(d) => {
                        cur = d.meta.parent_id.clone();
                        out.push(d);
                    }
                    None => break,
                }
                if out.len() > 64 {
                    break; // cycle guard
                }
            }
            Ok(out)
        })
    }

    /// Get the singleton settings doc, creating it on first use.
    pub fn settings(&self) -> Result<Doc<crate::model::Settings>> {
        if let Some(s) = self.all_of::<crate::model::Settings>()?.into_iter().next() {
            return Ok(s);
        }
        self.insert(None, crate::model::Settings::default())
    }
}

/// Write handle inside a batch transaction.
pub struct Tx<'a> {
    conn: &'a Connection,
    changes: Vec<Change>,
}

impl Tx<'_> {
    pub fn insert<T: Model>(&mut self, parent_id: Option<&str>, body: T) -> Result<Doc<T>> {
        let now = now_ms();
        let meta = Meta {
            id: new_id(T::PREFIX),
            kind: T::TYPE.to_string(),
            parent_id: parent_id.map(str::to_string),
            sort_key: next_sort_key(self.conn, parent_id)?,
            created: now,
            modified: now,
        };
        let doc = Doc { meta, body };
        self.write(&doc.meta, &serde_json::to_value(&doc.body)?)?;
        Ok(doc)
    }

    /// Insert with a caller-chosen id (used by importers and tests).
    pub fn insert_with_id<T: Model>(
        &mut self,
        id: &str,
        parent_id: Option<&str>,
        body: T,
    ) -> Result<Doc<T>> {
        let now = now_ms();
        let meta = Meta {
            id: id.to_string(),
            kind: T::TYPE.to_string(),
            parent_id: parent_id.map(str::to_string),
            sort_key: next_sort_key(self.conn, parent_id)?,
            created: now,
            modified: now,
        };
        let doc = Doc { meta, body };
        self.write(&doc.meta, &serde_json::to_value(&doc.body)?)?;
        Ok(doc)
    }

    /// Write a document exactly as given (id, parent, sort key, timestamps), creating
    /// or overwriting it. Used by importers that keep original identities.
    pub fn put<T: Model>(&mut self, doc: &Doc<T>) -> Result<()> {
        let mut meta = doc.meta.clone();
        meta.kind = T::TYPE.to_string();
        self.write(&meta, &serde_json::to_value(&doc.body)?)
    }

    pub fn update<T: Model>(&mut self, doc: &Doc<T>) -> Result<Doc<T>> {
        let mut doc = doc.clone();
        doc.meta.kind = T::TYPE.to_string();
        doc.meta.modified = now_ms();
        self.write(&doc.meta, &serde_json::to_value(&doc.body)?)?;
        Ok(doc)
    }

    /// Write an untyped document back (e.g. a rename from the UI).
    pub fn update_raw(&mut self, doc: &RawDoc) -> Result<()> {
        let mut meta = doc.meta.clone();
        meta.modified = now_ms();
        self.write(&meta, &doc.data)
    }

    /// Move a document under a new parent and/or position.
    pub fn move_to(&mut self, id: &str, parent_id: Option<&str>, sort_key: f64) -> Result<()> {
        let Some(mut d) = raw(self.conn, id)? else {
            return Err(StoreError::NotFound {
                kind: "Document",
                id: id.into(),
            });
        };
        d.meta.parent_id = parent_id.map(str::to_string);
        d.meta.sort_key = sort_key;
        d.meta.modified = now_ms();
        self.write(&d.meta, &d.data)
    }

    /// Delete a document and all its descendants. Returns the number removed.
    pub fn delete(&mut self, id: &str) -> Result<usize> {
        let Some(root) = raw(self.conn, id)? else {
            return Ok(0);
        };
        let mut all = descendants(self.conn, id)?;
        all.push(root);
        for d in &all {
            self.conn
                .execute("DELETE FROM docs WHERE id = ?1", [&d.meta.id])?;
            self.changes.push(Change::Delete {
                id: d.meta.id.clone(),
                kind: d.meta.kind.clone(),
            });
        }
        Ok(all.len())
    }

    pub fn get<T: Model>(&self, id: &str) -> Result<Doc<T>> {
        raw(self.conn, id)?
            .ok_or_else(|| StoreError::NotFound {
                kind: T::TYPE,
                id: id.to_string(),
            })?
            .typed()
    }

    pub fn children<T: Model>(&self, parent_id: &str) -> Result<Vec<Doc<T>>> {
        query(
            self.conn,
            "WHERE parent_id = ?1 AND type = ?2",
            params![parent_id, T::TYPE],
        )?
        .iter()
        .map(RawDoc::typed)
        .collect()
    }

    fn write(&mut self, meta: &Meta, data: &Value) -> Result<()> {
        self.conn.execute(
            "INSERT INTO docs (id, type, parent_id, sort_key, created, modified, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET type = excluded.type, parent_id = excluded.parent_id,
               sort_key = excluded.sort_key, modified = excluded.modified, data = excluded.data",
            params![
                meta.id,
                meta.kind,
                meta.parent_id,
                meta.sort_key,
                meta.created,
                meta.modified,
                data.to_string()
            ],
        )?;
        self.changes.push(Change::Upsert {
            id: meta.id.clone(),
            kind: meta.kind.clone(),
            parent_id: meta.parent_id.clone(),
        });
        Ok(())
    }
}

fn next_sort_key(c: &Connection, parent_id: Option<&str>) -> Result<f64> {
    let max: Option<f64> = c
        .query_row(
            "SELECT MAX(sort_key) FROM docs WHERE parent_id IS ?1",
            [parent_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(max.map(|m| m + 1.0).unwrap_or(0.0))
}

fn row_to_raw(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Meta, String)> {
    Ok((
        Meta {
            id: r.get(0)?,
            kind: r.get(1)?,
            parent_id: r.get(2)?,
            sort_key: r.get(3)?,
            created: r.get(4)?,
            modified: r.get(5)?,
        },
        r.get(6)?,
    ))
}

const COLS: &str = "SELECT id, type, parent_id, sort_key, created, modified, data FROM docs ";

fn raw(c: &Connection, id: &str) -> Result<Option<RawDoc>> {
    let row = c
        .query_row(&format!("{COLS} WHERE id = ?1"), [id], row_to_raw)
        .optional()?;
    row.map(|(meta, data)| {
        Ok(RawDoc {
            meta,
            data: serde_json::from_str(&data)?,
        })
    })
    .transpose()
}

fn query(c: &Connection, where_: &str, p: impl rusqlite::Params) -> Result<Vec<RawDoc>> {
    let mut stmt = c.prepare(&format!("{COLS} {where_} ORDER BY sort_key, created"))?;
    let rows = stmt.query_map(p, row_to_raw)?;
    rows.map(|r| {
        let (meta, data) = r?;
        Ok(RawDoc {
            meta,
            data: serde_json::from_str(&data)?,
        })
    })
    .collect()
}

fn descendants(c: &Connection, root_id: &str) -> Result<Vec<RawDoc>> {
    let mut out = vec![];
    let mut frontier = vec![root_id.to_string()];
    while let Some(id) = frontier.pop() {
        for d in query(c, "WHERE parent_id = ?1", params![id])? {
            frontier.push(d.meta.id.clone());
            out.push(d);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ws(store: &Store) -> Doc<Workspace> {
        store
            .insert(
                None,
                Workspace {
                    name: "W".into(),
                    ..Default::default()
                },
            )
            .unwrap()
    }

    #[test]
    fn crud_and_hierarchy() {
        let s = Store::open_in_memory().unwrap();
        let w = ws(&s);
        let f = s
            .insert(
                Some(w.id()),
                Folder {
                    name: "F".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut r = s
            .insert(
                Some(f.id()),
                Request {
                    url: "http://x".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        r.method = "POST".into();
        s.update(&r).unwrap();

        let back: Doc<Request> = s.get(r.id()).unwrap();
        assert_eq!(back.method, "POST");
        let anc = s.ancestors(r.id()).unwrap();
        assert_eq!(
            anc.iter().map(|d| d.meta.kind.as_str()).collect::<Vec<_>>(),
            ["Folder", "Workspace"]
        );
        assert_eq!(s.descendants(w.id()).unwrap().len(), 2);

        assert_eq!(s.delete(w.id()).unwrap(), 3);
        assert!(s.raw(r.id()).unwrap().is_none());
    }

    #[test]
    fn wrong_type_is_an_error() {
        let s = Store::open_in_memory().unwrap();
        let w = ws(&s);
        assert!(matches!(
            s.get::<Request>(w.id()),
            Err(StoreError::WrongType { .. })
        ));
    }

    #[test]
    fn batch_emits_one_notification_and_rolls_back_on_error() {
        let s = Store::open_in_memory().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        s.subscribe(move |ch| {
            assert!(!ch.is_empty());
            c2.fetch_add(1, Ordering::SeqCst);
        });
        let w = ws(&s);
        s.batch(|tx| {
            for i in 0..50 {
                tx.insert(
                    Some(w.id()),
                    Request {
                        name: format!("r{i}"),
                        ..Default::default()
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2); // workspace insert + batch

        let err = s.batch(|tx| {
            tx.insert(Some(w.id()), Request::default())?;
            Err::<(), _>(StoreError::NotFound {
                kind: "x",
                id: "y".into(),
            })
        });
        assert!(err.is_err());
        assert_eq!(s.children::<Request>(w.id()).unwrap().len(), 50);
    }

    #[test]
    fn children_sorted_by_insert_order() {
        let s = Store::open_in_memory().unwrap();
        let w = ws(&s);
        for n in ["a", "b", "c"] {
            s.insert(
                Some(w.id()),
                Request {
                    name: n.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let names: Vec<_> = s
            .children::<Request>(w.id())
            .unwrap()
            .into_iter()
            .map(|r| r.body.name)
            .collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn reopening_file_db_keeps_data_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let id = {
            let s = Store::open(&path).unwrap();
            ws(&s).meta.id
        };
        let s = Store::open(&path).unwrap();
        assert_eq!(s.get::<Workspace>(&id).unwrap().name, "W");
    }
}
