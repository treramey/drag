#!/usr/bin/env node
"use strict";

const crypto = require("crypto");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");
const { Readable } = require("stream");
const { pipeline } = require("stream/promises");
const { getPlatform } = require("./platform");

const installDir = path.join(__dirname, "bin");

function run(command, args) {
  const result = spawnSync(command, args, { stdio: "pipe" });
  if (result.error || result.status !== 0) {
    throw new Error(`Could not extract Drag: ${result.error?.message || result.stderr.toString()}`);
  }
}

async function download(url, destination) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok || !response.body) {
    throw new Error(`Download failed (${response.status}): ${url}`);
  }
  await pipeline(Readable.fromWeb(response.body), fs.createWriteStream(destination));
}

async function install() {
  const { version } = require("./package.json");
  const platform = getPlatform();
  const base = `https://github.com/treramey/drag/releases/download/v${version}/${platform.artifact}`;
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "drag-"));
  const archive = path.join(temp, platform.artifact);
  try {
    await Promise.all([download(base, archive), download(`${base}.sha256`, `${archive}.sha256`)]);
    const expected = fs.readFileSync(`${archive}.sha256`, "utf8").trim().split(/\s+/)[0].toLowerCase();
    const actual = crypto.createHash("sha256").update(fs.readFileSync(archive)).digest("hex");
    if (actual !== expected) throw new Error("SHA256 checksum mismatch");

    fs.rmSync(installDir, { recursive: true, force: true });
    fs.mkdirSync(installDir, { recursive: true });
    if (archive.endsWith(".zip")) {
      const a = archive.replaceAll("'", "''");
      const d = installDir.replaceAll("'", "''");
      run("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", `Expand-Archive -LiteralPath '${a}' -DestinationPath '${d}' -Force`]);
    } else {
      run("tar", ["-xzf", archive, "-C", installDir, "--strip-components=1"]);
    }

    const binaries = [platform.binary, platform.companionBinary];
    for (const name of binaries) {
      let binary = path.join(installDir, name);
      if (!fs.existsSync(binary)) {
      const directory = fs.readdirSync(installDir, { withFileTypes: true }).find((entry) => entry.isDirectory());
        if (directory && fs.existsSync(path.join(installDir, directory.name, name))) {
          fs.renameSync(path.join(installDir, directory.name, name), binary);
        }
      }
      if (!fs.existsSync(binary)) throw new Error(`Released archive did not contain ${name}`);
      if (process.platform !== "win32") fs.chmodSync(binary, 0o755);
    }
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
}

install().catch((error) => {
  console.error(`Failed to install Drag: ${String(error.message).replace(/[\r\n]+/g, " ")}`);
  process.exit(1);
});
