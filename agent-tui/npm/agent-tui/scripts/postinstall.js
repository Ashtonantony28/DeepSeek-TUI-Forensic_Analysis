#!/usr/bin/env node
// Downloads the prebuilt agent-tui binary for the current platform from
// GitHub Releases and places it at npm/agent-tui/bin/agent-tui.
// Mirrors the pattern used by deepseek-tui's npm wrapper.

"use strict";

const https = require("https");
const fs = require("fs");
const path = require("path");
const os = require("os");
const { execSync } = require("child_process");
const zlib = require("zlib");

const VERSION = require("../package.json").version;
const REPO = "ashtonantony28/deepseek-tui-enhanced";
const BASE_URL = `https://github.com/${REPO}/releases/download/v${VERSION}`;
const BIN_DIR = path.join(__dirname, "..", "bin");

function platformTarget() {
  const arch = os.arch(); // 'x64' or 'arm64'
  const plat = process.platform; // 'linux', 'darwin', 'win32'

  if (plat === "linux" && arch === "x64")
    return { target: "x86_64-unknown-linux-gnu", ext: "tar.gz", bin: "agent-tui" };
  if (plat === "linux" && arch === "arm64")
    return { target: "aarch64-unknown-linux-gnu", ext: "tar.gz", bin: "agent-tui" };
  if (plat === "darwin" && arch === "x64")
    return { target: "x86_64-apple-darwin", ext: "tar.gz", bin: "agent-tui" };
  if (plat === "darwin" && arch === "arm64")
    return { target: "aarch64-apple-darwin", ext: "tar.gz", bin: "agent-tui" };
  if (plat === "win32" && arch === "x64")
    return { target: "x86_64-pc-windows-msvc", ext: "zip", bin: "agent-tui.exe" };

  return null;
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(dest);
    function get(u) {
      https.get(u, (res) => {
        if (res.statusCode === 301 || res.statusCode === 302) {
          return get(res.headers.location);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode} for ${u}`));
        }
        res.pipe(file);
        file.on("finish", () => file.close(resolve));
      }).on("error", reject);
    }
    get(url);
  });
}

function extractTarGz(archive, targetDir) {
  execSync(`tar -xzf "${archive}" -C "${targetDir}" --strip-components=1`);
}

function extractZip(archive, targetDir) {
  // Node's built-in has no zip support; fall back to system unzip or PowerShell.
  if (process.platform === "win32") {
    execSync(
      `powershell -Command "Expand-Archive -Path '${archive}' -DestinationPath '${targetDir}' -Force"`
    );
    // Move binary out of subdirectory created by Expand-Archive.
    const entries = fs.readdirSync(targetDir);
    if (entries.length === 1 && fs.statSync(path.join(targetDir, entries[0])).isDirectory()) {
      const sub = path.join(targetDir, entries[0]);
      for (const f of fs.readdirSync(sub)) {
        fs.renameSync(path.join(sub, f), path.join(targetDir, f));
      }
      fs.rmdirSync(sub);
    }
  } else {
    execSync(`unzip -o "${archive}" -d "${targetDir}"`);
  }
}

async function main() {
  const info = platformTarget();
  if (!info) {
    console.warn(
      `agent-tui: unsupported platform ${process.platform}/${os.arch()}. ` +
        `Build from source: cargo install agent-tui-cli`
    );
    process.exit(0);
  }

  if (!fs.existsSync(BIN_DIR)) fs.mkdirSync(BIN_DIR, { recursive: true });

  const archiveName = `agent-tui-${info.target}.${info.ext}`;
  const url = `${BASE_URL}/${archiveName}`;
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "agent-tui-install-"));
  const archivePath = path.join(tmpDir, archiveName);
  const destBin = path.join(BIN_DIR, process.platform === "win32" ? "agent-tui.exe" : "agent-tui");

  console.log(`agent-tui: downloading ${url}`);
  try {
    await download(url, archivePath);
  } catch (e) {
    console.error(`agent-tui: download failed: ${e.message}`);
    console.error("Build from source: cargo install agent-tui-cli");
    process.exit(0); // soft failure — don't block npm install
  }

  console.log(`agent-tui: extracting ${archiveName}`);
  if (info.ext === "tar.gz") {
    extractTarGz(archivePath, tmpDir);
  } else {
    extractZip(archivePath, tmpDir);
  }

  const extractedBin = path.join(tmpDir, info.bin);
  if (!fs.existsSync(extractedBin)) {
    console.error(`agent-tui: extracted binary not found at ${extractedBin}`);
    process.exit(0);
  }

  fs.copyFileSync(extractedBin, destBin);
  if (process.platform !== "win32") {
    fs.chmodSync(destBin, 0o755);
  }

  // Clean up temp dir.
  fs.rmSync(tmpDir, { recursive: true, force: true });

  console.log(`agent-tui: installed to ${destBin}`);
}

main().catch((e) => {
  console.error(`agent-tui: postinstall failed: ${e.message}`);
  process.exit(0); // soft failure
});
