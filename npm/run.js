#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");
const { getPlatform } = require("./platform");

const platform = getPlatform();
const invoked = process.env.DRAG_NPM_BINARY || path.basename(process.argv[1]).replace(/\.cmd$/i, "");
const selected = invoked === "drag-companion" ? platform.companionBinary : platform.binary;
const binary = path.join(__dirname, "bin", selected);
if (!fs.existsSync(binary)) {
  const install = spawnSync(process.execPath, [path.join(__dirname, "install.js")], { stdio: "inherit" });
  if (install.status !== 0) process.exit(install.status ?? 1);
}
const result = spawnSync(binary, process.argv.slice(2), { cwd: process.cwd(), stdio: "inherit" });
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
