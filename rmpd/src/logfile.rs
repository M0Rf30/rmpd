// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A log-file writer that can be re-opened at runtime.
//!
//! MPD re-opens its log file on `SIGHUP` (`LogInit.cxx`) so `logrotate` can
//! rename the file and then signal the daemon. [`ReopenableFile`] provides the
//! same behaviour for `tracing_subscriber`: it is a cheaply clonable handle
//! that implements [`MakeWriter`], and [`ReopenableFile::reopen`] atomically
//! swaps the underlying descriptor for a freshly opened one.

#![cfg_attr(not(unix), allow(dead_code))]

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use tracing_subscriber::fmt::MakeWriter;

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// Shared handle to an append-mode log file that can be swapped for a freshly
/// opened descriptor on demand.
#[derive(Clone, Debug)]
pub struct ReopenableFile {
    path: Arc<PathBuf>,
    // Writers take the read lock (a `&File` can be written concurrently, and
    // `O_APPEND` keeps each `write(2)` atomic); only `reopen` takes the write
    // lock, so logging threads never serialise against each other.
    file: Arc<RwLock<File>>,
}

impl ReopenableFile {
    /// Open (creating if needed) `path` for appending.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let file = open_append(&path)?;
        Ok(Self {
            path: Arc::new(path),
            file: Arc::new(RwLock::new(file)),
        })
    }

    /// Path this handle writes to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Open the path again and replace the current descriptor with it. On
    /// failure the previous descriptor stays in use, so no log line is lost.
    pub fn reopen(&self) -> io::Result<()> {
        let fresh = open_append(&self.path)?;
        *self.file.write().unwrap_or_else(PoisonError::into_inner) = fresh;
        Ok(())
    }
}

/// Writer handed out per log event by [`ReopenableFile`].
pub struct LogWriter<'a>(&'a ReopenableFile);

impl Write for LogWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let guard = self.0.file.read().unwrap_or_else(PoisonError::into_inner);
        (&*guard).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let guard = self.0.file.read().unwrap_or_else(PoisonError::into_inner);
        (&*guard).flush()
    }
}

impl<'a> MakeWriter<'a> for ReopenableFile {
    type Writer = LogWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self)
    }
}

/// Re-open `log` every time the process receives `SIGHUP` (logrotate hook).
///
/// The signal handler is registered before this returns, so there is no window
/// in which `SIGHUP` would still hit the default (terminate) disposition once
/// the call has succeeded. Must be called from within a tokio runtime context.
#[cfg(unix)]
pub fn reopen_on_sighup(log: ReopenableFile) -> io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut hup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        while hup.recv().await.is_some() {
            match log.reopen() {
                Ok(()) => tracing::info!("SIGHUP: reopened log file {}", log.path().display()),
                Err(e) => tracing::warn!(
                    "SIGHUP: unable to reopen log file {} ({e}); keeping the previous one",
                    log.path().display()
                ),
            }
        }
    });
    Ok(())
}

/// Non-unix platforms have no `SIGHUP`; log rotation by signal is a no-op.
#[cfg(not(unix))]
pub fn reopen_on_sighup(_log: ReopenableFile) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rmpd-logfile-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn appends_to_existing_content() {
        let dir = temp_dir("append");
        let path = dir.join("a.log");
        std::fs::write(&path, "old\n").unwrap();
        let log = ReopenableFile::open(&path).unwrap();
        log.make_writer().write_all(b"new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old\nnew\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reopen_follows_rotation() {
        let dir = temp_dir("rotate");
        let path = dir.join("r.log");
        let rotated = dir.join("r.log.1");
        let log = ReopenableFile::open(&path).unwrap();
        log.make_writer().write_all(b"before\n").unwrap();

        // logrotate: rename, then signal. Writes still land in the renamed file.
        std::fs::rename(&path, &rotated).unwrap();
        log.make_writer().write_all(b"still-old\n").unwrap();
        assert!(!path.exists());

        log.reopen().unwrap();
        log.make_writer().write_all(b"after\n").unwrap();

        assert_eq!(
            std::fs::read_to_string(&rotated).unwrap(),
            "before\nstill-old\n"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_reopen_keeps_previous_file() {
        let dir = temp_dir("fail");
        let path = dir.join("f.log");
        let log = ReopenableFile::open(&path).unwrap();
        log.make_writer().write_all(b"one\n").unwrap();

        // Removing the directory makes the re-open fail (the file cannot be
        // re-created), while the already-open descriptor stays writable.
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(log.reopen().is_err());
        log.make_writer().write_all(b"two\n").unwrap();
    }

    #[test]
    fn clones_share_the_descriptor() {
        let dir = temp_dir("clone");
        let path = dir.join("c.log");
        let log = ReopenableFile::open(&path).unwrap();
        let other = log.clone();
        std::fs::rename(&path, dir.join("c.log.1")).unwrap();
        other.reopen().unwrap();
        log.make_writer().write_all(b"x\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
