//! HTTP GET with an on-disk cache.
//!
//! Each entry keeps the body and its `ETag`. A stale entry is revalidated with `If-None-Match`
//! (a 304 costs no body) and, when the request fails, served as is and marked `stale`. Errors are
//! never cached. A 429/503 with a short `Retry-After` and a network error or 5xx are retried once.
//! Entries unused for `MAX_UNUSED` are deleted at startup.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::mem::DropGuard;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::problem::{Kind, Problem};

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
/// The 6.5 MB item catalog is the largest response.
const MAX_BODY: u64 = 64 << 20;
const TIMEOUT: Duration = Duration::from_secs(30);
/// Longer waits are left to the caller, with the time in the error.
const MAX_RETRY_WAIT_S: u64 = 10;
/// Spacing between requests to one host, at most 10 per second: under the Deadlock API's
/// tightest documented limit (match metadata from storage, 100 per 10 s) and polite to the wiki.
const MIN_GAP: Duration = Duration::from_millis(100);
/// Most URLs are never asked for again once their time window or game version is past (the meta
/// tools' start time moves every hour, each patch brings a new 6.5 MB item catalog), so without
/// this the cache only grows.
const MAX_UNUSED: Duration = Duration::from_hours(30 * 24);

static NEXT_SLOT: Mutex<BTreeMap<String, Instant>> = Mutex::new(BTreeMap::new());

/// Waits for this host's next free slot. The slot is taken under the lock, the wait happens
/// outside it, so parallel tool calls queue up instead of bursting.
fn pace(url: &str) {
    let now = Instant::now();
    let start = NEXT_SLOT.lock().map_or(now, |mut slots| {
        let start = slots
            .get(host(url))
            .copied()
            .filter(|s| *s > now)
            .unwrap_or(now);
        slots.insert(host(url).to_owned(), start + MIN_GAP);
        start
    });
    std::thread::sleep(start.saturating_duration_since(now));
}

#[derive(Clone, Copy)]
pub enum Fresh {
    /// Never changes once fetched (a finished match).
    Forever,
    For(Duration),
}

/// Where a piece of an answer came from, so the model can tell the user how current it is.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Source {
    pub url: String,
    /// Unix time the data was fetched from the server.
    pub fetched_at: u64,
    pub from_cache: bool,
    /// The server could not be reached or refused, and this is an older copy.
    pub stale: bool,
    /// The request failed and nothing was cached, so the part of the answer it gives is missing
    /// (Steam names, place names, awards); `fetched_at` is when it was tried.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Source {
    /// A source whose request failed, for a part an answer can go without.
    pub fn failed(problem: &Problem) -> Self {
        Self {
            url: problem.instance.clone().unwrap_or_default(),
            fetched_at: unix_now(),
            from_cache: false,
            stale: false,
            error: Some(problem.message()),
        }
    }
}

/// The value of a part an answer can go without (`part` names it in the log). When its request
/// failed, the failure is logged and the answer gets an empty value and a failed source.
pub fn optional_part<T: Default>(
    part: &str,
    fetched: crate::problem::Result<(T, Source)>,
) -> (T, Source) {
    fetched.unwrap_or_else(|problem| {
        eprintln!("{}: {part}: {}", env!("CARGO_PKG_NAME"), problem.message());
        (T::default(), Source::failed(&problem))
    })
}

fn host(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or(url)
}

#[derive(Serialize, Deserialize)]
struct Meta {
    url: String,
    etag: Option<String>,
    fetched_at: u64,
}

fn dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join(env!("CARGO_PKG_NAME"))
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or_default(|d| d.as_secs())
}

