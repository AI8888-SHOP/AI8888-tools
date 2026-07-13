import { rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

const output = path.join(tmpdir(), "ai8888-renderer-test");
const tsc = path.resolve("node_modules", "typescript", "lib", "tsc.js");
rmSync(output, { recursive: true, force: true });

try {
  const compile = spawnSync(process.execPath, [
    tsc,
    "--target", "ES2020",
    "--module", "CommonJS",
    "--moduleResolution", "Node",
    "--skipLibCheck",
    "--strict",
    "--outDir", output,
    "src/subscription.ts",
    "tests/subscription.test.ts",
  ], { cwd: process.cwd(), stdio: "inherit" });
  if (compile.status !== 0) process.exit(compile.status ?? 1);

  const testFile = path.join(output, "tests", "subscription.test.js");
  const test = spawnSync(process.execPath, [testFile], { cwd: process.cwd(), stdio: "inherit" });
  if (test.status !== 0) process.exit(test.status ?? 1);
} finally {
  rmSync(output, { recursive: true, force: true });
}
