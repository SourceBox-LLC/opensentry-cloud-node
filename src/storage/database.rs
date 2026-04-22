// SourceBox Sentry CloudNode - Camera streaming node for SourceBox Sentry Cloud
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
    ///
    /// If the ciphertext was written by a CloudNode version that derived its
    /// key from the hostname, the value is transparently re-encrypted with
    /// the current machine-id-derived key so subsequent loads take the fast
    /// path and the weak legacy key is retired from the DB.
    pub fn get_config_encrypted(&self, key: &str) -> Result<Option<String>> {
        match self.get_config(key)? {
            Some(encrypted) => {
                let (plaintext, was_legacy) = decrypt_value(&encrypted)
                    .map_err(|e| Error::Storage(format!("Decryption error: {}", e)))?;
                if was_legacy {
                    tracing::info!(
                        "Migrating encrypted config key '{}' from hostname-derived \
                         (v1) to machine-id-derived (v2) encryption key",
                        key,
                    );
                    // Best-effort: if re-encryption fails we still return the
                    // decrypted value so the node keeps working. Next startup
                    // will retry the migration.
                    if let Err(e) = self.set_config_encrypted(key, &plaintext) {
                        tracing::warn!(
                            "Failed to re-encrypt config key '{}' during migration: {}",
                            key,
                            e,
                        );
                    }
                }
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
// Secrets (currently just the cloud API key) are encrypted at rest with a key
// derived from the host machine's OS-managed machine identifier — on Linux
// `/etc/machine-id`, on Windows `HKLM\SOFTWARE\Microsoft\Cryptography\
// MachineGuid`, on macOS `IOPlatformUUID`. These are 128-bit values set once
// at OS install time and not user-modifiable, so an attacker who steals just
// `node.db` can't decrypt the API key without also having code execution on
// the original host.
//
// Earlier versions derived the key from the hostname, which is predictable
// (most Pis ship as `raspberrypi`) and trivially guessable from a stolen DB.
// The older path is kept as `derive_key_legacy` only to transparently migrate
// existing installs: on first decrypt we try the new key, fall back to the
// legacy key, and immediately re-encrypt with the new key.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, AeadCore, Nonce,
};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// Domain-separation tag mixed into the SHA-256 hash so the same machine ID
/// never produces the same key as some unrelated tool that happens to hash
/// the same input. `v2` marks the switch from hostname-derived (v1) keys.
const KEY_DOMAIN_V2: &[u8] = b"opensentry-cloudnode-machine-id-v2";

/// Legacy domain tag — kept so `derive_key_legacy` still reproduces the
/// pre-migration key for DBs written with the old code.
const KEY_DOMAIN_V1_LEGACY: &[u8] = b"opensentry-cloudnode-v1";

/// Cached derived key for this process — `machine_id()` on Linux reads a
/// file and on Windows / macOS shells out, so we avoid re-deriving per op.
static CACHED_KEY: OnceLock<[u8; 32]> = OnceLock::new();

/// Read the OS-managed machine identifier.
///
/// Returns an error (not a fallback value) so weak encryption can't silently
/// slip back in — the caller surfaces the error and the user can file a bug
/// with the exact platform. Cached by `derive_key` so we pay the I/O cost at
/// most once per process.
///
/// On Linux, if the OS sources are unavailable (e.g. minimal Docker images
/// without systemd or dbus), falls back to a node-local identifier stored at
/// `$OPENSENTRY_DATA_DIR/.machine-id`, generating one from
/// `/proc/sys/kernel/random/uuid` on first use. This is weaker than a
/// host-wide ID (an attacker who copies the data directory gets the key
/// material) but still a major upgrade over the hostname-derived v1 key.
fn machine_id() -> std::result::Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        // systemd / freedesktop.org standard. Both files hold a 32-char hex
        // string written at OS install time.
        for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(content) = std::fs::read_to_string(path) {
                let id = content.trim();
                if !id.is_empty() {
                    return Ok(id.to_string());
                }
            }
        }
        // Docker/minimal-image fallback: generate once, persist to the data
        // volume so the ID survives container rebuilds.
        if let Ok(data_dir) = std::env::var("OPENSENTRY_DATA_DIR") {
            let fallback_path = std::path::PathBuf::from(&data_dir).join(".machine-id");
            if let Ok(content) = std::fs::read_to_string(&fallback_path) {
                let id = content.trim();
                if !id.is_empty() {
                    return Ok(id.to_string());
                }
            }
            // Not present — generate from the kernel RNG.
            if let Ok(uuid) = std::fs::read_to_string("/proc/sys/kernel/random/uuid") {
                let id = uuid.trim().to_string();
                if !id.is_empty() {
                    // Best-effort write; if it fails we still return the ID
                    // for this process but the next run will generate again.
                    if let Err(e) = std::fs::write(&fallback_path, &id) {
                        tracing::warn!(
                            "Could not persist fallback machine ID to {}: {}",
                            fallback_path.display(),
                            e,
                        );
                    }
                    return Ok(id);
                }
            }
        }
        return Err(
            "machine ID not found (tried /etc/machine-id, /var/lib/dbus/machine-id, \
             $OPENSENTRY_DATA_DIR/.machine-id). \
             Run `sudo systemd-machine-id-setup` or `sudo dbus-uuidgen --ensure=/etc/machine-id`, \
             or set OPENSENTRY_DATA_DIR to a writable directory."
                .into(),
        );
    }

    #[cfg(target_os = "windows")]
    {
        // HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid — present on every
        // Windows install since Vista. Read via `reg query` so we don't pull
        // in a Windows-only registry crate.
        use std::process::Command;
        let output = Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Cryptography",
                "/v",
                "MachineGuid",
            ])
            .output()
            .map_err(|e| format!("reg query failed: {}", e))?;
        if !output.status.success() {
            return Err(format!(
                "reg query exited with status {}",
                output.status
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Output format: "    MachineGuid    REG_SZ    <guid>"
        for line in stdout.lines() {
            if let Some(idx) = line.find("REG_SZ") {
                let value = line[idx + "REG_SZ".len()..].trim();
                if !value.is_empty() {
                    return Ok(value.to_string());
                }
            }
        }
        return Err("MachineGuid not found in registry output".into());
    }

    #[cfg(target_os = "macos")]
    {
        // IOPlatformUUID from the IOKit registry.
        use std::process::Command;
        let output = Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .map_err(|e| format!("ioreg failed: {}", e))?;
        if !output.status.success() {
            return Err(format!("ioreg exited with status {}", output.status));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("IOPlatformUUID") {
                // Line format: `    "IOPlatformUUID" = "XXXXXXXX-..."`
                if let Some(after_eq) = line.split('=').nth(1) {
                    let value = after_eq.trim().trim_matches('"');
                    if !value.is_empty() {
                        return Ok(value.to_string());
                    }
                }
            }
        }
        return Err("IOPlatformUUID not found in ioreg output".into());
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Err("machine ID lookup not implemented for this platform".into())
    }
}

/// Derive a 256-bit encryption key from the OS machine identifier.
fn derive_key() -> std::result::Result<[u8; 32], String> {
    if let Some(k) = CACHED_KEY.get() {
        return Ok(*k);
    }
    let id = machine_id()?;
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    hasher.update(KEY_DOMAIN_V2);
    let key: [u8; 32] = hasher.finalize().into();
    // OnceLock::set returns Err if already set — harmless race, either value
    // is the same because machine_id() is deterministic within a process.
    let _ = CACHED_KEY.set(key);
    Ok(key)
}

/// Derive the pre-migration key from the hostname.
///
/// Only used by `decrypt_value` to transparently migrate DBs written by
/// earlier CloudNode versions. Kept byte-for-byte identical to the old
/// implementation so existing ciphertexts still decrypt.
fn derive_key_legacy() -> [u8; 32] {
    let host = sysinfo::System::host_name()
        .unwrap_or_else(|| "opensentry-fallback".to_string());
    let mut hasher = Sha256::new();
    hasher.update(host.as_bytes());
    hasher.update(KEY_DOMAIN_V1_LEGACY);
    hasher.finalize().into()
}

/// Encrypt with an explicit key (pure — no machine-id dependency, testable).
fn encrypt_with_key(key: &[u8; 32], plaintext: &str) -> std::result::Result<String, String> {
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| format!("encrypt: {}", e))?;
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(to_hex(&combined))
}

