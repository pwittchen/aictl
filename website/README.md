# aictl website

Landing page for [aictl](https://github.com/pwittchen/aictl) — deployed at https://aictl.app.

Plain HTML/CSS/JS. No frameworks. [Bun](https://bun.sh) is used only as a
build tool to produce a minified `dist/` and to run a dev server.

## Develop

```bash
bun install
bun run dev           # http://localhost:3000
```

## Build

```bash
bun run build         # writes minified output to ./dist
```

Deploy `dist/` to any static host (GitHub Pages, Cloudflare Pages, Netlify, S3).

## Structure

```
index.html    # single page, semantic sections
style.css     # design tokens + components (dark brutalist, cyan accent)
script.js     # copy-to-clipboard + smooth scroll
build.ts      # Bun build: minifies HTML/CSS/JS into dist/
serve.ts      # dev server
```

The design follows [`DESIGN.md`](DESIGN.md) — pure `#1f2228` background, white
text, Geist Mono for display and buttons, Inter for body. The only color accent
is a cyan `#5ed3f3` (borrowed from the terminal app's banner), used sparingly
for the blinking caret, section kickers, and command-prompt glyphs.
