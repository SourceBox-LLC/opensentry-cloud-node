// OpenSentry CloudNode - Camera streaming node for OpenSentry Cloud
// Copyright (C) 2026  SourceBox LLC
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.
//! SQLite-backed local storage for snapshots and recording segments.
//!
//! Replaces flat-file storage so data isn't exposed in open folders.
//! All binary data (JPEG snapshots, TS segments) is stored as BLOBs.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::{Error, Result};

/// Thread-safe handle to the local SQLite database.
#[derive(Clone)]
pub struct NodeDatabase {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Serialize)]
pub struct SnapshotRecord {
    pub id: i64,
    pub camera_id: String,
    pub filename: String,
    pub timestamp: i64,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct RecordingSummary {
    pub camera_id: String,
    pub date: String,
    pub segment_count: u64,
    pub total_size_bytes: u64,
}

impl NodeDatabase {
    /// Open (or create) the database at the given path.
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Storage(format!("Cannot create DB dir: {}", e)))?;
        }

        let conn = Connection::open(db_path)
            .map_err(|e| Error::Storage(format!("Cannot open DB: {}", e)))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;

             CREATE TABLE IF NOT EXISTS snapshots (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 camera_id  TEXT    NOT NULL,
                 filename   TEXT    NOT NULL,
                 timestamp  INTEGER NOT NULL,
                 data       BLOB   NOT NULL,
                 size_bytes INTEGER NOT NULL
             );

             CREATE TABLE IF NOT EXISTS recording_segments (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 camera_id   TEXT    NOT NULL,
                 segment_seq INTEGER NOT NULL,
                 date        TEXT    NOT NULL,
                 data        BLOB   NOT NULL,
                 size_bytes  INTEGER NOT NULL
             );

             CREATE TABLE IF NOT EXISTS config (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS logs (
                 id        INTEGER PRIMARY KEY AUTOINCREMENT,
                 timestamp TEXT    NOT NULL,
                 level     TEXT    NOT NULL,
                 message   TEXT    NOT NULL
             );

             CREATE INDEX IF NOT EXISTS idx_snap_camera
                 ON snapshots(camera_id);
             CREATE INDEX IF NOT EXISTS idx_rec_camera_date
                 ON recording_segments(camera_id, date);
             CREATE INDEX IF NOT EXISTS idx_logs_timestamp
                 ON logs(id DESC);",
        )
        .map_err(|e| Error::Storage(format!("DB init error: {}", e)))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // ── Snapshots ────────────────────────────────────────────────────────

    pub fn save_snapshot(
        &self,
        camera_id: &str,
        filename: &str,
        timestamp: i64,
        data: &[u8],
    ) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO snapshots (camera_id, filename, timestamp, data, size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![camera_id, filename, timestamp, data, data.len() as i64],
        )
        .map_err(|e| Error::Storage(format!("Snapshot insert error: {}", e)))?;
        Ok(())
    }

    pub fn list_snapshots(&self, camera_id: Option<&str>) -> Result<Vec<SnapshotRecord>> {
        let conn = self.lock()?;
        let mut rows = Vec::new();

        if let Some(cam) = camera_id {
            let mut stmt = conn
                .prepare(
                    "SELECT id, camera_id, filename, timestamp, size_bytes
                     FROM snapshots WHERE camera_id = ?1 ORDER BY timestamp DESC",
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            let iter = stmt
                .query_map(params![cam], |row| {
                    Ok(SnapshotRecord {
                        id: row.get(0)?,
                        camera_id: row.get(1)?,
                        filename: row.get(2)?,
                        timestamp: row.get(3)?,
                        size_bytes: row.get::<_, i64>(4)? as u64,
                    })
                })
                .map_err(|e| Error::Storage(e.to_string()))?;
            for r in iter {
                rows.push(r.map_err(|e| Error::Storage(e.to_string()))?);
            }
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT id, camera_id, filename, timestamp, size_bytes
                     FROM snapshots ORDER BY timestamp DESC",
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            let iter = stmt
                .query_map([], |row| {
                    Ok(SnapshotRecord {
                        id: row.get(0)?,
                        camera_id: row.get(1)?,
                        filename: row.get(2)?,
                        timestamp: row.get(3)?,
                        size_bytes: row.get::<_, i64>(4)? as u64,
                    })
                })
                .map_err(|e| Error::Storage(e.to_string()))?;
            for r in iter {
                rows.push(r.map_err(|e| Error::Storage(e.to_string()))?);
            }
        }

        Ok(rows)
    }

    // ── Recordings ───────────────────────────────────────────────────────

    pub fn save_recording_segment(
        &self,
        camera_id: &str,
        segment_seq: u64,
        date: &str,
        data: &[u8],
    ) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO recording_segments (camera_id, segment_seq, date, data, size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![camera_id, segment_seq as i64, date, data, data.len() as i64],
        )
        .map_err(|e| Error::Storage(format!("Recording insert error: {}", e)))?;
        Ok(())
    }

    pub fn list_recordings(
        &self,
        camera_id: Option<&str>,
    ) -> Result<Vec<RecordingSummary>> {
        let conn = self.lock()?;
        let mut rows = Vec::new();

        let sql = if camera_id.is_some() {
            "SELECT camera_id, date, COUNT(*) as cnt, SUM(size_bytes) as total
             FROM recording_segments WHERE camera_id = ?1
             GROUP BY camera_id, date ORDER BY date DESC"
        } else {
            "SELECT camera_id, date, COUNT(*) as cnt, SUM(size_bytes) as total
             FROM recording_segments
             GROUP BY camera_id, date ORDER BY date DESC"
        };

        let mut stmt = conn.prepare(sql).map_err(|e| Error::Storage(e.to_string()))?;

        let iter = if let Some(cam) = camera_id {
            stmt.query_map(params![cam], Self::map_recording_summary)
        } else {
            stmt.query_map([], Self::map_recording_summary)
        }
        .map_err(|e| Error::Storage(e.to_string()))?;

        for r in iter {
            rows.push(r.map_err(|e| Error::Storage(e.to_string()))?);
        }
        Ok(rows)
    }

    fn map_recording_summary(row: &rusqlite::Row) -> rusqlite::Result<RecordingSummary> {
        Ok(RecordingSummary {
            camera_id: row.get(0)?,
            date: row.get(1)?,
            segment_count: row.get::<_, i64>(2)? as u64,
            total_size_bytes: row.get::<_, i64>(3)? as u64,
        })
    }

    // ── Retention ────────────────────────────────────────────────────────

    /// Total bytes stored across snapshots + recordings.
    pub fn total_size(&self) -> Result<u64> {
        let conn = self.lock()?;
        let snap: i64 = conn
            .query_row("SELECT COALESCE(SUM(size_bytes),0) FROM snapshots", [], |r| r.get(0))
            .map_err(|e| Error::Storage(e.to_string()))?;
        let rec: i64 = conn
            .query_row("SELECT COALESCE(SUM(size_bytes),0) FROM recording_segments", [], |r| r.get(0))
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok((snap + rec) as u64)
    }

    /// Delete the oldest data until total size is under `max_bytes`.
    /// Returns `(current_size, bytes_freed)`.
    pub fn enforce_retention(&self, max_bytes: u64) -> Result<(u64, u64)> {
        let total = self.total_size()?;
        if total <= max_bytes {
            return Ok((total, 0));
        }

        let conn = self.lock()?;
        let mut freed: u64 = 0;
        let excess = total - max_bytes;

        // Delete oldest recording segments first (they're the bulk of the data)
        {
            let mut stmt = conn
                .prepare("SELECT id, size_bytes FROM recording_segments ORDER BY id ASC")
                .map_err(|e| Error::Storage(e.to_string()))?;
            let rows: Vec<(i64, i64)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| Error::Storage(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();

            for (id, size) in rows {
                if freed >= excess {
                    break;
                }
                conn.execute("DELETE FROM recording_segments WHERE id = ?1", params![id])
                    .map_err(|e| Error::Storage(e.to_string()))?;
                freed += size as u64;
            }
        }

        // If still over, delete oldest snapshots
        if freed < excess {
            let mut stmt = conn
                .prepare("SELECT id, size_bytes FROM snapshots ORDER BY id ASC")
                .map_err(|e| Error::Storage(e.to_string()))?;
            let rows: Vec<(i64, i64)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| Error::Storage(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();

            for (id, size) in rows {
                if freed >= excess {
                    break;
                }
                conn.execute("DELETE FROM snapshots WHERE id = ?1", params![id])
                    .map_err(|e| Error::Storage(e.to_string()))?;
                freed += size as u64;
            }
        }

        Ok((total - freed, freed))
    }

    // ── Logs ─────────────────────────────────────────────────────────────

    /// Persist a single log entry.
    pub fn save_log(&self, timestamp: &str, level: &str, message: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO logs (timestamp, level, message) VALUES (?1, ?2, ?3)",
            params![timestamp, level, message],
        )
        .map_err(|e| Error::Storage(format!("Log insert error: {}", e)))?;
        Ok(())
    }

    /// Load the most recent `limit` log entries (oldest first).
    pub fn load_recent_logs(&self, limit: usize) -> Result<Vec<(String, String, String)>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT timestamp, level, message FROM logs ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map(params![limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| Error::Storage(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        // Reverse so oldest is first (for VecDeque push_back ordering)
        Ok(rows.into_iter().rev().collect())
    }

    /// Keep only the most recent `keep` log entries, delete the rest.
    /// Returns the number of rows deleted.
    pub fn prune_logs(&self, keep: usize) -> Result<u64> {
        let conn = self.lock()?;
        let deleted = conn
            .execute(
                "DELETE FROM logs WHERE id NOT IN (SELECT id FROM logs ORDER BY id DESC LIMIT ?1)",
                params![keep as i64],
            )
            .map_err(|e| Error::Storage(format!("Log prune error: {}", e)))?;
        Ok(deleted as u64)
    }

    // ── Config ───────────────────────────────────────────────────────────

    /// Store a config value (plaintext).
    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO config (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(|e| Error::Storage(format!("Config set error: {}", e)))?;
        Ok(())
    }

    /// Read a config value (plaintext).
    pub fn get_config(&self, key: &str) -> Result<Option<String>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT value FROM config WHERE key = ?1")
            .map_err(|e| Error::Storage(e.to_string()))?;
        let result = stmt
            .query_row(params![key], |row| row.get(0))
            .ok();
        Ok(result)
    }

    /// Store a config value encrypted with machine-derived key.
    pub fn set_config_encrypted(&self, key: &str, plaintext: &str) -> Result<()> {
        let encrypted = encrypt_value(plaintext)
            .map_err(|e| Error::Storage(format!("Encryption error: {}", e)))?;
        self.set_config(key, &encrypted)
    }

    /// Read and decrypt a config value.
    pub fn get_config_encrypted(&self, key: &str) -> Result<Option<String>> {
        match self.get_config(key)? {
            Some(encrypted) => {
                let plaintext = decrypt_value(&encrypted)
                    .map_err(|e| Error::Storage(format!("Decryption error: {}", e)))?;
                Ok(Some(plaintext))
            }
            None => Ok(None),
        }
    }

    /// Delete a config key from the database.
    pub fn delete_config(&self, key: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM config WHERE key = ?1", params![key])
            .map_err(|e| Error::Storage(format!("Config delete error: {}", e)))?;
        Ok(())
    }

    /// Check if any config values exist in the database.
    pub fn has_config(&self) -> bool {
        self.get_config("node_id")
            .ok()
            .flatten()
            .is_some()
    }

    // ── Wipe ─────────────────────────────────────────────────────────────

    /// Delete all stored data (called when the node is decommissioned).
    pub fn wipe_all(&self) -> Result<()> {
        let conn = self.lock()?;
        conn.execute_batch(
            "DELETE FROM snapshots; DELETE FROM recording_segments; DELETE FROM logs; DELETE FROM config; VACUUM;",
        )
        .map_err(|e| Error::Storage(format!("Wipe error: {}", e)))?;
        Ok(())
    }

    // ── Internal ─────────────────────────────────────────────────────────

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| Error::Storage(format!("DB lock error: {}", e)))
    }
}

