# Contract sources

`cargo xtask contracts` turns these files into `src/contracts/{api,valve}.rs`. Both the
inputs and the generated code are committed; the server build needs neither generator nor network.

| File | Origin | Pinned at |
|---|---|---|
| `openapi.json` | <https://api.deadlock-api.com/openapi.json> (OpenAPI 3.1) | fetched 2026-11-26 |
| `protos/` | `crates/valveprotos/protos` of [deadlock-api/deadlock-api](https://github.com/deadlock-api/deadlock-api) | commit `ce14545498608cd30c3a12e6c41eea22f3882cd9` |
| `protos/LICENSE` | `crates/valveprotos/license-protos.txt` of the same commit | |

The protos come from Valve's game files via [SteamDatabase/Protobufs](https://github.com/SteamDatabase/Protobufs);
deadlock-api's `fetch-protos` strips options protox can't read and adds a `syntax` line. They are
the processed copy because the API serializes match metadata from exactly these messages.

## Updating

```sh
curl -s https://api.deadlock-api.com/openapi.json | python3 -m json.tool --indent 1 > contracts/openapi.json
# protos: copy crates/valveprotos/protos from a deadlock-api checkout, note its commit above
cargo xtask contracts
git diff src/contracts   # review what changed upstream, then fix the compile errors it causes
```
