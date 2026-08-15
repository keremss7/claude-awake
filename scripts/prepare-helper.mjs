#!/usr/bin/env node
/**
 * Builds the privileged helper so the bundler can pick it up.
 *
 * The helper is a second `[[bin]]` in the same crate, which Tauri copies into the
 * bundle next to the app executable — `Contents/MacOS/` on macOS, the install
 * directory on Windows. Nothing else needs to move it.
 *
 * This exists instead of a shell one-liner in package.json for two reasons: the
 * same command has to work in bash, zsh and PowerShell, and the universal macOS
 * build needs both architectures merged by hand (`universal-apple-darwin` is not
 * a real rustc target — Tauri fabricates it with lipo, and only for the app).
 */

import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const targetRoot = join(root, "src-tauri", "target");
const manifest = join(root, "src-tauri", "Cargo.toml");
const exe = process.platform === "win32" ? "claude-awake-helperd.exe" : "claude-awake-helperd";
const profile = process.argv.includes("--debug") ? "debug" : "release";
const universal = process.argv.includes("--universal") && process.platform === "darwin";

const UNIVERSAL_TRIPLE = "universal-apple-darwin";
const SLICES = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

function build(target) {
  const args = ["build", "--manifest-path", manifest, "--bin", "claude-awake-helperd"];
  if (profile === "release") args.push("--release");
  if (target) args.push("--target", target);
  console.log(`> cargo ${args.join(" ")}`);
  execFileSync("cargo", args, { stdio: "inherit" });
  return join(targetRoot, ...(target ? [target] : []), profile, exe);
}

let built;
if (universal) {
  const slices = SLICES.map(build);
  // Tauri looks for every crate binary under the universal target directory, so
  // the merged helper has to land exactly there.
  const outDir = join(targetRoot, UNIVERSAL_TRIPLE, profile);
  mkdirSync(outDir, { recursive: true });
  built = join(outDir, exe);
  console.log(`> lipo -create -output ${built}`);
  execFileSync("lipo", ["-create", "-output", built, ...slices], { stdio: "inherit" });
} else {
  built = build(null);
}

if (!existsSync(built)) {
  console.error(`helper binary not found at ${built}`);
  process.exit(1);
}
if (process.platform !== "win32") chmodSync(built, 0o755);
console.log(`helper ready: ${built}`);