/// Decrypt with an explicit key (pure — no machine-id dependency, testable).
fn decrypt_with_key(key: &[u8; 32], hex_str: &str) -> std::result::Result<String, String> {
    let combined = from_hex(hex_str).map_err(|e| format!("hex decode: {}", e))?;
    if combined.len() < 13 {
        return Err("ciphertext too short".into());
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "decryption failed (wrong key or corrupted data)".to_string())?;
    String::from_utf8(plaintext).map_err(|e| format!("utf8: {}", e))
}

/// Encrypt a plaintext string → hex-encoded (nonce ‖ ciphertext).
///
/// Uses the v2 (machine-id-derived) key.
fn encrypt_value(plaintext: &str) -> std::result::Result<String, String> {
    let key = derive_key()?;
    encrypt_with_key(&key, plaintext)
}

/// Decrypt a hex-encoded (nonce ‖ ciphertext) → plaintext string.
///
/// Tries the current (machine-id) key first. If that fails, tries the legacy
/// hostname-derived key for DBs written by older CloudNode versions. Returns
/// `(plaintext, was_legacy)` — the caller re-encrypts with the new key when
/// `was_legacy` is true so the next load is fast and the legacy path is
/// eventually exercised to zero on every install.
fn decrypt_value(hex_str: &str) -> std::result::Result<(String, bool), String> {
    let new_key = derive_key()?;
    if let Ok(pt) = decrypt_with_key(&new_key, hex_str) {
        return Ok((pt, false));
    }
    let legacy_key = derive_key_legacy();
    match decrypt_with_key(&legacy_key, hex_str) {
        Ok(pt) => Ok((pt, true)),
        Err(_) => Err("decryption failed (wrong machine or corrupted data)".into()),
    }
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

#[cfg(test)]
mod encryption_tests {
    use super::*;

    #[test]
    fn roundtrip_with_known_key() {
        let key = [0x42u8; 32];
        let ct = encrypt_with_key(&key, "super secret value").unwrap();
        let pt = decrypt_with_key(&key, &ct).unwrap();
        assert_eq!(pt, "super secret value");
    }

    #[test]
    fn roundtrip_preserves_empty_string() {
        let key = [0x01u8; 32];
        let ct = encrypt_with_key(&key, "").unwrap();
        assert_eq!(decrypt_with_key(&key, &ct).unwrap(), "");
    }

    #[test]
    fn roundtrip_preserves_utf8() {
        let key = [0x7fu8; 32];
        let pt_in = "tokens: 🔐 ñ é 中文";
        let ct = encrypt_with_key(&key, pt_in).unwrap();
        assert_eq!(decrypt_with_key(&key, &ct).unwrap(), pt_in);
    }

    #[test]
    fn nonce_randomness_produces_distinct_ciphertexts() {
        // AEAD encrypting the same plaintext twice must use a fresh nonce,
        // or confidentiality is gone. A round-trip test won't catch this —
        // check that two encryptions of the same plaintext differ.
        let key = [0x33u8; 32];
        let a = encrypt_with_key(&key, "same plaintext").unwrap();
        let b = encrypt_with_key(&key, "same plaintext").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn decrypt_rejects_wrong_key() {
        let ct = encrypt_with_key(&[1u8; 32], "secret").unwrap();
        assert!(decrypt_with_key(&[2u8; 32], &ct).is_err());
    }

    #[test]
    fn decrypt_rejects_tampered_ciphertext() {
        // AES-GCM is authenticated — flipping a byte in the tag or body
        // must fail decrypt. Prevents silent corruption / bit-flip attacks.
        let key = [0x44u8; 32];
        let ct = encrypt_with_key(&key, "hello").unwrap();
        let mut bytes = from_hex(&ct).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let tampered = to_hex(&bytes);
        assert!(decrypt_with_key(&key, &tampered).is_err());
    }

    #[test]
    fn decrypt_rejects_truncated_ciphertext() {
        // Nonce is 12 bytes; anything shorter than nonce + 1 byte of tag is
        // rejected up front rather than being handed to AES-GCM.
        let key = [0x55u8; 32];
        assert!(decrypt_with_key(&key, "").is_err());
        assert!(decrypt_with_key(&key, "00").is_err());
        assert!(decrypt_with_key(&key, &to_hex(&[0u8; 12])).is_err());
    }

    #[test]
    fn decrypt_rejects_bad_hex() {
        let key = [0x66u8; 32];
        assert!(decrypt_with_key(&key, "not hex at all").is_err());
        assert!(decrypt_with_key(&key, "abc").is_err()); // odd length
    }

    #[test]
    fn legacy_and_new_keys_are_distinct() {
        // Even if a hostile environment somehow produced the same value for
        // the hostname and the machine-id, the v1/v2 domain tags guarantee
        // the derived keys still differ.
        let host_matches_id = "constant";
        let mut v1 = Sha256::new();
        v1.update(host_matches_id.as_bytes());
        v1.update(KEY_DOMAIN_V1_LEGACY);
        let v1_key: [u8; 32] = v1.finalize().into();

        let mut v2 = Sha256::new();
        v2.update(host_matches_id.as_bytes());
        v2.update(KEY_DOMAIN_V2);
        let v2_key: [u8; 32] = v2.finalize().into();

        assert_ne!(v1_key, v2_key);
    }

    #[test]
    fn legacy_ciphertext_cannot_decrypt_with_new_key() {
        // Simulates the migration scenario: DB written by the old code can't
        // be opened with the new key, but decrypt_with_key against the
        // legacy key still works.
        let legacy = [0xaau8; 32];
        let new = [0xbbu8; 32];
        let ct = encrypt_with_key(&legacy, "api_key_abc123").unwrap();
        assert!(decrypt_with_key(&new, &ct).is_err());
        assert_eq!(
            decrypt_with_key(&legacy, &ct).unwrap(),
            "api_key_abc123",
        );
    }

    #[test]
    fn hex_roundtrip() {
        let data = vec![0x00, 0xff, 0x42, 0xab, 0xcd];
        assert_eq!(to_hex(&data), "00ff42abcd");
        assert_eq!(from_hex("00ff42abcd").unwrap(), data);
    }

    #[test]
    fn from_hex_rejects_odd_length() {
        assert!(from_hex("abc").is_err());
    }

    #[test]
    fn from_hex_rejects_non_hex_chars() {
        assert!(from_hex("gg").is_err());
        assert!(from_hex("xy").is_err());
    }

    // ── Live platform smoke test ──────────────────────────────────────
    //
    // Runs the real `machine_id()` / `derive_key()` against the host the
    // tests run on. It's the only test that actually shells out (on
    // Windows + macOS) or reads /etc/machine-id (on Linux), so it's
    // deliberately lightweight — we just check that the lookup works and
    // the derived key is non-zero. CI and local dev all exercise it.

    #[test]
    fn machine_id_returns_nonempty_on_this_platform() {
        let id = machine_id().expect("machine_id() must succeed on this platform");
        assert!(!id.is_empty(), "machine ID came back empty");
        // Sanity: both Linux's /etc/machine-id (32 hex) and Windows/macOS
        // GUIDs (36 chars with dashes) comfortably exceed 8 characters.
        assert!(
            id.len() >= 8,
            "machine ID suspiciously short ({} chars): {:?}",
            id.len(),
            id,
        );
    }

    #[test]
    fn derive_key_produces_nonzero_output() {
        let k = derive_key().expect("derive_key() must succeed on this platform");
        assert_ne!(k, [0u8; 32], "derived key is all zeros");
    }

    #[test]
    fn derive_key_is_deterministic_within_process() {
        let a = derive_key().expect("first derive_key");
        let b = derive_key().expect("second derive_key");
        assert_eq!(a, b);
    }
}
