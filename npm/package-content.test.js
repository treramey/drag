#!/usr/bin/env node
"use strict";

const assert = require("assert");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");
const { supportedPlatforms, bin, files } = require("./package.json");

assert.strictEqual(bin.drag, "run.js");
assert.strictEqual(bin["drag-tracking"], "run-tracking.js");
assert.strictEqual(bin["drag-companion"], "run-companion.js");
assert(files.includes("install.js"));
assert(files.includes("platform.js"));
assert(files.includes("run.js"));
assert(files.includes("run-tracking.js"));
assert(files.includes("run-companion.js"));

for (const [target, platform] of Object.entries(supportedPlatforms)) {
  assert(platform.artifact.startsWith(`drag-${target}`), `${target} artifact should match target`);
  assert(platform.binary, `${target} missing drag binary`);
  assert(platform.trackingBinary, `${target} missing tracking binary`);
  assert(platform.companionBinary, `${target} missing companion binary`);
  if (target.includes("windows")) {
    assert.strictEqual(platform.binary, "drag.exe");
    assert.strictEqual(platform.trackingBinary, "drag-tracking.exe");
    assert.strictEqual(platform.companionBinary, "drag-companion.exe");
  } else {
    assert.strictEqual(platform.binary, "drag");
    assert.strictEqual(platform.trackingBinary, "drag-tracking");
    assert.strictEqual(platform.companionBinary, "drag-companion");
  }
}

for (const workflow of ["release.yml", "homebrew.yml"]) {
  const text = fs.readFileSync(path.join(__dirname, "..", ".github", "workflows", workflow), "utf8");
  assert(text.includes("drag-companion"), `${workflow} must package/install drag-companion`);
  assert(text.includes("drag-tracking"), `${workflow} must package/install drag-tracking`);
}

if (process.platform !== "win32") {
  const sandbox = fs.mkdtempSync(path.join(os.tmpdir(), "drag-npm-routing-"));
  try {
    for (const file of ["package.json", "platform.js", "run.js", "run-tracking.js"]) {
      fs.copyFileSync(path.join(__dirname, file), path.join(sandbox, file));
    }
    const sandboxBin = path.join(sandbox, "bin");
    fs.mkdirSync(sandboxBin);
    const platform = require(path.join(sandbox, "platform.js")).getPlatform();
    const resultFile = path.join(sandbox, "result.txt");
    fs.writeFileSync(
      path.join(sandboxBin, platform.trackingBinary),
      "#!/bin/sh\nprintf 'tracking:%s\\n' \"${DRAG_NPM_BINARY-unset}\" >> \"$RESULT_FILE\"\nexec \"$NODE\" \"$SANDBOX/run.js\" nested\n",
      { mode: 0o700 }
    );
    fs.writeFileSync(
      path.join(sandboxBin, platform.binary),
      "#!/bin/sh\nprintf 'drag:%s\\n' \"${DRAG_NPM_BINARY-unset}\" >> \"$RESULT_FILE\"\n",
      { mode: 0o700 }
    );
    const routed = spawnSync(process.execPath, [path.join(sandbox, "run-tracking.js")], {
      env: {
        ...process.env,
        NODE: process.execPath,
        RESULT_FILE: resultFile,
        SANDBOX: sandbox
      }
    });
    assert.strictEqual(routed.status, 0, routed.stderr?.toString());
    assert.strictEqual(fs.readFileSync(resultFile, "utf8"), "tracking:unset\ndrag:unset\n");
  } finally {
    fs.rmSync(sandbox, { recursive: true, force: true });
  }
}
