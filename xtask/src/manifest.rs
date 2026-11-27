//! Distribution metadata, derived instead of hand-written: package facts come from `Cargo.toml`
//! (via `cargo metadata`), tools from the server itself over stdio.
//!
//! - `cargo xtask manifest` writes `manifest.json`, the MCP Bundle manifest
//!   (<https://github.com/modelcontextprotocol/mcpb/blob/main/MANIFEST.md>). It is committed; CI fails
//!   when it is stale.
//! - `cargo xtask server-json <sha256>` writes `<target dir>/server.json` for the MCP Registry
//!   (<https://github.com/modelcontextprotocol/registry>). Its `.mcpb` package needs the archive's
//!   SHA-256, which exists only once a release is built, so the release writes it.
//! - `cargo xtask npm <dir>` writes the npm packages: `deadlock-mcp`, whose launcher runs the binary
//!   of the `@grinv/deadlock-mcp-<platform>` package npm installs for the machine. The release puts
//!   each binary into its package's `bin/` and publishes them.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::Result;

const PACKAGE: &str = "deadlock-mcp";
/// MCPB manifest version: 0.4 adds only the `uv` server type; 0.3 is the CLI's default and what
/// clients have supported longest.
const MANIFEST_VERSION: &str = "0.3";
/// The bundle keeps the binaries here; MCPB clients run them from the unpacked bundle. This one is
/// macOS's universal binary; Linux's are `-x86_64` and `-aarch64` next to it, Windows' `.exe`.
const ENTRY_POINT: &str = "server/deadlock-mcp";
/// macOS: Claude Desktop unpacks bundles with mode 0600, so it can't exec a binary in one
/// (modelcontextprotocol/mcpb#294). It can exec `/bin/sh`, which restores the bit and hands over.
const DARWIN_LAUNCH: &str = r#"chmod u+x "$0" && exec "$0""#;
/// Linux: the same, and MCPB has no CPU field (modelcontextprotocol/mcpb#10), so `uname -m`
/// picks the binary.
const LINUX_LAUNCH: &str = r#"b="$0-$(uname -m)" && chmod u+x "$b" && exec "$b""#;
const REGISTRY_SCHEMA: &str =
    "https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json";
/// The registry's limit on `description`.
const MAX_DESCRIPTION: usize = 100;
/// npm scope of the per-platform binary packages.
const NPM_SCOPE: &str = "@grinv";
/// The npm packages with a binary: name suffix, `os`, `cpu`, and the binary's file name. The
/// Windows one also installs on Arm, which runs it through its x64 emulation.
const NPM_PLATFORMS: &[(&str, &str, &[&str], &str)] = &[
    ("linux-x64", "linux", &["x64"], "deadlock-mcp"),
    ("linux-arm64", "linux", &["arm64"], "deadlock-mcp"),
    ("darwin-x64", "darwin", &["x64"], "deadlock-mcp"),
    ("darwin-arm64", "darwin", &["arm64"], "deadlock-mcp"),
    ("win32-x64", "win32", &["x64", "arm64"], "deadlock-mcp.exe"),
];

struct Facts {
    package: Value,
    server: Value,
    tools: Vec<Value>,
}

impl Facts {
    fn load(root: &Path) -> Result<Self> {
        let package = package(root)?;
        let description = package["description"].as_str().unwrap_or_default();
        if description.chars().count() > MAX_DESCRIPTION {
            return Err(format!(
                "Cargo.toml's description has {} chars; the MCP Registry allows {MAX_DESCRIPTION}",
                description.chars().count()
            )
            .into());
        }
        let (server, tools) = surface(root)?;
        Ok(Self {
            package,
            server,
            tools,
        })
    }

    fn repository(&self) -> &str {
        self.package["repository"].as_str().unwrap_or_default()
    }
}

/// `https://github.com/Owner/Repo` -> `io.github.Owner/Repo`, the namespace the registry verifies
/// through GitHub.
fn registry_name(repository: &str) -> Result<String> {
    repository
        .strip_prefix("https://github.com/")
        .map(|path| format!("io.github.{path}"))
        .ok_or_else(|| "the registry name needs a GitHub repository URL".into())
}

