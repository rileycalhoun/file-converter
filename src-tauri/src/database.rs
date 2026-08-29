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

#[derive(Clone)]
pub struct PrivateSettings {
    pub gateway_url: String,
    pub gateway_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicSettings {
    pub gateway_url: String,
    pub token_configured: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversion {
    pub id: Uuid,
    pub source_name: String,
    pub output_format: String,
    pub status: String,
    pub output_path: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
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

impl Database {
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
                output_format TEXT NOT NULL,
                status TEXT NOT NULL,
                output_path TEXT,
                output_data BLOB,
                compression TEXT,
                original_size INTEGER,
                stored_size INTEGER,
                output_base64 TEXT,
                error TEXT,
                created_at TEXT NOT NULL
             );",
        )?;
        for (column, definition) in [
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

    pub fn public_settings(&self, token_configured: bool) -> Result<PublicSettings> {
        let connection = self.lock()?;
        let gateway_url = setting(&connection, "gateway_url")?.unwrap_or_default();
        Ok(PublicSettings {
            gateway_url,
            token_configured,
        })
    }

    pub fn private_settings(&self, gateway_token: String) -> Result<PrivateSettings> {
        let connection = self.lock()?;
        let gateway_url = setting(&connection, "gateway_url")?
            .filter(|value| !value.is_empty())
            .context("Configure the gateway URL in Settings first.")?;
        Ok(PrivateSettings {
            gateway_url,
            gateway_token,
        })
    }

    pub fn save_gateway_url(&self, gateway_url: &str) -> Result<()> {
        let connection = self.lock()?;
        upsert_setting(&connection, "gateway_url", gateway_url)?;
        Ok(())
    }

    pub fn legacy_gateway_token(&self) -> Result<Option<String>> {
        let connection = self.lock()?;
        Ok(setting(&connection, "gateway_token")?.filter(|value| !value.trim().is_empty()))
    }

    pub fn delete_legacy_gateway_token(&self) -> Result<()> {
        let connection = self.lock()?;
        connection.execute("DELETE FROM settings WHERE key = 'gateway_token'", [])?;
        Ok(())
    }

    pub fn fail_interrupted_conversions(&self) -> Result<()> {
        let connection = self.lock()?;
        connection.execute(
            "UPDATE conversions
             SET status = 'failed', error = 'Conversion was interrupted before it completed.'
             WHERE status = 'processing'",
            [],
        )?;
        Ok(())
    }

    pub fn backfill_stored_files(&self) -> Result<usize> {
        let pending = {
            let connection = self.lock()?;
            let mut statement = connection.prepare(
                "SELECT id, output_path FROM conversions
                 WHERE status = 'finished'
                   AND output_path IS NOT NULL
                   AND output_data IS NULL
                   AND (output_base64 IS NULL OR output_base64 = '')",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut stored = 0;
        for (id, path) in pending {
            if let Ok(bytes) = std::fs::read(path) {
                self.save_stored_file(&id, &bytes)?;
                stored += 1;
            }
        }
        Ok(stored)
    }

    pub fn migrate_legacy_base64_files(&self) -> Result<usize> {
        let ids = {
            let connection = self.lock()?;
            let mut statement = connection.prepare(
                "SELECT id FROM conversions
                 WHERE output_data IS NULL
                   AND output_base64 IS NOT NULL
                   AND output_base64 <> ''",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut migrated = 0;
        for id in ids {
            let encoded: String = {
                let connection = self.lock()?;
                connection.query_row(
                    "SELECT output_base64 FROM conversions WHERE id = ?1",
                    [&id],
                    |row| row.get(0),
                )?
            };
            if let Ok(bytes) = STANDARD.decode(encoded) {
                self.save_stored_file(&id, &bytes)?;
                migrated += 1;
            }
        }
        Ok(migrated)
    }

    pub fn insert_conversion(
        &self,
        id: Uuid,
        source_name: &str,
        output_format: &str,
    ) -> Result<Conversion> {
        let created_at = Utc::now();
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO conversions
             (id, source_name, output_format, status, created_at)
             VALUES (?1, ?2, ?3, 'processing', ?4)",
            params![
                id.to_string(),
                source_name,
                output_format,
                created_at.to_rfc3339()
            ],
        )?;
        drop(connection);
        self.get_conversion(id)
    }

    pub fn get_conversion(&self, id: Uuid) -> Result<Conversion> {
        let connection = self.lock()?;
        query_conversion(&connection, id)?.context("Conversion was not found.")
    }

    pub fn list_conversions(&self) -> Result<Vec<Conversion>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, source_name, output_format, status, output_path, error, created_at
             FROM conversions ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map([], conversion_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn mark_failed(&self, id: Uuid, error: &str) -> Result<Conversion> {
        let connection = self.lock()?;
        connection.execute(
            "UPDATE conversions SET status = 'failed', error = ?2 WHERE id = ?1",
            params![id.to_string(), error],
        )?;
        drop(connection);
        self.get_conversion(id)
    }

    pub fn mark_finished(
        &self,
        id: Uuid,
        output_path: &Path,
        output_bytes: &[u8],
    ) -> Result<Conversion> {
        let stored = encode_stored_file(output_bytes)?;
        let connection = self.lock()?;
        connection.execute(
            "UPDATE conversions
             SET status = 'finished', output_path = ?2,
                 output_data = ?3, compression = ?4,
                 original_size = ?5, stored_size = ?6,
                 output_base64 = NULL, error = NULL
             WHERE id = ?1",
            params![
                id.to_string(),
                output_path.to_string_lossy(),
                stored.data,
                stored.compression,
                stored.original_size,
                stored.stored_size,
            ],
        )?;
        drop(connection);
        self.get_conversion(id)
    }

    pub fn has_stored_file(&self, id: Uuid) -> Result<bool> {
        let connection = self.lock()?;
        connection
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
        let connection = self.lock()?;
        let stored: Option<StoredFileRow> = connection
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

    fn save_stored_file(&self, id: &str, bytes: &[u8]) -> Result<()> {
        let stored = encode_stored_file(bytes)?;
        let connection = self.lock()?;
        connection.execute(
            "UPDATE conversions
             SET output_data = ?2, compression = ?3,
                 original_size = ?4, stored_size = ?5, output_base64 = NULL
             WHERE id = ?1",
            params![
                id,
                stored.data,
                stored.compression,
                stored.original_size,
                stored.stored_size,
            ],
        )?;
        Ok(())
    }

    pub fn update_output_path(&self, id: Uuid, output_path: &Path) -> Result<()> {
        let connection = self.lock()?;
        connection.execute(
            "UPDATE conversions SET output_path = ?2 WHERE id = ?1",
            params![id.to_string(), output_path.to_string_lossy()],
        )?;
        Ok(())
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

struct EncodedFile {
    data: Vec<u8>,
    compression: &'static str,
    original_size: i64,
    stored_size: i64,
}

fn encode_stored_file(bytes: &[u8]) -> Result<EncodedFile> {
    let compressed =
        zstd::stream::encode_all(bytes, 3).context("Could not compress the database copy.")?;
    let (data, compression) = if compressed.len() < bytes.len() {
        (compressed, "zstd")
    } else {
        (bytes.to_vec(), "none")
    };
    Ok(EncodedFile {
        original_size: i64::try_from(bytes.len()).context("The converted file is too large.")?,
        stored_size: i64::try_from(data.len()).context("The database copy is too large.")?,
        data,
        compression,
    })
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
        let actual = i64::try_from(bytes.len()).context("The restored file is too large.")?;
        if actual != expected {
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

fn setting(connection: &Connection, key: &str) -> Result<Option<String>> {
    connection
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(Into::into)
}

fn upsert_setting(connection: &Connection, key: &str, value: &str) -> Result<()> {
    connection.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn query_conversion(connection: &Connection, id: Uuid) -> Result<Option<Conversion>> {
    connection
        .query_row(
            "SELECT id, source_name, output_format, status, output_path, error, created_at
             FROM conversions WHERE id = ?1",
            [id.to_string()],
            conversion_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn conversion_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conversion> {
    let id: String = row.get(0)?;
    let created_at: String = row.get(6)?;
    Ok(Conversion {
        id: Uuid::parse_str(&id).map_err(|error| conversion_error(0, error))?,
        source_name: row.get(1)?,
        output_format: row.get(2)?,
        status: row.get(3)?,
        output_path: row.get(4)?,
        error: row.get(5)?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map_err(|error| conversion_error(6, error))?
            .with_timezone(&Utc),
    })
}

fn conversion_error(
    column: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::Database;
    use rusqlite::Connection;
    use uuid::Uuid;

    #[test]
    fn stores_settings_and_conversion_history() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("test.sqlite3")).unwrap();
        database
            .save_gateway_url("https://gateway.example.com")
            .unwrap();
        assert_eq!(
            database.public_settings(true).unwrap().gateway_url,
            "https://gateway.example.com"
        );

        {
            let connection = database.lock().unwrap();
            super::upsert_setting(&connection, "gateway_token", "legacy-secret").unwrap();
        }
        assert_eq!(
            database.legacy_gateway_token().unwrap().as_deref(),
            Some("legacy-secret")
        );
        database.delete_legacy_gateway_token().unwrap();
        assert!(database.legacy_gateway_token().unwrap().is_none());

        let id = Uuid::new_v4();
        database
            .insert_conversion(id, "letter.docx", "pdf")
            .unwrap();
        database.fail_interrupted_conversions().unwrap();
        let conversions = database.list_conversions().unwrap();
        assert_eq!(conversions.len(), 1);
        assert_eq!(conversions[0].status, "failed");

        let output_path = directory.path().join("letter.pdf");
        database
            .mark_finished(id, &output_path, b"%PDF-saved-copy")
            .unwrap();
        {
            let connection = database.lock().unwrap();
            let stored: (String, String, i64, i64, Option<String>) = connection
                .query_row(
                    "SELECT typeof(output_data), compression, original_size,
                            stored_size, output_base64
                     FROM conversions WHERE id = ?1",
                    [id.to_string()],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .unwrap();
            assert_eq!(stored, ("blob".into(), "none".into(), 15, 15, None));
        }
        assert!(database.has_stored_file(id).unwrap());
        assert_eq!(database.stored_file(id).unwrap().bytes, b"%PDF-saved-copy");
    }

    #[test]
    fn migrates_existing_conversion_history_for_saved_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite3");
        let connection = Connection::open(&path).unwrap();
        let id = Uuid::new_v4();
        let output_path = directory.path().join("legacy.pdf");
        std::fs::write(&output_path, b"%PDF-legacy-copy").unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conversions (
                    id TEXT PRIMARY KEY,
                    source_name TEXT NOT NULL,
                    output_format TEXT NOT NULL,
                    status TEXT NOT NULL,
                    output_path TEXT,
                    error TEXT,
                    created_at TEXT NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversions
                 (id, source_name, output_format, status, output_path, created_at)
                 VALUES (?1, 'legacy.docx', 'pdf', 'finished', ?2, '2026-01-01T00:00:00Z')",
                rusqlite::params![id.to_string(), output_path.to_string_lossy()],
            )
            .unwrap();
        drop(connection);

        let database = Database::open(&path).unwrap();
        {
            let connection = database.lock().unwrap();
            assert!(super::has_column(&connection, "conversions", "output_base64").unwrap());
            assert!(super::has_column(&connection, "conversions", "output_data").unwrap());
        }
        assert_eq!(database.backfill_stored_files().unwrap(), 1);
        assert_eq!(database.stored_file(id).unwrap().bytes, b"%PDF-legacy-copy");
        assert_eq!(database.backfill_stored_files().unwrap(), 0);
    }

    #[test]
    fn migrates_legacy_base64_copies_to_blobs() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("base64.sqlite3")).unwrap();
        let id = Uuid::new_v4();
        database
            .insert_conversion(id, "archive.docx", "pdf")
            .unwrap();
        {
            let connection = database.lock().unwrap();
            connection
                .execute(
                    "UPDATE conversions
                     SET status = 'finished', output_base64 = 'JVBERi1sZWdhY3ktYmFzZTY0'
                     WHERE id = ?1",
                    [id.to_string()],
                )
                .unwrap();
        }

        assert_eq!(database.migrate_legacy_base64_files().unwrap(), 1);
        assert_eq!(
            database.stored_file(id).unwrap().bytes,
            b"%PDF-legacy-base64"
        );
        let connection = database.lock().unwrap();
        let legacy: Option<String> = connection
            .query_row(
                "SELECT output_base64 FROM conversions WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(legacy.is_none());
    }

    #[test]
    fn uses_zstd_only_when_it_reduces_the_stored_size() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("compressed.sqlite3")).unwrap();
        let id = Uuid::new_v4();
        database.insert_conversion(id, "large.docx", "pdf").unwrap();
        let bytes = vec![b'A'; 4096];
        database
            .mark_finished(id, &directory.path().join("large.pdf"), &bytes)
            .unwrap();

        let connection = database.lock().unwrap();
        let (compression, original_size, stored_size): (String, i64, i64) = connection
            .query_row(
                "SELECT compression, original_size, stored_size
                 FROM conversions WHERE id = ?1",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        drop(connection);
        assert_eq!(compression, "zstd");
        assert_eq!(original_size, 4096);
        assert!(stored_size < original_size);
        assert_eq!(database.stored_file(id).unwrap().bytes, bytes);
    }
}
