use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use uuid::Uuid;

pub struct Database {
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversion {
    pub id: Uuid,
    pub source_name: String,
    pub source_path: Option<String>,
    pub detected_format: Option<String>,
    pub output_format: String,
    pub engine: Option<String>,
    pub status: String,
    pub output_path: Option<String>,
    pub source_size: Option<u64>,
    pub output_size: Option<u64>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub struct NewConversion<'a> {
    pub id: Uuid,
    pub source_name: &'a str,
    pub source_path: &'a Path,
    pub detected_format: &'a str,
    pub output_format: &'a str,
    pub engine: &'a str,
    pub source_size: u64,
}

pub struct StoredFile {
    pub source_name: String,
    pub bytes: Vec<u8>,
}

struct StoredFileRow {
    source_name: String,
    data: Option<Vec<u8>>,
    compression: Option<String>,
    original_size: Option<i64>,
    legacy_base64: Option<String>,
}

const CONVERSION_COLUMNS: &str = "id, source_name, source_path, detected_format,
    output_format, engine, status, output_path, source_size, output_size, error,
    created_at, completed_at";

impl Database {
    pub fn migrate_if_missing(source: &Path, destination: &Path) -> Result<bool> {
        if destination.exists() || !source.is_file() {
            return Ok(false);
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("could not create application home at {}", parent.display())
            })?;
        }
        let connection = Connection::open(source)
            .with_context(|| format!("could not open legacy database at {}", source.display()))?;
        if let Err(error) =
            connection.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])
        {
            let _ = std::fs::remove_file(destination);
            return Err(error).with_context(|| {
                format!(
                    "could not migrate database from {} to {}",
                    source.display(),
                    destination.display()
                )
            });
        }
        Ok(true)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)
            .with_context(|| format!("could not open local database at {}", path.display()))?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS conversions (
                id TEXT PRIMARY KEY,
                source_name TEXT NOT NULL,
                source_path TEXT,
                detected_format TEXT,
                output_format TEXT NOT NULL,
                engine TEXT,
                status TEXT NOT NULL,
                output_path TEXT,
                source_size INTEGER,
                output_size INTEGER,
                error TEXT,
                created_at TEXT NOT NULL,
                completed_at TEXT,
                output_data BLOB,
                compression TEXT,
                original_size INTEGER,
                stored_size INTEGER,
                output_base64 TEXT
             );",
        )?;
        for (column, definition) in [
            ("source_path", "TEXT"),
            ("detected_format", "TEXT"),
            ("engine", "TEXT"),
            ("source_size", "INTEGER"),
            ("output_size", "INTEGER"),
            ("completed_at", "TEXT"),
            ("output_data", "BLOB"),
            ("compression", "TEXT"),
            ("original_size", "INTEGER"),
            ("stored_size", "INTEGER"),
            ("output_base64", "TEXT"),
        ] {
            add_column_if_missing(&connection, "conversions", column, definition)?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn fail_interrupted_conversions(&self) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.lock()?.execute(
            "UPDATE conversions
             SET status = 'failed', error = 'Conversion was interrupted before it completed.',
                 completed_at = ?1
             WHERE status IN ('processing', 'initializing', 'converting')",
            [&now],
        )?;
        Ok(())
    }

    pub fn insert_conversion(&self, input: NewConversion<'_>) -> Result<Conversion> {
        let created_at = Utc::now();
        self.lock()?.execute(
            "INSERT INTO conversions
             (id, source_name, source_path, detected_format, output_format, engine,
              status, source_size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'processing', ?7, ?8)",
            params![
                input.id.to_string(),
                input.source_name,
                input.source_path.to_string_lossy(),
                input.detected_format,
                input.output_format,
                input.engine,
                i64::try_from(input.source_size).context("source file is too large")?,
                created_at.to_rfc3339(),
            ],
        )?;
        self.get_conversion(input.id)
    }

    pub fn get_conversion(&self, id: Uuid) -> Result<Conversion> {
        let connection = self.lock()?;
        query_conversion(&connection, id)?.context("Conversion was not found.")
    }

    pub fn list_conversions(&self) -> Result<Vec<Conversion>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {CONVERSION_COLUMNS} FROM conversions ORDER BY created_at DESC"
        ))?;
        let rows = statement.query_map([], conversion_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn mark_finished(
        &self,
        id: Uuid,
        output_path: &Path,
        engine: &str,
        output_size: u64,
    ) -> Result<Conversion> {
        let completed_at = Utc::now();
        self.lock()?.execute(
            "UPDATE conversions
             SET status = 'finished', output_path = ?2, engine = ?3, output_size = ?4,
                 error = NULL, completed_at = ?5
             WHERE id = ?1",
            params![
                id.to_string(),
                output_path.to_string_lossy(),
                engine,
                i64::try_from(output_size).context("output file is too large")?,
                completed_at.to_rfc3339(),
            ],
        )?;
        self.get_conversion(id)
    }

    pub fn mark_failed(&self, id: Uuid, error: &str) -> Result<Conversion> {
        self.mark_terminal(id, "failed", error)
    }

    pub fn mark_cancelled(&self, id: Uuid) -> Result<Conversion> {
        self.mark_terminal(id, "cancelled", "Conversion was cancelled.")
    }

    fn mark_terminal(&self, id: Uuid, status: &str, error: &str) -> Result<Conversion> {
        self.lock()?.execute(
            "UPDATE conversions SET status = ?2, error = ?3, completed_at = ?4 WHERE id = ?1",
            params![id.to_string(), status, error, Utc::now().to_rfc3339()],
        )?;
        self.get_conversion(id)
    }

    pub fn update_output_path(&self, id: Uuid, output_path: &Path) -> Result<()> {
        self.lock()?.execute(
            "UPDATE conversions SET output_path = ?2 WHERE id = ?1",
            params![id.to_string(), output_path.to_string_lossy()],
        )?;
        Ok(())
    }

    pub fn has_stored_file(&self, id: Uuid) -> Result<bool> {
        self.lock()?
            .query_row(
                "SELECT output_data IS NOT NULL
                        OR (output_base64 IS NOT NULL AND output_base64 <> '')
                 FROM conversions WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.unwrap_or(false))
            .map_err(Into::into)
    }

    pub fn stored_file(&self, id: Uuid) -> Result<StoredFile> {
        let stored: Option<StoredFileRow> = self
            .lock()?
            .query_row(
                "SELECT source_name, output_data, compression, original_size, output_base64
                 FROM conversions WHERE id = ?1",
                [id.to_string()],
                |row| {
                    Ok(StoredFileRow {
                        source_name: row.get(0)?,
                        data: row.get(1)?,
                        compression: row.get(2)?,
                        original_size: row.get(3)?,
                        legacy_base64: row.get(4)?,
                    })
                },
            )
            .optional()?;
        let stored = stored.context("Conversion was not found.")?;
        let bytes = if let Some(data) = stored.data {
            decode_stored_file(
                &data,
                stored.compression.as_deref().unwrap_or("none"),
                stored.original_size,
            )?
        } else {
            let encoded = stored
                .legacy_base64
                .filter(|value| !value.is_empty())
                .context("This conversion does not have a saved database copy.")?;
            STANDARD
                .decode(encoded)
                .context("The saved database copy is damaged and could not be decoded.")?
        };
        Ok(StoredFile {
            source_name: stored.source_name,
            bytes,
        })
    }

    pub fn delete_conversion(&self, id: Uuid) -> Result<Option<String>> {
        let connection = self.lock()?;
        let path: Option<Option<String>> = connection
            .query_row(
                "SELECT output_path FROM conversions WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        connection.execute("DELETE FROM conversions WHERE id = ?1", [id.to_string()])?;
        Ok(path.flatten())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| anyhow::anyhow!("local database lock was poisoned"))
    }
}

