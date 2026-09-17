//! Timestamped config backups (create / list / restore / prune).
//!
//! Layout: backups live in <config dir>/backups and every file is named
//! config-<UTC yyyymmdd-HHMMSS>-<tag>.yaml. The stamp is ASCII and zero padded, so plain byte
//! order of the name is also chronological order - that is what list() and prune() rely on.
//! Plain library module: no HTTP, no config semantics. Validating a restored config is the
//! caller's job.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::timeutil;

/// Directory holding the backups of a config file.
///
/// Next to the config file (<config dir>/backups), not beside the binary or in an OS data dir:
/// the config is the one thing a --config user points at, so its own directory is the only
/// location that is right for every deployment (installed service, portable copy, tests) and a
/// backup stays discoverable without knowing how the process was started.
pub fn backup_dir(config_path: &Path) -> PathBuf {
    match config_path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.join(BACKUP_DIR_NAME),
        _ => PathBuf::from(BACKUP_DIR_NAME),
    }
}

#[derive(Debug, Clone)]
pub struct BackupEntry {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub created_at: i64,
}

impl BackupEntry {
    /// UI/API shape: ISO8601 timestamp plus an age, so a client never has to do date math.
    pub fn to_json(&self, now: i64) -> Value {
        json!({
            "name": self.name,
            "bytes": self.bytes,
            "created_at": timeutil::iso8601(self.created_at),
            "age_secs": (now - self.created_at).max(0),
        })
    }
}

const BACKUP_DIR_NAME: &str = "backups";
const PREFIX: &str = "config-";
const SUFFIX: &str = ".yaml";
const MAX_TAG: usize = 32;
/// Tag of the safety copy taken before a restore overwrites the live config.
const PRE_RESTORE_TAG: &str = "pre-restore";

/// Copy the config file into the backup directory as config-<utc>-<tag>.yaml.
/// Never overwrites: an existing name gets a -2, -3, ... suffix.
pub fn create(config_path: &Path, tag: &str) -> Result<BackupEntry, String> {
    let dir = backup_dir(config_path);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create backup dir {}: {}", dir.display(), e))?;
    let bytes = std::fs::read(config_path)
        .map_err(|e| format!("cannot read {}: {}", config_path.display(), e))?;
    // yyyymmdd-HHMMSS in UTC, built from the same civil-time helpers the rest of the crate uses.
    // ASCII, filesystem-safe (no colons) and fixed width, so the name sorts chronologically as a
    // plain string and the missing year field of a unix second is never needed again.
    let c = timeutil::civil_from_unix(crate::util::now_secs());
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        c.year, c.month, c.day, c.hour, c.min, c.sec
    );
    let base = format!("{}{}-{}{}", PREFIX, stamp, sanitise_tag(tag), SUFFIX);
    let stem = &base[..base.len() - SUFFIX.len()];
    let mut name = base.clone();
    let mut n = 2u32;
    while dir.join(&name).exists() {
        if n > 10_000 {
            return Err(format!("cannot find a free backup name for {}", base));
        }
        // "-2" sorts before the bare name as text, so list() re-keys on the counter (see rank());
        // here the suffix only has to be unique and stay inside the backup pattern.
        name = format!("{}-{}{}", stem, n, SUFFIX);
        n += 1;
    }
    let path = dir.join(&name);
    std::fs::write(&path, &bytes)
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    let entry = BackupEntry {
        name,
        path,
        bytes: bytes.len() as u64,
        created_at: crate::util::now_secs(),
    };
    crate::log_info!(
        "config backup created: {} ({} bytes)",
        entry.path.display(),
        entry.bytes
    );
    Ok(entry)
}

/// Every config-*.yaml in the backup directory, newest first. A missing directory is not an
/// error: having no backups is a normal state.
pub fn list(config_path: &Path) -> Vec<BackupEntry> {
    let dir = backup_dir(config_path);
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<BackupEntry> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // Names we do not own (and cannot date) are never listed, so prune() cannot delete them.
        let created_at = match parse_name(&name) {
            Some(t) => t,
            None => continue,
        };
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        out.push(BackupEntry {
            name,
            path: e.path(),
            bytes: meta.len(),
            created_at,
        });
    }
    // Newest first. Sorting the raw name is not enough: within one second a collision suffix
    // ("...-manual-2.yaml") sorts BEFORE the bare name and "-10" before "-9", so the key moves the
    // counter into a number (rank()) and the comparison is reversed. Ties (equal key AND equal
    // name) cannot happen, names are unique inside a directory.
    out.sort_by(|a, b| rank(&b.name).cmp(&rank(&a.name)));
    out
}

