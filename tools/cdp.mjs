// Drives the running app's WebView2 over CDP (for testing on a dev machine only).
// Run the app: WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9333" ShunguangDecor.exe
// Usage: node tools/cdp.mjs eval "return document.title"   |   node tools/cdp.mjs shot out.png   (CDP_PORT changes the port)
const [, , mode, arg] = process.argv;
const port = process.env.CDP_PORT || "9333";
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
// A web wallpaper is also a WebView2 in the same process (same debug port): skip its page.
const pages = targets.filter((t) => t.type === "page");
const page = pages.find((t) => /^https?:\/\/(localhost|tauri\.localhost)/.test(t.url)) ?? pages.find((t) => !t.url.includes("wallpaper.sgdecor.example")) ?? pages[0];
if (!page) throw new Error("no page target: " + JSON.stringify(targets));
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
};
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
if (mode === "eval") {
  const res = await send("Runtime.evaluate", { expression: `(async()=>{${arg}})()`, awaitPromise: true, returnByValue: true });
  const ex = res.result?.exceptionDetails; console.log(ex ? "EXC " + (ex.exception?.description ?? ex.text) : JSON.stringify(res.result?.result?.value, null, 1));
} else if (mode === "shot") {
  const res = await send("Page.captureScreenshot", { format: "png" });
  const fs = await import("node:fs");
  fs.writeFileSync(arg, Buffer.from(res.result.data, "base64"));
  console.log("saved", arg);
}
ws.close();
process.exit(0);
