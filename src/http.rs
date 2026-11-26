//! HTTP GET with an on-disk cache.
//!
//! Each entry keeps the body and its `ETag`. A stale entry is revalidated with `If-None-Match`
//! (a 304 costs no body) and, when the request fails, served as is and marked `stale`. Errors are
//! never cached. A 429/503 with a short `Retry-After` and a network error or 5xx are retried once.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::problem::{Kind, Problem};

const USER_AGENT: &str = concat!("deadlock-mcp/", env!("CARGO_PKG_VERSION"));
/// The 6.5 MB item catalog is the largest response.
const MAX_BODY: u64 = 64 << 20;
const TIMEOUT: Duration = Duration::from_secs(30);
/// Longer waits are left to the caller, with the time in the error.
const MAX_RETRY_WAIT_S: u64 = 10;

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
    base.join("deadlock-mcp")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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

/// The deadlock-api answers errors as `{"status": .., "error": ".."}` JSON or as plain text.
fn upstream_detail(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
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

fn attempt(url: &str, etag: Option<&str>) -> Attempt {
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
        Err(e) => {
            let problem =
                Problem::new(Kind::Unreachable, format!("{}: {e}", host(url))).with_instance(url);
            return Attempt::Failed {
                problem,
                retry_in: Some(Duration::from_secs(1)),
            };
        }
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
        .take(MAX_BODY)
        .read_to_end(&mut body)
    {
        let problem =
            Problem::new(Kind::Unreachable, format!("{}: {e}", host(url))).with_instance(url);
        return Attempt::Failed {
            problem,
            retry_in: Some(Duration::from_secs(1)),
        };
    }
    if (200..300).contains(&status) {
        return Attempt::Body {
            etag: new_etag,
            body,
        };
    }
    let detail = upstream_detail(&body);
    let problem = |kind| {
        Problem::new(kind, detail.clone())
            .with_status(status)
            .with_instance(url)
    };
    match status {
        429 | 503 if retry_after.is_some() || status == 429 => {
            let wait = retry_after.map_or_else(|| "later".to_owned(), |s| format!("in {s} s"));
            let mut p = Problem::new(
                Kind::RateLimited,
                format!("{} asked to slow down; try again {wait}", host(url)),
            )
            .with_status(status)
            .with_instance(url);
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
            problem: problem(Kind::NotFound),
            retry_in: None,
        },
        500..=599 => Attempt::Failed {
            problem: problem(Kind::Upstream),
            retry_in: Some(Duration::from_secs(1)),
        },
        _ => Attempt::Failed {
            problem: problem(Kind::Upstream),
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
    let cached = std::fs::read(&meta_path)
        .ok()
        .and_then(|m| serde_json::from_slice::<Meta>(&m).ok())
        .filter(|m| m.url == url)
        .and_then(|m| std::fs::read(&body_path).ok().map(|b| (m, b)));
    let source = |fetched_at, from_cache, stale| Source {
        url: url.to_owned(),
        fetched_at,
        from_cache,
        stale,
    };

    if let Some((meta, body)) = &cached {
        let still_fresh = match fresh {
            Fresh::Forever => true,
            Fresh::For(ttl) => now().saturating_sub(meta.fetched_at) < ttl.as_secs(),
        };
        if still_fresh {
            return Ok((body.clone(), source(meta.fetched_at, true, false)));
        }
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
            let fetched_at = now();
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
                let fetched_at = now();
                save(&meta_path, &body_path, &Meta { fetched_at, ..meta }, None);
                Ok((body, source(fetched_at, true, false)))
            }
            None => Err(
                Problem::new(Kind::InvalidResponse, "304 without a cached copy").with_instance(url),
            ),
        },
        Attempt::Failed { problem, .. } if problem.kind == Kind::NotFound => Err(problem),
        Attempt::Failed { problem, .. } => match cached {
            Some((meta, body)) => Ok((body, source(meta.fetched_at, true, true))),
            None => Err(problem),
        },
    }
}

/// A cache write that fails only costs a refetch, so it is not an error for the caller.
fn save(meta_path: &Path, body_path: &Path, meta: &Meta, body: Option<&[u8]>) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir())?;
        if let Some(body) = body {
            std::fs::write(body_path, body)?;
        }
        std::fs::write(meta_path, serde_json::to_vec(meta)?)
    };
    if let Err(e) = write() {
        eprintln!("deadlock-mcp: cache write failed: {e}");
    }
}

pub fn get_json(url: &str, fresh: Fresh) -> crate::problem::Result<(serde_json::Value, Source)> {
    let (body, source) = get(url, fresh)?;
    let value = serde_json::from_slice(&body).map_err(|e| {
        Problem::new(Kind::InvalidResponse, format!("not JSON: {e}")).with_instance(url)
    })?;
    Ok((value, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_keys_and_encoding() {
        assert_eq!(key("a"), "af63dc4c8601ec8c");
        assert_eq!(encode("Sinner's Sacrifice"), "Sinner%27s%20Sacrifice");
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
    }
}