pub fn manifest(root: &Path) -> Result<()> {
    let facts = Facts::load(root)?;
    let repository = facts.repository();
    let author = facts.package["authors"][0]
        .as_str()
        .ok_or("Cargo.toml needs `authors`")?;
    let manifest = json!({
        "manifest_version": MANIFEST_VERSION,
        "name": facts.server["name"],
        "display_name": facts.server["title"],
        "version": facts.server["version"],
        "description": facts.package["description"],
        // `authors` entries may carry an email in angle brackets.
        "author": { "name": author.split(" <").next().unwrap_or(author) },
        "repository": { "type": "git", "url": repository },
        "homepage": repository,
        "documentation": format!("{repository}#readme"),
        "support": format!("{repository}/issues"),
        "license": facts.package["license"],
        "keywords": facts.package["keywords"],
        "server": {
            "type": "binary",
            "entry_point": ENTRY_POINT,
            "mcp_config": {
                "command": format!("${{__dirname}}/{ENTRY_POINT}"),
                "args": [],
                "env": {},
                "platform_overrides": {
                    "darwin": {
                        "command": "/bin/sh",
                        "args": ["-c", DARWIN_LAUNCH, format!("${{__dirname}}/{ENTRY_POINT}")]
                    },
                    "linux": {
                        "command": "/bin/sh",
                        "args": ["-c", LINUX_LAUNCH, format!("${{__dirname}}/{ENTRY_POINT}")]
                    },
                    "win32": { "command": format!("${{__dirname}}/{ENTRY_POINT}.exe") }
                }
            }
        },
        "tools_generated": false,
        "tools": facts.tools,
        // MCPB's static prompts can't hold review_match's optional `match_id`, so clients ask
        // the server for its prompts.
        "prompts_generated": true,
        "compatibility": { "platforms": ["darwin", "win32", "linux"] },
        "privacy_policies": [
            format!("{repository}/blob/main/PRIVACY.md"),
            "https://deadlock-api.com/data-privacy"
        ]
    });
    write(&root.join("manifest.json"), &manifest)
}

pub fn server_json(root: &Path, sha256: &str) -> Result<()> {
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("usage: cargo xtask server-json <sha256 of the .mcpb archive>".into());
    }
    let facts = Facts::load(root)?;
    let repository = facts.repository();
    let name = registry_name(repository)?;
    let version = &facts.server["version"];
    let server = json!({
        "$schema": REGISTRY_SCHEMA,
        "name": name,
        "title": facts.server["title"],
        "description": facts.package["description"],
        "version": version,
        "websiteUrl": format!("{repository}#readme"),
        "repository": { "url": repository, "source": "github" },
        "packages": [{
            "registryType": "npm",
            "identifier": PACKAGE,
            "version": version,
            "runtimeHint": "npx",
            "transport": { "type": "stdio" }
        }, {
            "registryType": "mcpb",
            "identifier": format!(
                "{repository}/releases/download/v{}/{PACKAGE}.mcpb",
                version.as_str().unwrap_or_default()
            ),
            "version": version,
            "fileSha256": sha256.to_ascii_lowercase(),
            "transport": { "type": "stdio" }
        }]
    });
    write(&target_dir(root).join("server.json"), &server)
}

