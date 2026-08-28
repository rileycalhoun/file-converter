use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result};
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
                error TEXT,
                created_at TEXT NOT NULL
             );",
        )?;
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

    pub fn mark_finished(&self, id: Uuid, output_path: &Path) -> Result<Conversion> {
        let connection = self.lock()?;
        connection.execute(
            "UPDATE conversions
             SET status = 'finished', output_path = ?2, error = NULL
             WHERE id = ?1",
            params![id.to_string(), output_path.to_string_lossy()],
        )?;
        drop(connection);
        self.get_conversion(id)
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
        database
            .mark_finished(id, &directory.path().join("letter.pdf"))
            .unwrap();
        let conversions = database.list_conversions().unwrap();
        assert_eq!(conversions.len(), 1);
        assert_eq!(conversions[0].status, "finished");
    }
}