/// Delete every backup older than the newest `keep`; returns how many files were removed.
/// keep == 0 is legal and removes all of them. Files not matching the config-*.yaml pattern are
/// never touched.
pub fn prune(config_path: &Path, keep: usize) -> Result<usize, String> {
    let mut removed = 0usize;
    for e in list(config_path).iter().skip(keep) {
        std::fs::remove_file(&e.path)
            .map_err(|err| format!("cannot delete backup {}: {}", e.path.display(), err))?;
        removed += 1;
    }
    if removed > 0 {
        crate::log_info!(
            "pruned {} backup(s), kept the newest {}",
            removed,
            keep
        );
    }
    Ok(removed)
}

/// Replace the live config with backup `name`.
///
/// The name is resolved strictly to a regular file inside the backup directory: separators, a
/// drive prefix and any parent component are rejected before the filesystem is touched (path
/// traversal), and anything not matching the backup pattern is refused. The bytes are copied
/// verbatim - this never parses or validates the config, that is the caller's job.
pub fn restore(config_path: &Path, name: &str) -> Result<(), String> {
    let dir = backup_dir(config_path);
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name.contains("..")
        || Path::new(name).components().count() != 1
    {
        return Err(format!("invalid backup name {:?}", name));
    }
    if parse_name(name).is_none() {
        return Err(format!(
            "invalid backup name {:?} (expected config-<yyyymmdd>-<hhmmss>-<tag>.yaml)",
            name
        ));
    }
    let src = dir.join(name);
    let meta = std::fs::metadata(&src)
        .map_err(|e| format!("cannot read backup {}: {}", src.display(), e))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", src.display()));
    }
    // Safety copy first: whatever happens next, the pre-restore state is already on disk. A byte
    // copy is enough here; nothing reads the live config while it is replaced below.
    create(config_path, PRE_RESTORE_TAG)?;
    let bytes = std::fs::read(&src)
        .map_err(|e| format!("cannot read backup {}: {}", src.display(), e))?;
    write(config_path, &String::from_utf8_lossy(&bytes))?;
    crate::log_info!("config restored from {}", src.display());
    Ok(())
}

/// Read the live config (export).
pub fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))
}

/// Atomically replace `path` with `text`: parent directories are created if missing, the data
/// goes to a temp file in the same directory, is fsynced, then renamed over the target.
pub fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
        }
    }
    let tmp = temp_path(path);
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot replace {}: {}", path.display(), e));
    }
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string());
    name.push_str(".tmp");
    match path.parent() {
        Some(d) => d.join(name),
        None => PathBuf::from(name),
    }
}

/// [a-z0-9_-] only, at most 32 chars, empty becomes "manual".
/// The tag never carries the timestamp, so the same tag twice in one second collides on the
/// name and takes the -2 suffix.
fn sanitise_tag(tag: &str) -> String {
    let mut out = String::new();
    for c in tag.chars() {
        let c = c.to_ascii_lowercase();
        if (c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') && out.len() < MAX_TAG {
            out.push(c);
        }
    }
    if out.is_empty() {
        return "manual".to_string();
    }
    out
}

