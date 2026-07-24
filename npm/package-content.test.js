#!/usr/bin/env node
"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const { supportedPlatforms, bin, files } = require("./package.json");

assert.strictEqual(bin.drag, "run.js");
assert.strictEqual(bin["drag-companion"], "run-companion.js");
assert(files.includes("install.js"));
assert(files.includes("platform.js"));
assert(files.includes("run.js"));
assert(files.includes("run-companion.js"));

for (const [target, platform] of Object.entries(supportedPlatforms)) {
  assert(platform.artifact.startsWith(`drag-${target}`), `${target} artifact should match target`);
  assert(platform.binary, `${target} missing drag binary`);
  assert(platform.companionBinary, `${target} missing companion binary`);
  if (target.includes("windows")) {
    assert.strictEqual(platform.binary, "drag.exe");
    assert.strictEqual(platform.companionBinary, "drag-companion.exe");
  } else {
    assert.strictEqual(platform.binary, "drag");
    assert.strictEqual(platform.companionBinary, "drag-companion");
  }
}

for (const workflow of ["release.yml", "homebrew.yml"]) {
  const text = fs.readFileSync(path.join(__dirname, "..", ".github", "workflows", workflow), "utf8");
  assert(text.includes("drag-companion"), `${workflow} must package/install drag-companion`);
}
