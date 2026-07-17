#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");
const { getPlatform } = require("./platform");

const binary = path.join(__dirname, "bin", getPlatform().binary);
if (!fs.existsSync(binary)) {
  const install = spawnSync(process.execPath, [path.join(__dirname, "install.js")], { stdio: "inherit" });
  if (install.status !== 0) process.exit(install.status ?? 1);
}
const result = spawnSync(binary, process.argv.slice(2), { cwd: process.cwd(), stdio: "inherit" });
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
