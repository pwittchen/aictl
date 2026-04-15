// Simple dev server for the website. `bun run dev`
import { join } from "node:path";
import { existsSync, statSync } from "node:fs";

const root = import.meta.dir;
const port = Number(process.env.PORT) || 3000;

const types: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".css":  "text/css; charset=utf-8",
  ".js":   "application/javascript; charset=utf-8",
  ".svg":  "image/svg+xml",
  ".png":  "image/png",
  ".jpg":  "image/jpeg",
  ".ico":  "image/x-icon",
};

Bun.serve({
  port,
  async fetch(req) {
    const url = new URL(req.url);
    let path = decodeURIComponent(url.pathname);
    if (path === "/") path = "/index.html";
    const file = join(root, path);
    if (!file.startsWith(root) || !existsSync(file) || statSync(file).isDirectory()) {
      return new Response("Not found", { status: 404 });
    }
    const ext = path.slice(path.lastIndexOf("."));
    return new Response(Bun.file(file), {
      headers: { "content-type": types[ext] ?? "application/octet-stream" },
    });
  },
});

console.log(`serving http://localhost:${port}`);
