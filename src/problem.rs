//! Errors as RFC 9457 Problem Details, from the HTTP layer up to the MCP boundary, where a problem
//! becomes a tool result with `isError: true`: its title and detail as text for the model, the
//! object itself as `structuredContent` (error results are exempt from `outputSchema`).

use serde::Serialize;
use serde_json::{Map, Value};

/// Problem types; each `type` URI points at its section in `docs/errors.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The arguments can't be used as given.
    InvalidArgument,
    /// The requested thing doesn't exist (yet).
    NotFound,
    /// The data source asked us to slow down.
    RateLimited,
    /// The data source answered with an error.
    Upstream,
    /// The data source couldn't be reached.
    Unreachable,
    /// The data source answered with something we couldn't read.
    InvalidResponse,
}

impl Kind {
    const fn slug(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid-argument",
            Self::NotFound => "not-found",
            Self::RateLimited => "rate-limited",
            Self::Upstream => "upstream-error",
            Self::Unreachable => "unreachable",
            Self::InvalidResponse => "invalid-response",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::InvalidArgument => "Invalid argument",
            Self::NotFound => "Not found",
            Self::RateLimited => "Rate limited",
            Self::Upstream => "The data source returned an error",
            Self::Unreachable => "The data source could not be reached",
            Self::InvalidResponse => "The data source sent an unreadable answer",
        }
    }
}

const TYPE_BASE: &str = "https://github.com/Grinv/deadlock-mcp/blob/main/docs/errors.md#";

#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    #[serde(rename = "type")]
    pub type_uri: String,
    pub title: &'static str,
    /// The HTTP status of the data source's answer, when there was one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub detail: String,
    /// The request that failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(flatten)]
    pub extensions: Map<String, Value>,
    #[serde(skip)]
    pub kind: Kind,
}

impl Problem {
    pub fn new(kind: Kind, detail: impl Into<String>) -> Self {
        Self {
            type_uri: format!("{TYPE_BASE}{}", kind.slug()),
            title: kind.title(),
            status: None,
            detail: detail.into(),
            instance: None,
            extensions: Map::new(),
            kind,
        }
    }

    pub fn invalid_argument(detail: impl Into<String>) -> Self {
        Self::new(Kind::InvalidArgument, detail)
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(Kind::NotFound, detail)
    }

    #[must_use]
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    #[must_use]
    pub fn with_instance(mut self, url: impl Into<String>) -> Self {
        self.instance = Some(url.into());
        self
    }

    #[must_use]
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.extensions.insert(key.to_owned(), value.into());
        self
    }

    /// The text the model reads.
    pub fn message(&self) -> String {
        format!("{}: {}", self.title, self.detail)
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for Problem {}

pub type Result<T> = std::result::Result<T, Problem>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_rfc_9457() {
        let p = Problem::new(Kind::RateLimited, "try again in 30 s")
            .with_status(429)
            .with_instance("https://api.deadlock-api.com/v1/x")
            .with("retry_after_s", 30);
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({
                "type": "https://github.com/Grinv/deadlock-mcp/blob/main/docs/errors.md#rate-limited",
                "title": "Rate limited",
                "status": 429,
                "detail": "try again in 30 s",
                "instance": "https://api.deadlock-api.com/v1/x",
                "retry_after_s": 30
            })
        );
    }
}
