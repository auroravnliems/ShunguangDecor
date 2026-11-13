// Build shunguang_explorer.dll (crate sg-explorer-hook) ra target/hook/release/sg_explorer_hook.dll.
//
// - A separate target directory: does not pollute the app's cache.
// - Static CRT (+crt-static): a DLL loaded into explorer.exe must not depend on vcruntime140.dll (the user's machine may not have it).
// - src-tauri/build.rs embeds this file into the exe if present; if absent, the app reports "missing DLL" and uses the fallback method.
import { spawnSync } from "node:child_process";

const rustflags = [process.env.RUSTFLAGS, "-C target-feature=+crt-static"].filter(Boolean).join(" ");
const args = ["build", "-p", "sg-explorer-hook", "--release", "--target-dir", "target/hook", ...process.argv.slice(2)];
const r = spawnSync("cargo", args, { stdio: "inherit", env: { ...process.env, RUSTFLAGS: rustflags } });
if (r.error) {
  console.error(`build-hook: cannot run cargo: ${r.error.message}`);
  process.exit(1);
}
process.exit(r.status ?? 1);
