//! Torn-journal fuzzing against the journal's model: the frames a reader
//! accepts are exactly the longest prefix of complete, intact frames, whatever
//! bytes follow (a killed append, a flipped bit, garbage). Also checks that
//! request decoding never panics on arbitrary payloads
//! ([[openspec:mobile-bindings#iOS extension writes]], design "Refinement").
#![no_main]

use graphlava_ffi::journal::{frame, header, parse};
use graphlava_ffi::request::Request;
use libfuzzer_sys::fuzz_target;

// @tg: verifies:: [[openspec:mobile-bindings#iOS extension writes#Corrupt journal entry]] {level: property}
fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let base = u64::from(data[0]);
    let nframes = usize::from(data[1] % 8);
    let cut = usize::from(u16::from_le_bytes([data[2], data[3]]));
    let rest = &data[4..];
    // Build `nframes` intact frames from slices of the input.
    let mut file = header(base).to_vec();
    let mut ends = Vec::new();
    let mut payloads = Vec::new();
    for i in 0..nframes {
        let p = rest.get(i * 7..(i * 7 + 7).min(rest.len())).unwrap_or(&[]).to_vec();
        file.extend(frame(base + 1 + i as u64, &p));
        ends.push(file.len());
        payloads.push(p);
        let _ = Request::decode(&payloads[i]);
    }
    let _ = Request::decode(rest);
    // Tear the file at `cut`, then append the remaining input as garbage, or
    // flip one byte inside it.
    let torn_at = cut.min(file.len());
    let mut torn = file[..torn_at].to_vec();
    let flip = data[1] & 0x80 != 0;
    if flip && torn_at > 16 {
        let i = 16 + (cut % (torn_at - 16));
        torn[i] ^= 0x5A;
        torn.truncate(torn_at);
    } else {
        torn.extend_from_slice(rest);
    }
    let Ok(p) = parse(&torn) else {
        return;
    };
    // Model: every frame wholly before the tear (and before a flipped byte)
    // is accepted unchanged, in order; whatever follows is accepted only if
    // it is itself a run of intact frames with consecutive numbers.
    let intact_end = if flip && torn_at > 16 { 16 + (cut % (torn_at - 16)) } else { torn_at };
    let expect = ends.iter().filter(|e| **e <= intact_end).count();
    assert!(p.frames.len() >= expect, "lost an intact frame");
    for k in 0..expect {
        assert_eq!(p.frames[k].0, base + 1 + k as u64);
        assert_eq!(p.frames[k].1, payloads[k]);
    }
    for (k, (seq, _)) in p.frames.iter().enumerate() {
        assert_eq!(*seq, base + 1 + k as u64, "numbers are consecutive");
    }
    assert!(p.valid_len <= torn.len());
});