fn decode_stored_file(
    data: &[u8],
    compression: &str,
    original_size: Option<i64>,
) -> Result<Vec<u8>> {
    let bytes = match compression {
        "none" => data.to_vec(),
        "zstd" => zstd::stream::decode_all(data)
            .context("The compressed database copy is damaged and could not be decoded.")?,
        value => anyhow::bail!("The database copy uses unsupported compression: {value}"),
    };
    if let Some(expected) = original_size {
        if i64::try_from(bytes.len()).context("The restored file is too large.")? != expected {
            anyhow::bail!("The database copy is damaged and has the wrong size.");
        }
    }
    Ok(bytes)
}

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    if !has_column(connection, table, column)? {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn query_conversion(connection: &Connection, id: Uuid) -> Result<Option<Conversion>> {
    connection
        .query_row(
            &format!("SELECT {CONVERSION_COLUMNS} FROM conversions WHERE id = ?1"),
            [id.to_string()],
            conversion_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn conversion_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conversion> {
    let id: String = row.get(0)?;
    let source_size: Option<i64> = row.get(8)?;
    let output_size: Option<i64> = row.get(9)?;
    let created_at: String = row.get(11)?;
    let completed_at: Option<String> = row.get(12)?;
    Ok(Conversion {
        id: Uuid::parse_str(&id).map_err(|error| conversion_error(0, error))?,
        source_name: row.get(1)?,
        source_path: row.get(2)?,
        detected_format: row.get(3)?,
        output_format: row.get(4)?,
        engine: row.get(5)?,
        status: row.get(6)?,
        output_path: row.get(7)?,
        source_size: source_size.and_then(|value| u64::try_from(value).ok()),
        output_size: output_size.and_then(|value| u64::try_from(value).ok()),
        error: row.get(10)?,
        created_at: parse_timestamp(11, &created_at)?,
        completed_at: completed_at
            .as_deref()
            .map(|value| parse_timestamp(12, value))
            .transpose()?,
    })
}

fn parse_timestamp(column: usize, value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| conversion_error(column, error))
}

fn conversion_error(
    column: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::{Database, NewConversion};
    use rusqlite::Connection;
    use uuid::Uuid;

    #[test]
    fn migrates_existing_database_to_new_application_home() {
        let directory = tempfile::tempdir().unwrap();
        let legacy_path = directory.path().join("legacy/file-converter.sqlite3");
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        let database = Database::open(&legacy_path).unwrap();
        let source = directory.path().join("letter.docx");
        std::fs::write(&source, b"PK fixture").unwrap();
        let id = Uuid::new_v4();
        database
            .insert_conversion(NewConversion {
                id,
                source_name: "letter.docx",
                source_path: &source,
                detected_format: "docx",
                output_format: "pdf",
                engine: "libreoffice-wasm",
                source_size: 10,
            })
            .unwrap();
        drop(database);

        let new_path = directory
            .path()
            .join("FileConverter/file-converter.sqlite3");
        assert!(Database::migrate_if_missing(&legacy_path, &new_path).unwrap());
        assert_eq!(
            Database::open(&new_path)
                .unwrap()
                .get_conversion(id)
                .unwrap()
                .source_name,
            "letter.docx"
        );
        assert!(!Database::migrate_if_missing(&legacy_path, &new_path).unwrap());
    }

    #[test]
    fn migrates_legacy_schema_without_destroying_blob_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite3");
        let id = Uuid::new_v4();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conversions (
                    id TEXT PRIMARY KEY, source_name TEXT NOT NULL, output_format TEXT NOT NULL,
                    status TEXT NOT NULL, output_path TEXT, output_data BLOB, compression TEXT,
                    original_size INTEGER, stored_size INTEGER, output_base64 TEXT, error TEXT,
                    created_at TEXT NOT NULL
                 );
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversions
                 (id, source_name, output_format, status, output_data, compression, original_size, created_at)
                 VALUES (?1, 'legacy.docx', 'pdf', 'finished', ?2, 'none', ?3, '2026-01-01T00:00:00Z')",
                rusqlite::params![id.to_string(), b"%PDF-legacy", 11],
            )
            .unwrap();
        drop(connection);

        let database = Database::open(&path).unwrap();
        assert_eq!(database.stored_file(id).unwrap().bytes, b"%PDF-legacy");
        let conversion = database.get_conversion(id).unwrap();
        assert_eq!(conversion.source_name, "legacy.docx");
        assert!(conversion.source_path.is_none());
    }

    #[test]
    fn new_conversions_store_metadata_but_not_pdf_blobs() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("test.sqlite3")).unwrap();
        let source = directory.path().join("letter.docx");
        std::fs::write(&source, b"PK fixture").unwrap();
        let id = Uuid::new_v4();
        database
            .insert_conversion(NewConversion {
                id,
                source_name: "letter.docx",
                source_path: &source,
                detected_format: "docx",
                output_format: "pdf",
                engine: "libreoffice-wasm",
                source_size: 10,
            })
            .unwrap();
        let output = directory.path().join("letter.pdf");
        database
            .mark_finished(id, &output, "libreoffice-wasm", 123)
            .unwrap();
        assert!(!database.has_stored_file(id).unwrap());
        let conversion = database.get_conversion(id).unwrap();
        assert_eq!(conversion.output_size, Some(123));
        assert_eq!(conversion.detected_format.as_deref(), Some("docx"));
    }

    #[test]
    fn interrupted_jobs_become_failed() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("test.sqlite3")).unwrap();
        let source = directory.path().join("notes.txt");
        std::fs::write(&source, b"hello").unwrap();
        let id = Uuid::new_v4();
        database
            .insert_conversion(NewConversion {
                id,
                source_name: "notes.txt",
                source_path: &source,
                detected_format: "txt",
                output_format: "pdf",
                engine: "libreoffice-wasm",
                source_size: 5,
            })
            .unwrap();
        database.fail_interrupted_conversions().unwrap();
        assert_eq!(database.get_conversion(id).unwrap().status, "failed");
    }
}
