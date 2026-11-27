# Privacy policy

`deadlock-mcp` runs as a process on your own computer and talks to your MCP client over stdio. It
has no server of its own, no accounts, no telemetry and no analytics, and it sends nothing to its
maintainer.

## What it sends, and where

Each tool call makes HTTPS GET requests that carry only what that call needs, with the user agent
`deadlock-mcp/<version>`:

- Deadlock API (`api.deadlock-api.com`): account ids, match ids and hero ids, a Steam name when
  you search for a player, and filters such as a rank range or a period. The Deadlock API is a
  community project; its privacy policy is at <https://deadlock-api.com/data-privacy>.
- Deadlock Wiki (`deadlock.wiki`): search text and page titles.

The arguments come from the AI model you use, which fills them in from your conversation. Like any
web request, these requests show the services your IP address.

## What it keeps

Answers are cached on your disk so repeated questions don't hit the network:
`$XDG_CACHE_HOME/deadlock-mcp` (`~/.cache/deadlock-mcp`), or `%LOCALAPPDATA%\deadlock-mcp` on
Windows. The cache holds the services' answers, which include public Steam names and profile
links of the players you looked up. Nothing leaves your computer from there. Entries unused for 30
days are deleted when the server starts; delete the folder to clear it all.

Errors such as a failed cache write go to standard error, which your MCP client may keep in its own
log.

## Changes

Changes to this policy are listed in [CHANGELOG.md](CHANGELOG.md).