// ─── AES-256-GCM encryption ─────────────────────────────────────────────────
//
// Encrypts secrets (API key) at rest using a key derived from the machine's
// hostname. Moving the DB to another machine makes the encrypted values
// unreadable without knowing the original hostname.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, AeadCore, Nonce,
};
use sha2::{Digest, Sha256};

/// Derive a 256-bit encryption key from the machine's hostname.
fn derive_key() -> [u8; 32] {
    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "opensentry-fallback".to_string());
    let mut hasher = Sha256::new();
    hasher.update(host.as_bytes());
    hasher.update(b"opensentry-cloudnode-v1");
    hasher.finalize().into()
}

/// Encrypt a plaintext string → hex-encoded (nonce ‖ ciphertext).
fn encrypt_value(plaintext: &str) -> std::result::Result<String, String> {
    let key = derive_key();
    let cipher = Aes256Gcm::new(&key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| format!("encrypt: {}", e))?;
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(to_hex(&combined))
}

/// Decrypt a hex-encoded (nonce ‖ ciphertext) → plaintext string.
fn decrypt_value(hex_str: &str) -> std::result::Result<String, String> {
    let combined = from_hex(hex_str).map_err(|e| format!("hex decode: {}", e))?;
    if combined.len() < 13 {
        return Err("ciphertext too short".into());
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let key = derive_key();
    let cipher = Aes256Gcm::new(&key.into());
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "decryption failed (wrong machine or corrupted data)".to_string())?;
    String::from_utf8(plaintext).map_err(|e| format!("utf8: {}", e))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn from_hex(hex: &str) -> std::result::Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|_| format!("bad hex at {}", i))
        })
        .collect()
}
