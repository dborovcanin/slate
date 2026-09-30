//! Note history stored as reverse line deltas.
//!
//! The current text of a note lives in `notes`; history rows hold older
//! versions, newest first. A row is either a full copy of its version or a
//! delta that turns the next newer version (or, for the newest row, the
//! current text) back into it. Every so often a row is stored as a full
//! checkpoint so rebuilding a version never walks a long chain and a damaged
//! delta cannot corrupt everything older than it.
//!
//! Everything here is pure; `storage` persists the rows.

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use similar::{capture_diff_slices_deadline, Algorithm, DiffOp};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Bound on diff work for pathological inputs; the delta stays correct, it
/// is just less minimal past the deadline.
const DIFF_DEADLINE: Duration = Duration::from_millis(200);
/// A full checkpoint after this many deltas in a row.
const MAX_DELTAS_BETWEEN_CHECKPOINTS: usize = 50;

/// One step of a line delta applied to a base text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Copy this many base lines.
    Keep(usize),
    /// Skip this many base lines.
    Delete(usize),
    /// Emit these lines.
    Insert(Vec<String>),
}

/// Line edits that turn a base text into a target text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Delta {
    pub ops: Vec<Op>,
}

/// A stored history row: a full version or a delta from the next newer one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    Full(String),
    Delta(Delta),
}

fn lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

impl Delta {
    fn push(&mut self, op: Op) {
        match (self.ops.last_mut(), op) {
            (_, Op::Keep(0) | Op::Delete(0)) => {}
            (_, Op::Insert(new)) if new.is_empty() => {}
            (Some(Op::Keep(n)), Op::Keep(m)) => *n += m,
            (Some(Op::Delete(n)), Op::Delete(m)) => *n += m,
            (Some(Op::Insert(lines)), Op::Insert(more)) => lines.extend(more),
            (_, op) => self.ops.push(op),
        }
    }

    /// Lines the delta emits and base lines it drops.
    pub fn line_counts(&self) -> (usize, usize) {
        self.ops.iter().fold((0, 0), |(ins, del), op| match op {
            Op::Keep(_) => (ins, del),
            Op::Delete(n) => (ins, del + n),
            Op::Insert(lines) => (ins + lines.len(), del),
        })
    }

    pub fn is_identity(&self) -> bool {
        self.ops.iter().all(|op| matches!(op, Op::Keep(_)))
    }
}

