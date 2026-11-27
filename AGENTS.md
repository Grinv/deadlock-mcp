# AGENTS.md

Notes for anyone changing this code, people and coding agents alike. What the server does is in
[README.md](README.md).

## Commands

```sh
cargo fmt --check
cargo clippy --workspace --all-targets   # pedantic is on in Cargo.toml; keep it at zero warnings
cargo test --workspace
cargo xtask contracts                    # regenerate src/contracts from contracts/
cargo xtask manifest                     # regenerate manifest.json from Cargo.toml and the server
```

`cargo xtask server-json <sha256>` writes `server.json` for the MCP Registry into the target
directory. It needs the SHA-256 of the release's `.mcpb` archive, so the release runs it.

CI runs the same with `CARGO_BUILD_WARNINGS=deny`, checks that `cargo xtask contracts` and
`cargo xtask manifest` leave no diff, validates `manifest.json` with the mcpb CLI, and runs the
tests on Linux, Windows and macOS (`.github/workflows/ci.yml`).

The resolver skips dependency versions published less than 7 days ago (`.cargo/config.toml`). If a
requirement in `Cargo.toml` names such a version, `cargo update` fails; lower the requirement and
the locked version stays.

## Releasing

1. Set the new `version` in `Cargo.toml`, run `cargo xtask manifest`, and rename `## [Unreleased]`
   in `CHANGELOG.md` to `## [x.y.z] - YYYY-MM-DD` above a fresh, empty `[Unreleased]`.
2. Commit, push, and once CI is green, tag `vx.y.z` and push the tag.

`.github/workflows/release.yml` then runs the whole CI on the tagged commit and builds the binaries:
Linux x86_64 and aarch64 against musl in a pinned Rust Alpine image (static, so they run on any
glibc), macOS arm64 and x86_64 joined into one universal binary, Windows x64. It checks the tag
against `Cargo.toml`, takes the release notes from the version's `CHANGELOG.md` section, packs
`deadlock-mcp.mcpb`, creates the GitHub Release, and publishes `server.json` with the bundle's
SHA-256 to the MCP Registry as `io.github.Grinv/deadlock-mcp`. It also publishes the npm packages
that `cargo xtask npm` lays out: `deadlock-mcp`, whose launcher (`npm/deadlock-mcp.js`) runs the
binary of the `@grinv/deadlock-mcp-<platform>` package npm installed for the machine by `os` and
`cpu`. Running the workflow by hand (`workflow_dispatch`) builds and packs everything without
publishing, to try a change first.

npm Trusted Publishing (OIDC, no token) can be set up only for a package that exists, so the six
packages are first published by hand from the `release` artifact of a dry run
(`npm publish <tarball> --access public`, platform packages first); then each one gets
`Grinv/deadlock-mcp` and `release.yml` as its trusted publisher on npmjs.com. The workflow skips a
version that is already on npm.

In the bundle, `server/deadlock-mcp` is the macOS binary, `server/deadlock-mcp-x86_64` and
`-aarch64` the Linux ones, `server/deadlock-mcp.exe` the Windows one. On macOS and Linux the
manifest starts them through `/bin/sh`: Claude Desktop unpacks bundles without the execute bit
(modelcontextprotocol/mcpb#294), and MCPB can't pick a binary by CPU (mcpb#10). The commands are in
`xtask/src/manifest.rs`.

## Layout

Data flows down this list: a module uses only the ones above it.

| Path | What it holds |
|---|---|
| `contracts/` | Pinned upstream contracts: the Deadlock API's `openapi.json`, Valve's protos, recorded answers in `fixtures/`, their origin in `SOURCES.md` |
| `src/contracts/` | Wire types generated from `contracts/` by `cargo xtask contracts`; committed, never edited by hand |
| `src/problem.rs` | RFC 9457 problems; every kind has a section in `docs/errors.md` |
| `src/http.rs` | GET with the disk cache, `ETag` revalidation, stale fallback, per-host pacing, one retry |
| `src/sources/` | One typed function per upstream endpoint, with its cache lifetime |
| `src/domain/` | Game knowledge: the catalog (heroes, items, ranks, lanes, named places on the map) for the newest game version, names for Valve's enums, a player's own result, text helpers |
| `src/tools/` | What each tool does, and its answer types |
| `src/server/` | The MCP surface: one tool router per area (player, matches, meta, game), resources, completion, the instructions and the prompt |
| `xtask/` | The generators behind `cargo xtask contracts`, `manifest` and `server-json` |

## Rules

- Each tool answers one question. Don't merge several endpoints into one answer; describe how the
  model should combine tools in `INSTRUCTIONS` and `REVIEW_STEPS` (`src/server/mod.rs`).
- Every fact has one source. Wire types come from the contracts, names from the game data (lanes
  from `generic-data`, since Valve's lane enum still names the four-lane map), and a tool's schema
  from its argument type. Generate or read a list that exists upstream instead of copying it.
- Matches over Valve's enums are exhaustive, without `_`, so a new variant breaks the build.
- Tool argument types use `#[serde(deny_unknown_fields)]`, a `garde` rule on every field, and a doc
  comment that becomes the schema description. Serde can't combine `#[serde(flatten)]` with
  `deny_unknown_fields`, so repeat shared fields instead. Prompt arguments (`ReviewMatchArgs`) get
  only the doc comment: rmcp parses them with plain serde, never runs `garde`, and lists each
  argument with just its name, description and whether it is required, so a limit would be enforced
  without being shown.
- Every answer has `sources`, plus the catalog's degraded ones (`tools::answer_sources`) when it
  used the catalog. Text written by people (Steam names, the wiki) goes through
  `domain::format::plain`. Every win rate carries its match count.
- Errors are `Problem`s of an existing kind. A new kind needs a section in `docs/errors.md`, and a
  test checks that.
- The server reads public data over the network only, never local game or Steam files. Keep real
  players' account ids out of docs and examples; the recorded fixtures are the exception.

## Adding a tool

1. Add the response schema name to `API_SCHEMAS` in `xtask/src/main.rs` (or an inline parameter
   enum to `API_PARAMS`), run `cargo xtask contracts`, and record a trimmed real answer in
   `contracts/fixtures/` for the test in `src/contracts/mod.rs` to parse.
2. Add a fetch function with a cache lifetime to `src/sources/api.rs`.
3. Write the logic and the answer type in `src/tools/<area>.rs`.
4. Register the tool in `src/server/<area>.rs` with a `title`, `input_schema::<Args>()` and the
   `read_only_hint` and `open_world_hint` annotations. Mention it in `INSTRUCTIONS` and in the
   README's tool table (tests fail when either is missing), run `cargo xtask manifest`, and add a
   line under `[Unreleased]` in `CHANGELOG.md`.
5. Call it over stdio against the live API, with good and bad arguments.

## Updating the contracts

Follow [contracts/SOURCES.md](contracts/SOURCES.md), then read `git diff src/contracts`. A field
that was renamed or removed upstream shows up there and nowhere else.
