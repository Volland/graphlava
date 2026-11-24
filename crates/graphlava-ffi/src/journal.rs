//! The iOS extension write journal ([[16-api#Mobile bindings]], ADR 0043).
//!
//! An app extension must not open the database file ([[adr:0002]]). When the
//! owner app is not reachable it appends each write request to its own
//! journal, `<appgroup>/graphlava/<extension-id>.journal`; the owner applies
//! the journal on its next open.
//!
//! File layout (little-endian):
//!
//! ```text
//! header := magic "GLVJ" | version:u32 (=1) | base_seq:u64
//! frame  := len:u32 | seq:u64 | crc32c:u32 | payload[len]
//! ```
//!
//! `payload` is an encoded request ([`crate::request`]); `crc32c` covers
//! `seq` and `payload`. Sequence numbers grow by one per frame, starting after
//! `base_seq`, which the owner raises when it truncates the journal so that
//! numbers are never reused.
//!
//! Applying: each complete frame runs as one write request through
//! [`graphlava::Database::submit_once`], which records `seq` under the
//! metadata key `journal_seq:<key>` in the same transaction, so a frame is
//! applied exactly once even when the owner crashes midway. The first frame
//! that is incomplete or fails its checksum ends the journal: it and every
//! byte after it are a torn tail and are dropped. After a durable flush the
//! journal is truncated to its header.
//!
//! Appends and applies take an exclusive `flock` on the journal (Unix), held
//! only for the duration of the call.

// @tg: implements:: [[openspec:mobile-bindings#iOS extension writes]]
// @tg: implements:: [[adr:0043]]

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use graphlava::{Database, Durability, Error, RequestBody, WriteRequest};

use crate::request::Request;

pub(crate) const MAGIC: &[u8; 4] = b"GLVJ";
pub(crate) const VERSION: u32 = 1;
pub(crate) const HEADER_LEN: usize = 16;
pub(crate) const FRAME_HEAD: usize = 16;
/// Frames larger than this are treated as torn.
pub(crate) const MAX_FRAME: usize = 64 << 20;

/// What one apply did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Frames applied now.
    pub applied: u64,
    /// Frames already applied before (a crash after apply, before truncation).
    pub skipped: u64,
    /// Frames whose request failed (bad request, conflict); recorded as
    /// applied so they are not retried.
    pub failed: u64,
    /// Bytes of a torn tail that were dropped (0 when the journal was clean).
    pub dropped_bytes: u64,
}

// ---- CRC32C (Castagnoli), table-driven -----------------------------------------

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0x82F6_3B78 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32C of the concatenation of `parts`.
pub fn crc32c(parts: &[&[u8]]) -> u32 {
    let mut c = !0u32;
    for p in parts {
        for &b in *p {
            c = CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    !c
}

// ---- frames ----------------------------------------------------------------------

/// Encodes one frame.
pub fn frame(seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(FRAME_HEAD + payload.len());
    f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.extend_from_slice(&seq.to_le_bytes());
    f.extend_from_slice(&crc32c(&[&seq.to_le_bytes(), payload]).to_le_bytes());
    f.extend_from_slice(payload);
    f
}

pub fn header(base_seq: u64) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..4].copy_from_slice(MAGIC);
    h[4..8].copy_from_slice(&VERSION.to_le_bytes());
    h[8..].copy_from_slice(&base_seq.to_le_bytes());
    h
}

/// A parsed journal: the base sequence number, the complete frames in order,
/// and the length of the valid prefix (header plus complete frames).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub base_seq: u64,
    pub frames: Vec<(u64, Vec<u8>)>,
    pub valid_len: usize,
}

/// Parses journal bytes. An empty input is an empty journal. A wrong magic or
/// version is an error (the file is not touched). The first frame that is
/// incomplete, too large, out of sequence or fails its checksum ends the
/// valid prefix.
pub fn parse(b: &[u8]) -> Result<Parsed, Error> {
    if b.is_empty() {
        return Ok(Parsed {
            valid_len: 0,
            ..Default::default()
        });
    }
    if b.len() < HEADER_LEN {
        // A header torn while the journal was being created.
        return Ok(Parsed::default());
    }
    if &b[..4] != MAGIC {
        return Err(Error::FormatMismatch("not a graphlava journal".into()));
    }
    let v = u32::from_le_bytes(b[4..8].try_into().expect("4"));
    if v != VERSION {
        return Err(Error::FormatMismatch(format!(
            "journal version {v}, this build reads {VERSION}"
        )));
    }
    let base_seq = u64::from_le_bytes(b[8..16].try_into().expect("8"));
    let mut out = Parsed {
        base_seq,
        frames: Vec::new(),
        valid_len: HEADER_LEN,
    };
    let mut at = HEADER_LEN;
    let mut expect = base_seq.checked_add(1);
    while b.len() - at >= FRAME_HEAD {
        let len = u32::from_le_bytes(b[at..at + 4].try_into().expect("4")) as usize;
        let seq = u64::from_le_bytes(b[at + 4..at + 12].try_into().expect("8"));
        let crc = u32::from_le_bytes(b[at + 12..at + 16].try_into().expect("4"));
        if len > MAX_FRAME || b.len() - at - FRAME_HEAD < len || Some(seq) != expect {
            break;
        }
        let payload = &b[at + FRAME_HEAD..at + FRAME_HEAD + len];
        if crc32c(&[&seq.to_le_bytes(), payload]) != crc {
            break;
        }
        out.frames.push((seq, payload.to_vec()));
        at += FRAME_HEAD + len;
        out.valid_len = at;
        expect = seq.checked_add(1);
    }
    Ok(out)
}

