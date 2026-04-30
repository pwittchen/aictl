// Build script — minifies HTML, CSS, JS into ./dist using Bun's bundler.
import { mkdir, copyFile, readFile, writeFile, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join } from "node:path";

const root = import.meta.dir;
const dist = join(root, "dist");

async function main() {
  if (existsSync(dist)) await rm(dist, { recursive: true });
  await mkdir(dist, { recursive: true });

  // Minify CSS via Bun's CSS bundler.
  const cssBuild = await Bun.build({
    entrypoints: [join(root, "style.css")],
    outdir: dist,
    minify: true,
  });
  if (!cssBuild.success) {
    console.error(cssBuild.logs);
    process.exit(1);
  }

  // Minify JS.
  const jsBuild = await Bun.build({
    entrypoints: [join(root, "script.js")],
    outdir: dist,
    minify: true,
    target: "browser",
  });
  if (!jsBuild.success) {
    console.error(jsBuild.logs);
    process.exit(1);
  }

  // Minify HTML (whitespace + HTML comments, preserving pre/code content).
  for (const page of ["index.html", "guides.html", "server.html"]) {
    let html = await readFile(join(root, page), "utf8");
    const blocks: string[] = [];
    html = html.replace(/<(pre|code)\b[^>]*>[\s\S]*?<\/\1>/g, (m) => {
      blocks.push(m);
      return `\x00BLOCK${blocks.length - 1}\x00`;
    });
    html = html
      .replace(/<!--[^[][\s\S]*?-->/g, "")
      .replace(/>\s+</g, "><")
      .replace(/\s{2,}/g, " ")
      .trim();
    html = html.replace(/\x00BLOCK(\d+)\x00/g, (_, i) => blocks[+i]);
    await writeFile(join(dist, page), html);
  }

  // Copy install.sh from the parent repo so the site can serve it for one-liner installs.
  await copyFile(join(root, "..", "install.sh"), join(dist, "install.sh"));

  // Copy product screenshot used by the preview section.
  await copyFile(join(root, "screenshot.png"), join(dist, "screenshot.png"));

  // Mirror the same step for install-server.sh — produced under
  // `dist/server/install.sh` so https://aictl.app/server/install.sh
  // resolves alongside the existing CLI installer at /install.sh.
  await mkdir(join(dist, "server"), { recursive: true });
  await copyFile(join(root, "..", "install-server.sh"), join(dist, "server", "install.sh"));

  // Copy llms.txt for LLM-friendly site metadata (https://llmstxt.org/).
  await copyFile(join(root, "llms.txt"), join(dist, "llms.txt"));

  console.log("✓ built -> dist/");
  for (const f of ["index.html", "guides.html", "server.html", "style.css", "script.js", "install.sh", "server/install.sh", "llms.txt", "screenshot.png"]) {
    const path = join(dist, f);
    if (existsSync(path)) {
      const size = (await Bun.file(path).arrayBuffer()).byteLength;
      console.log(`  ${f.padEnd(14)} ${(size / 1024).toFixed(2)} KB`);
    }
  }
}

main();