/// The delta that turns `base` into `target`, line by line. Common leading
/// and trailing lines are matched first, so a local edit in a large note
/// only diffs the changed region.
pub fn diff(base: &str, target: &str) -> Delta {
    let base = lines(base);
    let target = lines(target);
    let prefix = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let max_suffix = base.len().min(target.len()) - prefix;
    let suffix = base
        .iter()
        .rev()
        .zip(target.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let base_mid = &base[prefix..base.len() - suffix];
    let target_mid = &target[prefix..target.len() - suffix];

    let mut delta = Delta::default();
    delta.push(Op::Keep(prefix));
    let deadline = Some(Instant::now() + DIFF_DEADLINE);
    for op in capture_diff_slices_deadline(Algorithm::Myers, base_mid, target_mid, deadline) {
        let insert = |from: usize, len: usize| {
            Op::Insert(
                target_mid[from..from + len]
                    .iter()
                    .map(|line| line.to_string())
                    .collect(),
            )
        };
        match op {
            DiffOp::Equal { len, .. } => delta.push(Op::Keep(len)),
            DiffOp::Delete { old_len, .. } => delta.push(Op::Delete(old_len)),
            DiffOp::Insert {
                new_index, new_len, ..
            } => delta.push(insert(new_index, new_len)),
            DiffOp::Replace {
                old_len,
                new_index,
                new_len,
                ..
            } => {
                delta.push(Op::Delete(old_len));
                delta.push(insert(new_index, new_len));
            }
        }
    }
    delta.push(Op::Keep(suffix));
    delta
}

/// Applies `delta` to `base`. Fails when the delta does not fit the base,
/// which means the history chain is broken.
pub fn apply(delta: &Delta, base: &str) -> Result<String, String> {
    let base = lines(base);
    let mut out: Vec<&str> = Vec::with_capacity(base.len());
    let mut pos = 0usize;
    for op in &delta.ops {
        match op {
            Op::Keep(n) => {
                let end = pos + n;
                if end > base.len() {
                    return Err("history delta keeps past the end of its base".to_string());
                }
                out.extend_from_slice(&base[pos..end]);
                pos = end;
            }
            Op::Delete(n) => {
                pos += n;
                if pos > base.len() {
                    return Err("history delta deletes past the end of its base".to_string());
                }
            }
            Op::Insert(lines) => out.extend(lines.iter().map(String::as_str)),
        }
    }
    if pos != base.len() {
        return Err("history delta does not cover its base".to_string());
    }
    Ok(out.join("\n"))
}

// Encoding: one header line per op (`=N` keep, `-N` delete, `+N` insert)
// followed, for inserts, by the N inserted lines. Lines never contain `\n`
// because texts are split on it. The whole thing is zlib-compressed.

fn encode_ops(delta: &Delta) -> String {
    let mut out = String::new();
    for op in &delta.ops {
        match op {
            Op::Keep(n) => out.push_str(&format!("={n}\n")),
            Op::Delete(n) => out.push_str(&format!("-{n}\n")),
            Op::Insert(lines) => {
                out.push_str(&format!("+{}\n", lines.len()));
                for line in lines {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }
    out
}

fn decode_ops(text: &str) -> Result<Delta, String> {
    let bad = || "history delta is malformed".to_string();
    let mut delta = Delta::default();
    let mut rows = text.split('\n');
    while let Some(header) = rows.next() {
        if header.is_empty() {
            break;
        }
        let (kind, count) = header.split_at(1);
        let count: usize = count.parse().map_err(|_| bad())?;
        delta.ops.push(match kind {
            "=" => Op::Keep(count),
            "-" => Op::Delete(count),
            "+" => Op::Insert(
                (0..count)
                    .map(|_| rows.next().map(str::to_string).ok_or_else(bad))
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err(bad()),
        });
    }
    Ok(delta)
}

fn compress(text: &str) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(text.as_bytes())
        .expect("writing to a Vec cannot fail");
    encoder.finish().expect("writing to a Vec cannot fail")
}

fn decompress(bytes: &[u8]) -> Result<String, String> {
    let mut text = String::new();
    ZlibDecoder::new(bytes)
        .read_to_string(&mut text)
        .map_err(|e| format!("history payload is corrupt: {e}"))?;
    Ok(text)
}

impl Payload {
    pub fn is_full(&self) -> bool {
        matches!(self, Payload::Full(_))
    }

    /// Stored bytes: a kind byte (`F`/`D`) and the compressed body.
    pub fn encode(&self) -> Vec<u8> {
        let (kind, body) = match self {
            Payload::Full(text) => (b'F', compress(text)),
            Payload::Delta(delta) => (b'D', compress(&encode_ops(delta))),
        };
        let mut out = Vec::with_capacity(body.len() + 1);
        out.push(kind);
        out.extend(body);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        match bytes.split_first() {
            Some((b'F', body)) => Ok(Payload::Full(decompress(body)?)),
            Some((b'D', body)) => Ok(Payload::Delta(decode_ops(&decompress(body)?)?)),
            _ => Err("history payload has an unknown kind".to_string()),
        }
    }

    /// The version this row stores, given the next newer version.
    pub fn resolve(&self, newer: &str) -> Result<String, String> {
        match self {
            Payload::Full(text) => Ok(text.clone()),
            Payload::Delta(delta) => apply(delta, newer),
        }
    }
}

/// Whether the next stored row should be a full checkpoint rather than a
/// delta: after many deltas in a row, or once the deltas since the last
/// checkpoint weigh more than half the note.
pub fn should_checkpoint(
    deltas_since_full: usize,
    delta_bytes_since_full: usize,
    text_len: usize,
) -> bool {
    deltas_since_full >= MAX_DELTAS_BETWEEN_CHECKPOINTS
        || delta_bytes_since_full.saturating_mul(2) > text_len
}

/// Rebuilds the version stored at `index` in `rows` (newest first), starting
/// from `current`, the note's present text. Starts from the nearest full
/// checkpoint at or newer than `index`.
pub fn rebuild(current: &str, rows: &[Payload], index: usize) -> Result<String, String> {
    if index >= rows.len() {
        return Err("no such history version".to_string());
    }
    let start = rows[..=index]
        .iter()
        .rposition(Payload::is_full)
        .unwrap_or(0);
    let mut text = if start == 0 && !rows[0].is_full() {
        current.to_string()
    } else {
        String::new()
    };
    for row in &rows[start..=index] {
        text = row.resolve(&text)?;
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny deterministic generator so the randomized tests are repeatable.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    fn mutate(rng: &mut Rng, text: &str) -> String {
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(lines.len() + 1);
            match rng.below(4) {
                0 => lines.insert(at, format!("line {}", rng.next() % 1000)),
                1 if !lines.is_empty() => {
                    lines.remove(at.min(lines.len() - 1));
                }
                2 if !lines.is_empty() => {
                    let i = at.min(lines.len() - 1);
                    lines[i] = format!("{} edited {}", lines[i], rng.next() % 10);
                }
                _ => lines.push(String::new()),
            }
        }
        lines.join("\n")
    }

    #[test]
    fn diff_then_apply_round_trips_edge_cases() {
        for (base, target) in [
            ("", ""),
            ("", "a"),
            ("a", ""),
            ("a\nb\nc", "a\nb\nc"),
            ("a\nb\nc", "a\nx\nc"),
            ("a\nb\nc\n", "a\nb\nc"),
            ("\n\n", "\n"),
            ("x\ny", "y\nx"),
            ("same\nsame\nsame", "same\nsame"),
        ] {
            let delta = diff(base, target);
            assert_eq!(
                apply(&delta, base).unwrap(),
                target,
                "{base:?} -> {target:?}"
            );
        }
    }

    #[test]
    fn local_edit_in_a_large_text_stores_only_the_change() {
        let base: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
        let target = base.replacen("line 10000\n", "line 10000 edited\n", 1);
        let delta = diff(&base, &target);
        assert_eq!(delta.line_counts(), (1, 1));
        let encoded = Payload::Delta(delta).encode();
        assert!(encoded.len() < 100, "delta took {} bytes", encoded.len());
    }

    #[test]
    fn payloads_survive_encoding() {
        let delta = diff("a\nb\nc", "a\n+x\n=y\n-z\nc");
        for payload in [
            Payload::Delta(delta),
            Payload::Full("full\ntext".to_string()),
        ] {
            assert_eq!(Payload::decode(&payload.encode()).unwrap(), payload);
        }
        assert!(Payload::decode(b"Xjunk").is_err());
        assert!(Payload::decode(b"").is_err());
    }

    #[test]
    fn apply_rejects_a_delta_for_another_base() {
        let delta = diff("a\nb\nc", "a\nc");
        assert!(apply(&delta, "a\nb").is_err());
        assert!(apply(&delta, "a\nb\nc\nd").is_err());
    }

    #[test]
    fn random_histories_rebuild_every_version_with_checkpoints() {
        let mut rng = Rng(0x5eed);
        for _ in 0..40 {
            // versions[0] is the oldest; the last one is the current text.
            let mut versions = vec![(0..rng.below(30))
                .map(|i| format!("start {i}"))
                .collect::<Vec<_>>()
                .join("\n")];
            for _ in 0..rng.below(40) {
                let next = mutate(&mut rng, versions.last().unwrap());
                versions.push(next);
            }
            let current = versions.last().unwrap().clone();
            // Rows newest first: row i stores versions[len - 2 - i].
            let rows: Vec<Payload> = (0..versions.len() - 1)
                .rev()
                .map(|i| {
                    if rng.below(8) == 0 {
                        Payload::Full(versions[i].clone())
                    } else {
                        Payload::Delta(diff(&versions[i + 1], &versions[i]))
                    }
                })
                .collect();
            for (index, _) in rows.iter().enumerate() {
                let expected = &versions[versions.len() - 2 - index];
                assert_eq!(&rebuild(&current, &rows, index).unwrap(), expected);
            }
        }
    }

    #[test]
    fn checkpoint_rule_bounds_chains_and_weight() {
        assert!(!should_checkpoint(0, 0, 1000));
        assert!(!should_checkpoint(10, 400, 1000));
        assert!(should_checkpoint(10, 600, 1000));
        assert!(should_checkpoint(50, 0, 1_000_000));
    }
}
