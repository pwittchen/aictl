import hljs from "highlight.js";

/// Extension → highlight.js language id. Kept small on purpose: only
/// covers the languages we'd actually expect to see inside a project's
/// workspace, plus a handful of config formats. Anything outside this
/// table renders as escaped plain text — better than a wrong language
/// painting noise across the buffer.
const EXT_TO_LANG: Record<string, string> = {
  ts: "typescript",
  tsx: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  rs: "rust",
  py: "python",
  rb: "ruby",
  go: "go",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  swift: "swift",
  c: "c",
  h: "c",
  cpp: "cpp",
  cxx: "cpp",
  cc: "cpp",
  hpp: "cpp",
  cs: "csharp",
  php: "php",
  lua: "lua",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  md: "markdown",
  markdown: "markdown",
  json: "json",
  yml: "yaml",
  yaml: "yaml",
  toml: "toml",
  xml: "xml",
  html: "xml",
  htm: "xml",
  svg: "xml",
  css: "css",
  scss: "scss",
  sass: "scss",
  less: "less",
  sql: "sql",
  diff: "diff",
  patch: "diff",
  dockerfile: "dockerfile",
  ini: "ini",
  cfg: "ini",
  env: "bash",
  graphql: "graphql",
  gql: "graphql",
};

/// Pick a highlight.js language id from a workspace-relative path.
/// Filenames without an extension fall through unless a special case
/// matches the bare filename (Dockerfile, Makefile, etc.).
export function languageForPath(path: string): string | null {
  const base = path.split("/").pop() ?? path;
  const lower = base.toLowerCase();
  if (lower === "dockerfile" || lower.startsWith("dockerfile.")) {
    return "dockerfile";
  }
  if (lower === "makefile" || lower === "gnumakefile") {
    return "makefile";
  }
  const dot = lower.lastIndexOf(".");
  if (dot === -1 || dot === lower.length - 1) return null;
  const ext = lower.slice(dot + 1);
  return EXT_TO_LANG[ext] ?? null;
}

const escapeHtml = (s: string): string =>
  s.replace(/[&<>]/g, (c) => {
    if (c === "&") return "&amp;";
    if (c === "<") return "&lt;";
    return "&gt;";
  });

/// Run highlight.js on `code` for the given language id, falling back to
/// HTML-escaped plain text on any failure (unknown language, syntax
/// errors that hljs choked on, etc.).
export function highlightCode(code: string, lang: string | null): string {
  if (lang && hljs.getLanguage(lang)) {
    try {
      return hljs.highlight(code, { language: lang, ignoreIllegals: true })
        .value;
    } catch {
      /* fall through */
    }
  }
  return escapeHtml(code);
}
