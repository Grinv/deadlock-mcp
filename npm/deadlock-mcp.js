#!/usr/bin/env node
// @ts-check
// Runs the deadlock-mcp binary from the @grinv/deadlock-mcp-<platform> package npm installed next
// to this one (an optional dependency it picks by `os` and `cpu`), passing stdio through.
// Plain JavaScript: Node won't strip TypeScript types from files under node_modules.

import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

// Windows on Arm runs the x64 build through its x64 emulation.
const platform =
  process.platform === "win32" ? "win32-x64" : `${process.platform}-${process.arch}`;
const exe = process.platform === "win32" ? "deadlock-mcp.exe" : "deadlock-mcp";

let bin;
try {
  bin = fileURLToPath(import.meta.resolve(`@grinv/deadlock-mcp-${platform}/bin/${exe}`));
} catch {
  console.error(
    `deadlock-mcp: no build for ${process.platform} ${process.arch}. Builds exist for Linux and ` +
      "macOS (x64, arm64) and Windows; if yours is one, reinstall without --omit=optional.",
  );
  process.exit(1);
}

const child = spawn(bin, process.argv.slice(2), { stdio: "inherit" });
for (const signal of /** @type {const} */ (["SIGINT", "SIGTERM"])) {
  process.on(signal, () => child.kill(signal));
}
child.on("error", (error) => {
  console.error(`deadlock-mcp: ${error.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 1);
  }
});
