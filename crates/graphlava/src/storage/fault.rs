//! A fault-injecting redb backend for the crash harness ([[19-verification]] L4).
//!
//! It counts every I/O call redb makes (`write`, `set_len`, `sync_data`) and
//! aborts the process at a chosen call, so a test can crash the writer at every
//! I/O step. In `PowerCut` mode, writes are held in memory until the next
//! `sync_data`, so an abort also loses everything not yet synced: the file then
//! holds exactly what a power loss would leave.

use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use redb::StorageBackend;

/// What a crash loses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashMode {
    /// The process dies; the OS keeps every write it already accepted.
    ProcessKill,
    /// The machine loses power; only data covered by a completed sync survives.
    PowerCut,
}

/// When and how to crash.
#[derive(Debug, Clone)]
pub struct FaultPlan {
    /// Abort when the I/O call with this 1-based index starts. 0 never aborts.
    pub abort_at: u64,
    pub mode: CrashMode,
    /// Optional file to which the number of I/O calls (first line) and of bytes
    /// written (second line) is written on drop, so a harness can learn how
    /// many crash points a run has and how much it wrote.
    pub count_file: Option<std::path::PathBuf>,
}

#[derive(Debug, Default)]
struct Pending {
    len: Option<u64>,
    writes: Vec<(u64, Vec<u8>)>,
}

#[derive(Debug)]
pub struct FaultBackend {
    inner: redb::backends::FileBackend,
    plan: FaultPlan,
    ops: AtomicU64,
    bytes: AtomicU64,
    pending: Mutex<Pending>,
}

impl FaultBackend {
    pub(crate) fn new(inner: redb::backends::FileBackend, plan: FaultPlan) -> Self {
        FaultBackend {
            inner,
            plan,
            ops: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            pending: Mutex::new(Pending::default()),
        }
    }

    fn tick(&self) {
        let n = self.ops.fetch_add(1, Ordering::SeqCst) + 1;
        if n == self.plan.abort_at {
            // The crash: no destructors, no flush of user-space buffers.
            std::process::abort();
        }
    }

    fn buffered(&self) -> bool {
        self.plan.mode == CrashMode::PowerCut
    }
}

impl Drop for FaultBackend {
    fn drop(&mut self) {
        if let Some(p) = &self.plan.count_file {
            let _ = std::fs::write(
                p,
                format!(
                    "{}\n{}\n",
                    self.ops.load(Ordering::SeqCst),
                    self.bytes.load(Ordering::SeqCst)
                ),
            );
        }
    }
}

impl StorageBackend for FaultBackend {
    fn len(&self) -> Result<u64, io::Error> {
        let p = self.pending.lock().expect("fault backend lock");
        match p.len {
            Some(l) => Ok(l),
            None => {
                let base = self.inner.len()?;
                let end = p
                    .writes
                    .iter()
                    .map(|(o, d)| o + d.len() as u64)
                    .max()
                    .unwrap_or(0);
                Ok(base.max(end))
            }
        }
    }

    fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, io::Error> {
        let p = self.pending.lock().expect("fault backend lock");
        let file_len = self.inner.len()?;
        let mut buf = vec![0u8; len];
        if offset < file_len {
            let avail = ((file_len - offset) as usize).min(len);
            buf[..avail].copy_from_slice(&self.inner.read(offset, avail)?);
        }
        for (o, d) in &p.writes {
            let (start, end) = (*o, *o + d.len() as u64);
            let (rs, re) = (offset, offset + len as u64);
            if end <= rs || start >= re {
                continue;
            }
            let from = start.max(rs);
            let to = end.min(re);
            buf[(from - rs) as usize..(to - rs) as usize]
                .copy_from_slice(&d[(from - start) as usize..(to - start) as usize]);
        }
        Ok(buf)
    }

    fn set_len(&self, len: u64) -> Result<(), io::Error> {
        self.tick();
        if self.buffered() {
            let mut p = self.pending.lock().expect("fault backend lock");
            for (o, d) in p.writes.iter_mut() {
                if *o >= len {
                    d.clear();
                } else if *o + d.len() as u64 > len {
                    d.truncate((len - *o) as usize);
                }
            }
            p.len = Some(len);
            Ok(())
        } else {
            self.inner.set_len(len)
        }
    }

    fn sync_data(&self, eventual: bool) -> Result<(), io::Error> {
        self.tick();
        if self.buffered() {
            let mut p = self.pending.lock().expect("fault backend lock");
            if let Some(l) = p.len.take() {
                self.inner.set_len(l)?;
            }
            for (o, d) in p.writes.drain(..) {
                if !d.is_empty() {
                    self.inner.write(o, &d)?;
                }
            }
        }
        self.inner.sync_data(eventual)
    }

    fn write(&self, offset: u64, data: &[u8]) -> Result<(), io::Error> {
        self.tick();
        self.bytes.fetch_add(data.len() as u64, Ordering::SeqCst);
        if self.buffered() {
            self.pending
                .lock()
                .expect("fault backend lock")
                .writes
                .push((offset, data.to_vec()));
            Ok(())
        } else {
            self.inner.write(offset, data)
        }
    }
}