/// Writes the npm packages into `out`: `deadlock-mcp`, with the launcher, and one
/// `@grinv/deadlock-mcp-<platform>` per build, each with an empty `bin/` for its binary.
pub fn npm(root: &Path, out: &Path) -> Result<()> {
    let package = package(root)?;
    let version = &package["version"];
    let repository = package["repository"].as_str().unwrap_or_default();
    let common = |name: String, description: String| {
        json!({
            "name": name,
            "version": version,
            "description": description,
            "license": package["license"],
            "repository": { "type": "git", "url": format!("git+{repository}.git") },
            "files": ["bin/"]
        })
    };
    let license = std::fs::read(root.join("LICENSE"))?;
    let mut optional = serde_json::Map::new();
    for (suffix, os, cpu, exe) in NPM_PLATFORMS {
        let name = format!("{NPM_SCOPE}/{PACKAGE}-{suffix}");
        let mut manifest = common(
            name.clone(),
            format!("The {PACKAGE} binary for {suffix} ({exe})."),
        );
        manifest["os"] = json!([os]);
        manifest["cpu"] = json!(cpu);
        // Yarn PnP would otherwise keep the binary inside a zip it can't exec from.
        manifest["preferUnplugged"] = json!(true);
        let dir = out.join(suffix);
        std::fs::create_dir_all(dir.join("bin"))?;
        std::fs::write(dir.join("LICENSE"), &license)?;
        write(&dir.join("package.json"), &manifest)?;
        optional.insert(name, version.clone());
    }
    let mut manifest = common(
        PACKAGE.to_owned(),
        package["description"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    );
    manifest["keywords"] = package["keywords"].clone();
    manifest["homepage"] = json!(format!("{repository}#readme"));
    manifest["bugs"] = json!(format!("{repository}/issues"));
    // The MCP Registry reads this to check that the npm package belongs to the server.
    manifest["mcpName"] = json!(registry_name(repository)?);
    manifest["bin"] = json!({ PACKAGE: format!("bin/{PACKAGE}.js") });
    // `import.meta.resolve` returns synchronously from Node 20.6; 20 is out of support.
    manifest["type"] = json!("module");
    manifest["engines"] = json!({ "node": ">=22" });
    manifest["optionalDependencies"] = Value::Object(optional);
    let dir = out.join(PACKAGE);
    std::fs::create_dir_all(dir.join("bin"))?;
    std::fs::copy(
        root.join("npm").join(format!("{PACKAGE}.js")),
        dir.join("bin").join(format!("{PACKAGE}.js")),
    )?;
    std::fs::copy(root.join("README.md"), dir.join("README.md"))?;
    std::fs::write(dir.join("LICENSE"), &license)?;
    write(&dir.join("package.json"), &manifest)
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), Into::into)
}

fn write(path: &Path, value: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)? + "\n")?;
    println!("wrote {}", path.display());
    Ok(())
}

/// This package's entry in `cargo metadata`.
fn package(root: &Path) -> Result<Value> {
    let out = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"))
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    let meta: Value = serde_json::from_slice(&out.stdout)?;
    meta["packages"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["name"] == PACKAGE))
        .cloned()
        .ok_or_else(|| format!("no package {PACKAGE} in cargo metadata").into())
}

/// The server's `serverInfo` and its tools as `{name, description}`, asked over stdio.
fn surface(root: &Path) -> Result<(Value, Vec<Value>)> {
    let mut child = Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--package", PACKAGE])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let mut next_id = 0;
    // A request waits for its reply; a notification (`id: None`) doesn't get one.
    let mut call = |notify: bool, method: &str, params: Value| -> Result<Value> {
        if notify {
            writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "method": method }))?;
            return Ok(Value::Null);
        }
        next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": next_id, "method": method, "params": params });
        writeln!(stdin, "{msg}")?;
        loop {
            let reply: Value = serde_json::from_str(&lines.next().ok_or("server closed")??)?;
            if reply["id"] == next_id {
                return reply
                    .get("result")
                    .cloned()
                    .ok_or_else(|| format!("{method}: {reply}").into());
            }
        }
    };
    let init = call(
        false,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "xtask", "version": "0" }
        }),
    )?;
    call(true, "notifications/initialized", Value::Null)?;
    let tools = call(false, "tools/list", json!({}))?["tools"]
        .as_array()
        .ok_or("tools/list has no tools")?
        .iter()
        .map(|t| json!({ "name": t["name"], "description": t["description"] }))
        .collect();
    child.kill()?;
    child.wait()?;
    Ok((init["serverInfo"].clone(), tools))
}
