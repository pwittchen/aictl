import MarkdownIt from "markdown-it";
import hljs from "highlight.js";

// Streaming-aware markdown renderer. The chat surface re-runs `render`
// on every token so the parser must be lenient — we let markdown-it's
// default behaviour handle a half-finished tag (it just renders what
// it has and ignores trailing junk). Code-fence highlighting only
// kicks in once the closing fence is seen, which is fine — partial
// fences render as plain text until the stream finishes.
const md: MarkdownIt = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: false,
  highlight(code: string, lang: string): string {
    if (lang && hljs.getLanguage(lang)) {
      try {
        return hljs.highlight(code, { language: lang, ignoreIllegals: true })
          .value;
      } catch {
        /* fall through to plain */
      }
    }
    return md.utils.escapeHtml(code);
  },
});

export function renderMarkdown(input: string): string {
  return md.render(input);
}
