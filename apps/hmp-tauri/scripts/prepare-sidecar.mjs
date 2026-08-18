import { copyFileSync, existsSync, mkdirSync, readdirSync, statSync } from "node:fs";
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

function configureGStreamerWindows(env) {
  const candidates = [
    env.GSTREAMER_1_0_ROOT_MSVC_X86_64,
    "C:\\gstreamer\\1.0\\msvc_x86_64",
    env.ProgramFiles && path.join(env.ProgramFiles, "gstreamer", "1.0", "msvc_x86_64"),
    env.LOCALAPPDATA && path.join(env.LOCALAPPDATA, "Programs", "gstreamer", "1.0", "msvc_x86_64"),
  ].filter(Boolean);

  const root = candidates.find(
    (candidate) =>
      existsSync(path.join(candidate, "bin", "gst-inspect-1.0.exe")) &&
      existsSync(path.join(candidate, "lib", "pkgconfig", "gstreamer-1.0.pc")),
  );

  if (!root) {
    throw new Error(
      "GStreamer MSVC x86_64 SDK was not found. Install the official Runtime and Development components from https://gstreamer.freedesktop.org/download/.",
    );
  }

  const binDir = path.join(root, "bin");
  const pkgConfigDir = path.join(root, "lib", "pkgconfig");
  const pathEntries = (env.PATH ?? "").split(path.delimiter).filter(Boolean);
  const pkgConfigEntries = (env.PKG_CONFIG_PATH ?? "").split(path.delimiter).filter(Boolean);

  env.GSTREAMER_1_0_ROOT_MSVC_X86_64 = root;
  env.PATH = [binDir, ...pathEntries.filter((entry) => entry.toLowerCase() !== binDir.toLowerCase())].join(path.delimiter);
  env.PKG_CONFIG_PATH = [
    pkgConfigDir,
    ...pkgConfigEntries.filter((entry) => entry.toLowerCase() !== pkgConfigDir.toLowerCase()),
  ].join(path.delimiter);

  console.log(`GStreamer SDK: ${root}`);
}

const nativeSysPackages = [
  "glib-sys",
  "gobject-sys",
  "gio-sys",
  "gstreamer-sys",
  "gstreamer-base-sys",
  "gstreamer-video-sys",
  "gstreamer-player-sys",
];

function cleanStaleNativeOutputs(profile, env) {
  const buildDir = path.join(repoRoot, "target", profile, "build");
  if (!existsSync(buildDir)) return;

  const entries = readdirSync(buildDir);
  const hasStaleOutput = nativeSysPackages.some((packageName) =>
    entries.some((entry) => {
      if (!entry.startsWith(`${packageName}-`)) return false;
      const output = path.join(buildDir, entry, "output");
      return existsSync(output) && statSync(output).size === 0;
    }),
  );

  if (!hasStaleOutput) return;

  console.log("Removing stale docs-only native dependency outputs.");
  const args = ["clean", "--manifest-path", path.join(repoRoot, "Cargo.toml")];
  for (const packageName of nativeSysPackages) {
    args.push("-p", packageName);
  }
  run("cargo", args, { env });
}

export function prepareSidecar(profile = "debug", baseEnv = process.env) {
  if (!new Set(["debug", "release"]).has(profile)) {
    throw new Error(`Unsupported sidecar profile: ${profile}`);
  }
  if (baseEnv.DOCS_RS === "1") {
    throw new Error("DOCS_RS=1 disables native GStreamer linking. Clear DOCS_RS before building hmpd.");
  }

  const env = { ...baseEnv };
  if (process.platform === "win32") {
    configureGStreamerWindows(env);
  }
  cleanStaleNativeOutputs(profile, env);

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
