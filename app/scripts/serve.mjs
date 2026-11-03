// Static server with SPA fallback for the built app. Usage: node serve.mjs <dir> <port>
// Port 0 takes a free port; the ready line says which ("serving <dir> on <port>").
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { join, extname, normalize } from "node:path";
const [dir, port] = [process.argv[2], Number(process.argv[3] ?? 8765)];
const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".webmanifest": "application/manifest+json", ".json": "application/json", ".woff2": "font/woff2" };
const server = createServer(async (req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^(\.\.[/\\])+/, "");
  let file = join(dir, path);
  try { if (!(await stat(file)).isFile()) throw 0; } catch { file = join(dir, "index.html"); }
  try {
    const body = await readFile(file);
    res.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream" });
    res.end(body);
  } catch { res.writeHead(404); res.end(); }
});
server.listen(port, "127.0.0.1", () => console.log(`serving ${dir} on ${server.address().port}`));
