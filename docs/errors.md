# Errors

A tool that fails returns `isError: true`. Its text is meant for the model; its `structuredContent` is an
[RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) Problem Details object:

```json
{
  "type": "https://github.com/Grinv/deadlock-mcp/blob/main/docs/errors.md#rate-limited",
  "title": "Rate limited",
  "status": 429,
  "detail": "api.deadlock-api.com asked to slow down; try again in 30 s",
  "instance": "https://api.deadlock-api.com/v1/players/12345678/match-history",
  "retry_after_s": 30
}
```

`status` is the HTTP status of the data source's answer, when there was one, and `instance` the request that failed.

## invalid-argument

The arguments can't be used as given, e.g. an account that didn't play the match. `detail` says what to change.

## not-found

The thing asked for doesn't exist, or not yet: the Deadlock API has a match only once one of its players sends it
with deadlock-api-ingest or a bot watches it.

## rate-limited

The data source asked to slow down. A wait of up to 10 s is taken and the request retried once; a longer one comes
back here, with `retry_after_s` when the source gave it.

## upstream-error

The data source answered with an error. A 5xx is retried once after a second.

## unreachable

The data source couldn't be reached (network error or timeout). Retried once after a second.

## invalid-response

The data source answered with something that couldn't be read: not JSON, a shape that doesn't
match its published contract (`detail` names the field path, e.g. `[3].hero: invalid type`), or an
answer over 64 MiB.

## internal-error

A bug in this server, not a problem with the data source. Please report it with the tool call that
caused it.

When a request fails and an older copy is cached, the tool answers with that copy instead and marks it `stale` in
the answer's `sources`. When a part an answer can go without (Steam names, place names, awards) fails with nothing
cached, the answer leaves it out and lists its source with an `error`, so the gap isn't read as "none".
