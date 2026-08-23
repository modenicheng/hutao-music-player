import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
export const appRoot = path.resolve(scriptDir, "..");
export const repoRoot = path.resolve(appRoot, "..", "..");

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? repoRoot,
    env: options.env ?? process.env,
    encoding: options.capture ? "utf8" : undefined,
    stdio: options.capture ? ["ignore", "pipe", "inherit"] : "inherit",
  });

  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} exited with code ${result.status}`);
  }
  return result.stdout ?? "";
}

export function prepareSidecar(profile = "debug", baseEnv = process.env) {
  if (!new Set(["debug", "release"]).has(profile)) {
    throw new Error(`Unsupported sidecar profile: ${profile}`);
  }
  const env = { ...baseEnv };

  const cargoArgs = [
    "build",
    "--manifest-path",
    path.join(repoRoot, "Cargo.toml"),
    "-p",
    "hmp-daemon",
    "--bin",
    "hmpd",
    "--no-default-features",
  ];
  if (profile === "release") cargoArgs.push("--release");
  run("cargo", cargoArgs, { env });

  const hostLine = run("rustc", ["-vV"], { env, capture: true })
    .split(/\r?\n/)
    .find((line) => line.startsWith("host:"));
  if (!hostLine) {
    throw new Error("Could not parse the host triple from rustc -vV.");
  }

  const targetTriple = hostLine.slice("host:".length).trim();
  const extension = targetTriple.includes("windows") ? ".exe" : "";
  const source = path.join(repoRoot, "target", profile, `hmpd${extension}`);
  const destinationDir = path.join(appRoot, "src-tauri", "binaries");
  const destination = path.join(destinationDir, `hmpd-${targetTriple}${extension}`);

  if (!existsSync(source)) {
    throw new Error(`Built sidecar was not found: ${source}`);
  }
  mkdirSync(destinationDir, { recursive: true });
  copyFileSync(source, destination);
  console.log(`Staged Tauri sidecar: ${destination}`);

  return env;
}

if (path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    prepareSidecar(process.argv[2] ?? "debug");
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
}
