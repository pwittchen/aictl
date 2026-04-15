# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this directory.

This directory is the landing page for [aictl](https://github.com/pwittchen/aictl), deployed at https://aictl.app. The parent repo's `CLAUDE.md` (one level up) covers the Rust CLI itself — this file covers only the website.

## Commands

```bash
bun install
bun run dev       # hot-reloading dev server on http://localhost:3000 (serve.ts)
bun run build     # minified output to ./dist (build.ts)
bun run clean     # rm -rf dist
```

There is no test suite, no linter, and no framework. Bun is the only toolchain.

## Architecture

Plain HTML/CSS/JS single-page site. Four source files drive everything:

- `index.html` — semantic sections, single page
- `style.css` — design tokens + components
- `script.js` — copy-to-clipboard + smooth scroll
- `build.ts` — Bun bundler minifies CSS and JS, hand-rolled HTML minifier (strips comments and collapses whitespace, but leaves `<!--[...]-->` IE-style conditionals alone), then **copies `../install.sh` from the parent repo into `dist/install.sh`** so the site can serve the one-liner installer from the same origin
- `serve.ts` — dev server with a path-traversal guard (`file.startsWith(root)`)

`dist/` is the deploy artifact — any static host works (GitHub Pages, Cloudflare Pages, Netlify, S3).

## Design system

The visual design is fully specified in [`DESIGN.md`](DESIGN.md) and must be followed for any UI changes. Load-bearing rules:

- Background is `#1f2228` (never pure black); primary text is `#ffffff`.
- Two fonts only: **Geist Mono** for display headlines and buttons (uppercase, 1.4px letter-spacing), **Inter** (substituting for `universalSans`) for body and section headings. Never mix these roles.
- The single color accent is cyan `#5ed3f3`, used sparingly (blinking caret, section kickers, command-prompt glyphs). Everything else is white-on-dark with opacity-based borders (`rgba(255,255,255,0.1)` default, `0.2` emphasized).
- Sharp corners (0px radius) by default; no box shadows anywhere — depth comes from border/background opacity.
- Hover **dims** interactive elements to `rgba(255,255,255,0.5)` rather than brightening them.

When adding or modifying components, cross-reference `DESIGN.md` sections 4 (component stylings) and 7 (do's and don'ts) before writing CSS.

## Conventions

- Long-form only, no frameworks, no dependencies beyond `bun-types`. Don't introduce React/Vue/Tailwind/etc.
- The `install.sh` copy step in `build.ts` depends on the file living at `../install.sh` — don't move it without updating the build.
