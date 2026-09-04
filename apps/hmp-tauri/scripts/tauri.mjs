import { spawn } from "node:child_process";
import path from "node:path";
import { appRoot, prepareSidecar } from "./prepare-sidecar.mjs";

const [command, ...args] = process.argv.slice(2);
if (!command) {
  console.error("A Tauri command is required (for example: pnpm tauri dev).");
  process.exit(1);
}

try {
  let env = { ...process.env };
  const config = {};

  if (command === "dev") {
    env = prepareSidecar("debug", env);
    config.build = { beforeDevCommand: "pnpm dev" };
  } else if (command === "build") {
    env = prepareSidecar("release", env);
  }

  const tauriArgs = [command, ...args];
  if (command === "dev") {
    tauriArgs.push("--config", JSON.stringify(config));
  }

  const tauriCli = path.join(appRoot, "node_modules", "@tauri-apps", "cli", "tauri.js");
  const child = spawn(process.execPath, [tauriCli, ...tauriArgs], {
    cwd: appRoot,
    env,
    stdio: "inherit",
  });
  child.on("error", (error) => {
    console.error(error.message);
    process.exit(1);
  });
  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 1);
  });
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}
