import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const read = (file) => fs.readFileSync(path.join(root, file), "utf8");
const packageJson = JSON.parse(read("package.json"));
const packageLock = JSON.parse(read("package-lock.json"));
const tauriConfig = JSON.parse(read("src-tauri/tauri.conf.json"));
const expected = String(packageJson.version || "").trim();

if (!/^\d+\.\d+\.\d+$/.test(expected)) {
  throw new Error(`package.json contains an invalid release version: ${expected}`);
}

const cargoTomlVersion = read("src-tauri/Cargo.toml").match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const cargoLockVersion = read("src-tauri/Cargo.lock").match(/\[\[package\]\]\s+name\s*=\s*"ai8888-switch"\s+version\s*=\s*"([^"]+)"/m)?.[1];
const rustVersion = read("src-tauri/src/lib.rs").match(/const CURRENT_APP_VERSION: &str = "v([^"]+)";/)?.[1];
const renderer = read("src/main.tsx");
const readme = read("README.md");
const releaseWorkflow = read(".github/workflows/build-desktop.yml");
const values = {
  "package-lock root": packageLock.version,
  "package-lock workspace": packageLock.packages?.[""]?.version,
  "Cargo.toml": cargoTomlVersion,
  "Cargo.lock": cargoLockVersion,
  "tauri.conf.json": tauriConfig.version,
  "CURRENT_APP_VERSION": rustVersion,
  "README current version": readme.match(/^Current version: v(\d+\.\d+\.\d+)$/m)?.[1],
};

const mismatches = Object.entries(values).filter(([, value]) => value !== expected);
const rendererVersions = [...renderer.matchAll(/v(\d+\.\d+\.\d+) Copyright AI8888\.SHOP 2026/g)].map((match) => match[1]);
if (rendererVersions.length === 0 || rendererVersions.some((version) => version !== expected)) {
  mismatches.push(["renderer footer", rendererVersions.length > 0 ? rendererVersions.join(",") : "missing"]);
}
for (const runner of ["windows-latest", "windows-11-arm", "ubuntu-22.04", "ubuntu-24.04-arm", "macos-latest"]) {
  if (!releaseWorkflow.includes(`platform: ${runner}`)) {
    mismatches.push([`release runner ${runner}`, "missing"]);
  }
}
if (mismatches.length > 0) {
  throw new Error(`release version mismatch; expected ${expected}: ${mismatches.map(([name, value]) => `${name}=${value}`).join(", ")}`);
}

if (process.env.GITHUB_REF_TYPE === "tag") {
  const expectedTag = `v${expected}`;
  if (process.env.GITHUB_REF_NAME !== expectedTag) {
    throw new Error(`release tag ${process.env.GITHUB_REF_NAME} does not match source version ${expectedTag}`);
  }
}

console.log(`Release version verified: v${expected}`);