/// FNV-1a: a cache key that stays the same across Rust versions, unlike `DefaultHasher`.
fn key(url: &str) -> String {
    let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// Percent-encodes everything but unreserved characters, for query values and URL paths.
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Ids as one comma-separated query value, e.g. `1,22`.
pub fn id_list(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

enum Attempt {
    Body {
        etag: Option<String>,
        body: Vec<u8>,
    },
    NotModified,
    Failed {
        problem: Problem,
        retry_in: Option<Duration>,
    },
}

/// The deadlock-api answers errors as `{"status": .., "error": ".."}` JSON or as plain text; a
/// gateway in front of it (Cloudflare) answers with an HTML page, whose title says what happened.
fn upstream_detail(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    if text.trim_start().starts_with('<') {
        // ASCII only, so byte offsets stay those of `text`.
        let lower = text.to_ascii_lowercase();
        let title = lower
            .find("<title>")
            .map(|i| i + "<title>".len())
            .and_then(|start| Some(text[start..start + lower[start..].find("</title>")?].trim()));
        return title.map_or_else(
            || "an HTML page instead of the API's answer".to_owned(),
            str::to_owned,
        );
    }
    let detail = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("error")
                .or_else(|| v.get("detail"))
                .and_then(|e| e.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.trim().to_owned());
    detail.chars().take(500).collect()
}

/// A network error: worth one retry.
fn network_error(url: &str, e: impl std::fmt::Display) -> Attempt {
    Attempt::Failed {
        problem: Problem::new(Kind::Unreachable, format!("{}: {e}", host(url))).with_instance(url),
        retry_in: Some(Duration::from_secs(1)),
    }
}

fn attempt(url: &str, etag: Option<&str>) -> Attempt {
    pace(url);
    let mut req = ureq::get(url).header("User-Agent", USER_AGENT);
    if let Some(etag) = etag {
        req = req.header("If-None-Match", etag);
    }
    let resp = match req
        .config()
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .build()
        .call()
    {
        Ok(r) => r,
        Err(e) => return network_error(url, e),
    };
    let status = resp.status().as_u16();
    if status == 304 {
        return Attempt::NotModified;
    }
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let new_etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut body = Vec::new();
    if let Err(e) = resp
        .into_body()
        .into_reader()
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
    {
        return network_error(url, e);
    }
    if body.len() as u64 > MAX_BODY {
        return Attempt::Failed {
            problem: Problem::new(
                Kind::InvalidResponse,
                format!("{} sent more than {} MiB", host(url), MAX_BODY >> 20),
            )
            .with_instance(url),
            retry_in: None,
        };
    }
    if (200..300).contains(&status) {
        return Attempt::Body {
            etag: new_etag,
            body,
        };
    }
    failure(url, status, retry_after, &body)
}

/// What a non-2xx answer means for the caller, and whether one retry may help.
fn failure(url: &str, status: u16, retry_after: Option<u64>, body: &[u8]) -> Attempt {
    let problem = |kind, detail: String| {
        Problem::new(kind, detail)
            .with_status(status)
            .with_instance(url)
    };
    let detail = upstream_detail(body);
    match status {
        429 | 503 if retry_after.is_some() || status == 429 => {
            let wait = retry_after.map_or_else(|| "later".to_owned(), |s| format!("in {s} s"));
            let mut p = problem(
                Kind::RateLimited,
                format!("{} asked to slow down; try again {wait}", host(url)),
            );
            if let Some(s) = retry_after {
                p = p.with("retry_after_s", s);
            }
            Attempt::Failed {
                problem: p,
                retry_in: retry_after
                    .filter(|&s| s <= MAX_RETRY_WAIT_S)
                    .map(Duration::from_secs),
            }
        }
        404 => Attempt::Failed {
            problem: problem(Kind::NotFound, detail),
            retry_in: None,
        },
        500..=599 => Attempt::Failed {
            problem: problem(Kind::Upstream, detail),
            retry_in: Some(Duration::from_secs(1)),
        },
        _ => Attempt::Failed {
            problem: problem(Kind::Upstream, detail),
            retry_in: None,
        },
    }
}

pub fn get(url: &str, fresh: Fresh) -> crate::problem::Result<(Vec<u8>, Source)> {
    let key = key(url);
    let (body_path, meta_path) = (
        dir().join(format!("{key}.body")),
        dir().join(format!("{key}.json")),
    );
    let mut cached = std::fs::read(&meta_path)
        .ok()
        .and_then(|m| serde_json::from_slice::<Meta>(&m).ok())
        .filter(|m| m.url == url)
        .and_then(|m| std::fs::read(&body_path).ok().map(|b| (m, b)));
    let source = |fetched_at, from_cache, stale| Source {
        url: url.to_owned(),
        fetched_at,
        from_cache,
        stale,
        error: None,
    };

    let still_fresh = |meta: &Meta| match fresh {
        Fresh::Forever => true,
        Fresh::For(ttl) => unix_now().saturating_sub(meta.fetched_at) < ttl.as_secs(),
    };
    if let Some((meta, body)) = cached.take_if(|(meta, _)| still_fresh(meta)) {
        touch(&[&meta_path, &body_path]);
        return Ok((body, source(meta.fetched_at, true, false)));
    }

    let etag = cached.as_ref().and_then(|(m, _)| m.etag.as_deref());
    let mut result = attempt(url, etag);
    if let Attempt::Failed {
        retry_in: Some(wait),
        ..
    } = &result
    {
        std::thread::sleep(*wait);
        result = attempt(url, etag);
    }
    match result {
        Attempt::Body { etag, body } => {
            let fetched_at = unix_now();
            save(
                &meta_path,
                &body_path,
                &Meta {
                    url: url.to_owned(),
                    etag,
                    fetched_at,
                },
                Some(&body),
            );
            Ok((body, source(fetched_at, false, false)))
        }
        Attempt::NotModified => match cached {
            Some((meta, body)) => {
                let fetched_at = unix_now();
                save(&meta_path, &body_path, &Meta { fetched_at, ..meta }, None);
                touch(&[&body_path]);
                Ok((body, source(fetched_at, true, false)))
            }
            None => Err(
                Problem::new(Kind::InvalidResponse, "304 without a cached copy").with_instance(url),
            ),
        },
        Attempt::Failed { problem, .. } if problem.kind == Kind::NotFound => Err(problem),
        Attempt::Failed { problem, .. } => match cached {
            Some((meta, body)) => {
                touch(&[&meta_path, &body_path]);
                Ok((body, source(meta.fetched_at, true, true)))
            }
            None => Err(problem),
        },
    }
}

/// Marks cache files as used now, so `prune` keeps them; a failure only lets them expire sooner.
fn touch(paths: &[&Path]) {
    for path in paths {
        let _ = std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(SystemTime::now()));
    }
}

/// Deletes the cache files not used for `MAX_UNUSED`, among them temp files a killed process left.
pub fn prune_cache() {
    prune(&dir(), SystemTime::now());
}

fn prune(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let unused = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > MAX_UNUSED));
        if unused {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Readers see the old file or the new one, never a half-written one, even if the process dies
/// mid-write. The temp name is unique per process and call, as parallel tool calls and several
/// server processes share the cache.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    // Removes the temp file on every early return, including a write cut short (disk full).
    let tmp = DropGuard::new(
        path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        )),
        |tmp| {
            let _ = std::fs::remove_file(tmp);
        },
    );
    std::fs::write(&*tmp, bytes)?;
    std::fs::rename(&*tmp, path)?;
    DropGuard::dismiss(tmp);
    Ok(())
}