fn lock(f: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `flock` on a valid open descriptor; no memory is passed.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    #[cfg(not(unix))]
    let _ = f;
    Ok(())
}

fn open_locked(path: &Path, create: bool) -> std::io::Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .open(path)?;
    lock(&f)?;
    Ok(f)
}

fn read_all(f: &mut File) -> std::io::Result<Vec<u8>> {
    let mut b = Vec::new();
    f.seek(SeekFrom::Start(0))?;
    f.read_to_end(&mut b)?;
    Ok(b)
}

/// Appends one encoded request as a frame and syncs the file; returns the
/// frame's sequence number. A torn tail left by an earlier killed append is
/// cut off first.
pub fn append(path: &Path, payload: &[u8]) -> Result<u64, Error> {
    if payload.len() > MAX_FRAME {
        return Err(Error::Invalid(
            "request too large for a journal frame".into(),
        ));
    }
    let mut f = open_locked(path, true)?;
    let b = read_all(&mut f)?;
    let p = parse(&b)?;
    if p.valid_len < HEADER_LEN {
        f.set_len(0)?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header(0))?;
    } else if p.valid_len < b.len() {
        f.set_len(p.valid_len as u64)?;
    }
    let last = p.frames.last().map_or(p.base_seq, |(s, _)| *s);
    let seq = last.checked_add(1).ok_or(Error::Full)?;
    f.seek(SeekFrom::End(0))?;
    f.write_all(&frame(seq, payload))?;
    f.sync_all()?;
    Ok(seq)
}

fn engine_failure(e: &Error) -> bool {
    matches!(
        e,
        Error::Io(_) | Error::Corrupted(_) | Error::Full | Error::ReadOnly | Error::Locked
    )
}

/// Applies the journal at `path` to `db` under the exactly-once key `key`
/// (the extension identifier); see the module documentation. `after_frame`
/// runs after each frame commits (the crash tests kill the process there).
pub fn apply_with(
    db: &Database,
    path: &Path,
    key: &str,
    mut after_frame: impl FnMut(u64),
) -> Result<Report, Error> {
    let mut f = match open_locked(path, false) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Report::default()),
        Err(e) => return Err(e.into()),
    };
    let b = read_all(&mut f)?;
    let p = parse(&b)?;
    let mut rep = Report {
        dropped_bytes: (b.len() - p.valid_len) as u64,
        ..Default::default()
    };
    let mut last = p.base_seq.max(db.once_seq(key)?);
    for (seq, payload) in &p.frames {
        let res = Request::decode(payload)
            .and_then(|r| r.to_body())
            .and_then(|body| db.submit_once(key, *seq, &body, Durability::Ordered));
        match res {
            Ok(true) => rep.applied += 1,
            Ok(false) => rep.skipped += 1,
            Err(e) if engine_failure(&e) => return Err(e),
            Err(_) => {
                // The request itself is bad; record its number so it is not
                // retried forever.
                db.submit_once(
                    key,
                    *seq,
                    &RequestBody::Ops(WriteRequest::default()),
                    Durability::Ordered,
                )?;
                rep.failed += 1;
            }
        }
        last = last.max(*seq);
        after_frame(*seq);
    }
    db.flush()?;
    if !b.is_empty() {
        f.set_len(0)?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header(last))?;
        f.sync_all()?;
    }
    Ok(rep)
}

pub fn apply(db: &Database, path: &Path, key: &str) -> Result<Report, Error> {
    apply_with(db, path, key, |_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_check_value() {
        // The standard check value of CRC-32C.
        assert_eq!(crc32c(&[b"123456789"]), 0xE306_9283);
        assert_eq!(crc32c(&[b"1234", b"56789"]), 0xE306_9283);
    }

    #[test]
    fn parse_stops_at_the_first_bad_frame() {
        let mut b = header(4).to_vec();
        b.extend(frame(5, b"one"));
        b.extend(frame(6, b"two"));
        let good = b.len();
        b.extend(frame(7, b"three"));
        let n = b.len();
        b[n - 1] ^= 1; // checksum failure in the last frame
        let p = parse(&b).unwrap();
        assert_eq!(p.base_seq, 4);
        assert_eq!(p.frames, vec![(5, b"one".to_vec()), (6, b"two".to_vec())]);
        assert_eq!(p.valid_len, good);
        // Out-of-sequence frames end the prefix too.
        let mut c = header(0).to_vec();
        c.extend(frame(2, b"x"));
        assert!(parse(&c).unwrap().frames.is_empty());
        assert!(parse(b"nope-nope-nope-!").is_err());
        assert_eq!(parse(b"GLV").unwrap(), Parsed::default());
    }
}
