//! A record of what the engine did, and when.
//!
//! # Why a file and not a socket
//!
//! Because there is no daemon. Docker's `events` is a stream from a
//! process that is always there; ours has to work when the only thing
//! running is a supervisor that was started an hour ago by a CLI that has
//! long exited. A file in the state directory is readable by anyone who
//! can read the state directory, survives every writer exiting, and needs
//! nothing to be running to be read.
//!
//! # Why appends need no lock
//!
//! Every writer opens with `O_APPEND` and writes one line in one `write`.
//! POSIX guarantees an `O_APPEND` write is atomic with respect to the file
//! offset, and Linux guarantees a single `write` to a regular file under
//! `PIPE_BUF` (4096) is not interleaved. [`MAX_LINE`] is enforced below so
//! that stays true: a line that would exceed it is truncated rather than
//! risking two supervisors' events being spliced into one unparseable row.
//!
//! The consequence worth stating: this is a *log*, not a queue. Nothing
//! acknowledges, nothing replays, and a reader that was not watching when
//! something happened reads it afterwards or not at all.

use std::io::Write;
use std::path::Path;

use crate::state::Paths;

/// The largest line we will write, to stay inside the atomicity guarantee.
pub const MAX_LINE: usize = 3800;

/// How much history is kept before the file is rotated.
///
/// Rotated by truncation, not by renaming: a reader following the file by
/// offset would silently read the wrong file after a rename, and the
/// events here are small enough that losing the oldest is the right
/// trade against growing without bound on a busy host.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;

pub fn path(paths: &Paths) -> std::path::PathBuf {
    paths.containers().parent().map_or_else(
        || std::path::PathBuf::from("events.jsonl"),
        |p| p.join("events.jsonl"),
    )
}

/// Append one event. Never fails the caller: an engine that refuses to
/// start a container because it could not write a log line would be
/// trading a working container for a tidy audit trail.
pub fn record(paths: &Paths, kind: &str, id: &str, detail: serde_json::Value) {
    let line = serde_json::json!({
        "at": crate::state::now(),
        "kind": kind,
        "id": id,
        "detail": detail,
    });
    let mut text = line.to_string();
    if text.len() > MAX_LINE {
        // Truncating the detail rather than the line keeps it parseable,
        // which matters more than keeping it complete.
        text = serde_json::json!({
            "at": crate::state::now(),
            "kind": kind,
            "id": id,
            "detail": {"truncated": true},
        })
        .to_string();
    }
    text.push('\n');
    let _ = append(&path(paths), text.as_bytes());
}

fn append(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(m) = std::fs::metadata(p)
        && m.len() > MAX_BYTES
    {
        let _ = std::fs::write(p, b"");
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(p)?;
    // One call. Split into two and another writer's line can land between
    // them, and both rows become unparseable.
    f.write_all(bytes)
}

/// Events, oldest first, optionally only those at or after `since`.
///
/// Unparseable lines are skipped rather than reported. A torn line can
/// only come from a writer that crashed mid-`write`, and refusing to show
/// the other ten thousand events because of it would be the wrong answer.
pub fn read(paths: &Paths, since: u64, limit: usize) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(path(paths)) else {
        return Vec::new();
    };
    let mut out: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["at"].as_u64().unwrap_or(0) >= since)
        .collect();
    if out.len() > limit {
        out.drain(..out.len() - limit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(name: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("rustainer-events-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        let p = Paths::new(root);
        p.ensure_root().unwrap();
        p
    }

    #[test]
    fn events_round_trip_in_order() {
        let p = paths("roundtrip");
        record(&p, "create", "abc", serde_json::json!({"image": "alpine"}));
        record(&p, "start", "abc", serde_json::json!({}));
        record(&p, "die", "abc", serde_json::json!({"exit": 0}));
        let all = read(&p, 0, 100);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0]["kind"], "create");
        assert_eq!(all[2]["detail"]["exit"], 0);
        assert_eq!(all[0]["id"], "abc");
    }

    #[test]
    fn a_line_too_long_stays_parseable() {
        // The atomicity guarantee is what keeps two supervisors' events
        // from being spliced together, and it only holds below PIPE_BUF.
        let p = paths("toolong");
        record(
            &p,
            "pull",
            "abc",
            serde_json::json!({ "huge": "x".repeat(MAX_LINE * 2) }),
        );
        let all = read(&p, 0, 100);
        assert_eq!(all.len(), 1, "the line must still parse");
        assert_eq!(all[0]["detail"]["truncated"], true);
    }

    #[test]
    fn a_torn_line_does_not_hide_the_others() {
        let p = paths("torn");
        record(&p, "start", "a", serde_json::json!({}));
        append(&path(&p), b"{\"at\": 1, \"kind\": \"tr\n").unwrap();
        record(&p, "die", "b", serde_json::json!({}));
        let all = read(&p, 0, 100);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1]["id"], "b");
    }

    #[test]
    fn since_filters_and_limit_keeps_the_newest() {
        let p = paths("since");
        for i in 0..5 {
            record(&p, "tick", &i.to_string(), serde_json::json!({}));
        }
        assert_eq!(read(&p, 0, 3).len(), 3);
        // The newest three, not the oldest: a tail is what anyone wants.
        assert_eq!(read(&p, 0, 3)[2]["id"], "4");
        assert!(read(&p, crate::state::now() + 10, 100).is_empty());
    }
}