/// config-<yyyymmdd>-<hhmmss>-<tag>.yaml -> unix seconds (None = not a backup we own).
/// Parsed with timeutil, matching how the names are written - no date crate involved.
fn parse_name(name: &str) -> Option<i64> {
    let rest = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let (date, rest) = rest.split_once('-')?;
    let (time, tag) = rest.split_once('-')?;
    if tag.is_empty() || !tag.chars().all(is_tag_char) {
        return None;
    }
    if date.len() != 8 || time.len() != 6 {
        return None;
    }
    if !date.chars().chain(time.chars()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let year: i64 = date.get(0..4)?.parse().ok()?;
    let month: u32 = date.get(4..6)?.parse().ok()?;
    let day: u32 = date.get(6..8)?.parse().ok()?;
    let hour: i64 = time.get(0..2)?.parse().ok()?;
    let min: i64 = time.get(2..4)?.parse().ok()?;
    let sec: i64 = time.get(4..6)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    Some(timeutil::days_from_civil(year, month, day) * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_tag_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'
}

/// Sort key of one backup: the name with its collision counter moved into a numeric field, so
/// "-9" sorts before "-10" and a same-second pair keeps its creation order.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Fixup {
    stem: String,
    n: u64,
}

/// Order key of a backup name: the full name plus a trailing "-<number>" collision counter
/// split off so it compares as a number. The bare name carries n = 0, i.e. it is the oldest of
/// its group. Names that are not ours keep n = 0 and simply compare as text.
fn rank(name: &str) -> Fixup {
    // Only a name that actually matches the backup pattern may be split: the timestamp itself
    // contains '-', so a naive rsplit would cut "20260916-010203-manual.yaml" apart.
    let stem = name.strip_suffix(SUFFIX);
    if let Some(stem) = stem {
        if parse_name(name).is_some() {
            if let Some((head, num)) = stem.rsplit_once('-') {
                if let Some(n) = parse_counter(num) {
                    return Fixup {
                        stem: head.to_string(),
                        n,
                    };
                }
            }
            return Fixup {
                stem: stem.to_string(),
                n: 0,
            };
        }
    }
    Fixup {
        stem: name.to_string(),
        n: 0,
    }
}

/// Trailing "-N" collision counters are compared as numbers, so -9 sorts before -10.
fn parse_counter(s: &str) -> Option<u64> {
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// Unique scratch directory per test: temp_dir + pid + clock + counter.
    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ar-ocg-cfgbackup-{}-{}-{}",
            std::process::id(),
            crate::util::real_now_secs(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_cfg(dir: &Path, text: &str) -> PathBuf {
        let p = dir.join("config.yaml");
        std::fs::write(&p, text).unwrap();
        p
    }

    /// The yyyymmdd-HHMMSS stamp create() uses for "now".
    fn stamp_now() -> String {
        let c = timeutil::civil_from_unix(crate::util::now_secs());
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            c.year, c.month, c.day, c.hour, c.min, c.sec
        )
    }

    #[test]
    fn create_then_list_is_newest_first_with_collision_suffix() {
        let d = tmpdir();
        let cfg = write_cfg(&d, "server:\n  port: 1\n");
        assert_eq!(backup_dir(&cfg), d.join("backups"));

        let stamp = stamp_now();
        let a = create(&cfg, "Manual Backup").unwrap();
        assert_eq!(a.bytes, 18);
        assert_eq!(a.name, format!("config-{}-manualbackup.yaml", stamp));
        assert!(a.path.is_file());

        // A collision (same tag, same second) must take the -2 suffix. The second call repeats the
        // FIRST call's tag, so the name only avoids colliding when the wall clock crossed into the
        // next second - then the expected name carries that second's stamp.
        let b = create(&cfg, "Manual Backup").unwrap();
        assert_ne!(b.name, a.name, "the second create must never overwrite the first");
        assert_eq!(b.name, format!("config-{}-manualbackup-2.yaml", stamp_now()));
        assert!(a.path.is_file() && b.path.is_file(), "both files must exist");

        let l = list(&cfg);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name, b.name, "newest (highest name) first");
        assert_eq!(l[1].name, a.name);
        assert!(l[1].created_at <= l[0].created_at);
    }

    #[test]
    fn list_on_missing_dir_is_empty() {
        let d = tmpdir();
        let cfg = d.join("config.yaml");
        assert!(!d.join("backups").exists());
        assert!(list(&cfg).is_empty());
        assert!(list(&d.join("nowhere").join("config.yaml")).is_empty());
    }

    #[test]
    fn prune_keeps_the_n_newest() {
        let d = tmpdir();
        let cfg = write_cfg(&d, "v0\n");
        for i in 0..5 {
            create(&cfg, &format!("t{}", i)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(list(&cfg).len(), 5);
        assert_eq!(prune(&cfg, 2).unwrap(), 3);
        let left: Vec<String> = list(&cfg).into_iter().map(|e| e.name).collect();
        assert_eq!(left.len(), 2);
        // Keep everything: nothing to do.
        assert_eq!(prune(&cfg, 5).unwrap(), 0);
        // keep == 0 is allowed and removes the rest.
        assert_eq!(prune(&cfg, 0).unwrap(), 2);
        assert!(list(&cfg).is_empty());
    }

    #[test]
    fn prune_never_touches_foreign_names() {
        let d = tmpdir();
        let cfg = write_cfg(&d, "x\n");
        create(&cfg, "keepme").unwrap();
        let foreign = backup_dir(&cfg).join("notes.txt");
        std::fs::write(&foreign, "do not delete").unwrap();
        let odd = backup_dir(&cfg).join("config-broken.yaml");
        std::fs::write(&odd, "also not ours").unwrap();
        prune(&cfg, 0).unwrap();
        assert!(foreign.is_file());
        assert!(odd.is_file());
    }

    #[test]
    fn restore_rejects_traversal_and_absolute_paths() {
        let d = tmpdir();
        let cfg = write_cfg(&d, "live\n");
        std::fs::write(d.join("outside.yaml"), "outside\n").unwrap();
        std::fs::write(d.join("sub"), "not a directory").unwrap();

        for bad in [
            "../config.yaml",
            "../outside.yaml",
            "sub/config.yaml",
            "sub\\config.yaml",
            "",
            "notes.txt",
        ] {
            assert!(restore(&cfg, bad).is_err(), "{} must be rejected", bad);
        }
        // Absolute path (drive-qualified here, rooted on unix).
        let abs = d.join("config-20200101-000000-abs.yaml");
        std::fs::write(&abs, "abs\n").unwrap();
        assert!(restore(&cfg, &abs.to_string_lossy()).is_err());
        // Nothing was touched and no safety copy was taken.
        assert_eq!(read(&cfg).unwrap(), "live\n");
        assert!(list(&cfg).is_empty());
    }

    #[test]
    fn restore_replaces_live_config_and_leaves_pre_restore_backup() {
        let d = tmpdir();
        let cfg = write_cfg(&d, "live-v1\n");
        let b = create(&cfg, "good").unwrap();
        write(&cfg, "live-v2\n").unwrap();

        restore(&cfg, &b.name).unwrap();
        assert_eq!(read(&cfg).unwrap(), "live-v1\n");

        let names: Vec<String> = list(&cfg).into_iter().map(|e| e.name).collect();
        assert!(names.contains(&b.name), "the source backup survives");
        let pre = list(&cfg)
            .into_iter()
            .find(|e| e.name.ends_with("-pre-restore.yaml"))
            .expect("a pre-restore safety copy must exist");
        assert_eq!(read(&pre.path).unwrap(), "live-v2\n");

        // A missing source is an error.
        assert!(restore(&cfg, "config-20200101-000000-nope.yaml").is_err());
    }

    /// Tag sanitising and the shape of the generated name.
    fn names_are_sanitised_and_dated() {
        let d = tmpdir();
        let cfg = write_cfg(&d, "x\n");
        let stamp = stamp_now();
        // Uppercase, spaces, punctuation and path noise are dropped; [a-z0-9_-] survives.
        let e = create(&cfg, "  Before Upgrade! ../evil_path-2  ").unwrap();
        assert_eq!(e.name, format!("config-{}-beforeupgradeevil_path-2.yaml", stamp));
        // An empty (or fully sanitised-away) tag becomes "manual".
        let m = create(&cfg, "!!!").unwrap();
        assert_eq!(m.name, format!("config-{}-manual.yaml", stamp));
        // At most 32 tag chars.
        let long = create(&cfg, &"A".repeat(100)).unwrap();
        assert_eq!(long.name, format!("config-{}-{}.yaml", stamp, "a".repeat(32)));
        assert!(sanitise_tag("") == "manual");
        assert_eq!(sanitise_tag("Ünïcode ok"), "ncodeok");
    }

    #[test]
    fn entry_json_shape() {
        let e = BackupEntry {
            name: "config-20260916-010203-manual.yaml".to_string(),
            path: PathBuf::from("x"),
            bytes: 12,
            created_at: 1789520523,
        };
        let v = e.to_json(1789520523 + 90);
        assert_eq!(v["name"], "config-20260916-010203-manual.yaml");
        assert_eq!(v["bytes"], 12);
        assert_eq!(v["created_at"], "2026-09-16T01:02:03Z");
        assert_eq!(v["age_secs"], 90);
        // A clock that moved backwards must not produce a negative age.
        assert_eq!(e.to_json(0)["age_secs"], 0);
    }

    #[test]
    fn read_write_is_atomic_and_creates_parents() {
        let d = tmpdir();
        let p = d.join("nested").join("config.yaml");
        write(&p, "hello\n").unwrap();
        assert_eq!(read(&p).unwrap(), "hello\n");
        write(&p, "again\n").unwrap();
        assert_eq!(read(&p).unwrap(), "again\n");
        assert!(!p.with_file_name("config.yaml.tmp").exists());
        assert!(read(&d.join("absent.yaml")).is_err());
    }

    #[test]
    fn parse_name_accepts_only_our_pattern() {
        assert_eq!(parse_name("config-20260916-010203-manual.yaml"), Some(1789520523));
        assert!(parse_name("config-20260916-010203-manual-2.yaml").is_some());
        assert!(parse_name("config.yaml").is_none());
        assert!(parse_name("config-20260916-010203.yaml").is_none());
        assert!(parse_name("other-20260916-010203-x.yaml").is_none());
        assert!(parse_name("config-20260916-010203-x.txt").is_none());
        assert!(parse_name("config-20261316-010203-x.yaml").is_none());
        // The collision suffix is part of the tag, so it still matches and keeps the base stamp.
        assert_eq!(
            parse_name("config-20260916-010203-manual-2.yaml"),
            parse_name("config-20260916-010203-manual.yaml")
        );
        // Counters compare as numbers, so 9 sorts before 10 and the bare name is the oldest.
        assert!(rank("config-20260916-010203-manual.yaml") < rank("config-20260916-010203-manual-2.yaml"));
        assert!(rank("config-20260916-010203-manual-9.yaml") < rank("config-20260916-010203-manual-10.yaml"));
        assert_eq!(rank("config-20260916-010203-manual-2.yaml").n, 2);
        assert_eq!(rank("config-20260916-010203-manual.yaml").n, 0);
    }
}