/// A cache write that fails only costs a refetch, so it is not an error for the caller.
fn save(meta_path: &Path, body_path: &Path, meta: &Meta, body: Option<&[u8]>) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir())?;
        if let Some(body) = body {
            write_atomic(body_path, body)?;
        }
        write_atomic(meta_path, &serde_json::to_vec(meta)?)
    };
    if let Err(e) = write() {
        eprintln!("{}: cache write failed: {e}", env!("CARGO_PKG_NAME"));
    }
}

/// Fetches and parses JSON into `T`; a mismatch names the field path (`[3].hero: invalid type`).
pub fn get_json<T: serde::de::DeserializeOwned>(
    url: &str,
    fresh: Fresh,
) -> crate::problem::Result<(T, Source)> {
    let (body, source) = get(url, fresh)?;
    let value = serde_path_to_error::deserialize(&mut serde_json::Deserializer::from_slice(&body))
        .map_err(|e| {
            let path = e.path().to_string();
            Problem::new(Kind::InvalidResponse, format!("{path}: {}", e.into_inner()))
                .with_instance(url)
        })?;
    Ok((value, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("dmcp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.body");
        write_atomic(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prune_deletes_only_unused_files() {
        let dir = std::env::temp_dir().join(format!("dmcp-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (old, used) = (dir.join("old.body"), dir.join("used.body"));
        write_atomic(&old, b"x").unwrap();
        write_atomic(&used, b"x").unwrap();
        let now = SystemTime::now();
        let long_ago = now - MAX_UNUSED - Duration::from_secs(1);
        for path in [&old, &used] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(long_ago)
                .unwrap();
        }
        touch(&[&used]);
        prune(&dir, now);
        assert!(!old.exists() && used.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paces_requests_per_host() {
        // Lower bounds only: a slow CI runner may oversleep, never undersleep.
        let start = Instant::now();
        for _ in 0..3 {
            pace("https://pace.test/a");
        }
        assert!(start.elapsed() >= MIN_GAP * 2, "{:?}", start.elapsed());
        // Another host has its own slots: its first request doesn't wait.
        let other = Instant::now();
        pace("https://other.test/b");
        assert!(other.elapsed() < MIN_GAP, "{:?}", other.elapsed());
    }

    /// What each failed answer becomes, and which ones are retried once.
    #[test]
    fn failures_by_status() {
        let url = "https://api.test/x";
        let failed = |a: Attempt| match a {
            Attempt::Failed { problem, retry_in } => (
                problem.kind,
                problem.message(),
                problem.status,
                problem.instance,
                problem.extensions.get("retry_after_s").cloned(),
                retry_in,
            ),
            Attempt::Body { .. } | Attempt::NotModified => panic!("not a failure"),
        };
        let at = Some(url.to_owned());
        let body = br#"{"error":"gone"}"#;
        assert_eq!(
            failed(failure(url, 429, None, body)),
            (
                Kind::RateLimited,
                "Rate limited: api.test asked to slow down; try again later".to_owned(),
                Some(429),
                at.clone(),
                None,
                None
            )
        );
        let (kind, _, _, _, after, retry) = failed(failure(url, 503, Some(5), body));
        assert_eq!(
            (kind, after, retry),
            (
                Kind::RateLimited,
                Some(5.into()),
                Some(Duration::from_secs(5))
            )
        );
        let (kind, message, status, instance, _, retry) = failed(failure(url, 404, None, body));
        assert_eq!(
            (kind, message.ends_with(": gone"), status, instance, retry),
            (Kind::NotFound, true, Some(404), at.clone(), None)
        );
        let (kind, _, _, _, _, retry) = failed(failure(url, 503, None, body));
        assert_eq!(
            (kind, retry),
            (Kind::Upstream, Some(Duration::from_secs(1)))
        );
        let (kind, _, _, _, _, retry) = failed(failure(url, 400, None, body));
        assert_eq!((kind, retry), (Kind::Upstream, None));
        let (kind, message, status, instance, _, retry) = failed(network_error(url, "timed out"));
        assert_eq!(
            (
                kind,
                message.ends_with(": api.test: timed out"),
                status,
                instance,
                retry
            ),
            (
                Kind::Unreachable,
                true,
                None,
                at,
                Some(Duration::from_secs(1))
            )
        );
    }

    /// A failed optional part leaves an empty value and a failed source; a fetched one passes.
    #[test]
    fn optional_parts_fail_softly() {
        let problem = Problem::new(Kind::Unreachable, "down").with_instance("https://api.test/m");
        let (value, source): (Vec<u8>, Source) = optional_part("map", Err(problem));
        assert_eq!(value, Vec::<u8>::new());
        assert_eq!(
            (source.url.as_str(), source.error.as_deref()),
            (
                "https://api.test/m",
                Some("The data source could not be reached: down")
            )
        );
        let ok = Source::failed(&Problem::not_found("x"));
        let (value, _) = optional_part("map", Ok((vec![1_u8], ok)));
        assert_eq!(value, [1]);
    }

    #[test]
    fn stable_keys_and_encoding() {
        assert_eq!(key("a"), "af63dc4c8601ec8c");
        assert_eq!(encode("Sinner's Sacrifice"), "Sinner%27s%20Sacrifice");
        assert_eq!(id_list(&[1, 22]), "1,22");
        assert_eq!(
            host("https://api.deadlock-api.com/v1/x"),
            "api.deadlock-api.com"
        );
        assert_eq!(
            upstream_detail(br#"{"status":503,"error":"Retry later."}"#),
            "Retry later."
        );
        assert_eq!(
            upstream_detail(b"Invalid steam id: abc\n"),
            "Invalid steam id: abc"
        );
        // A gateway's HTML page by its title, not its markup; non-ASCII before it is fine.
        assert_eq!(
            upstream_detail(
                "<!DOCTYPE html><html><head><!-- é --><TITLE>deadlock-api.com | 504: Gateway time-out</TITLE>".as_bytes()
            ),
            "deadlock-api.com | 504: Gateway time-out"
        );
        assert_eq!(
            upstream_detail(b"<html><body>oops</body></html>"),
            "an HTML page instead of the API's answer"
        );
    }
}
