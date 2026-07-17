"use strict";

const os = require("os");
const { supportedPlatforms } = require("./package.json");

function getPlatform() {
  const arch = { x64: "x86_64", arm64: "aarch64" }[os.arch()];
  const platform = {
    Linux: "unknown-linux-gnu",
    Darwin: "apple-darwin",
    Windows_NT: "pc-windows-msvc"
  }[os.type()];
  const key = arch && platform ? `${arch}-${platform}` : "unsupported";
  if (!supportedPlatforms[key]) {
    throw new Error(`Unsupported platform: ${os.type()} ${os.arch()}`);
  }
  return supportedPlatforms[key];
}

module.exports = { getPlatform };
