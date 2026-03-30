# ISSUES

features/ideas/bug-fixes to be added in the future:

- [repl] [ui] [func] add model switch functionality during the session
- [repl] [ui] add provider and model display functionality during the session
- [repl] [func] add `/info` command, which will display details about current setup (provider, model, app version)
- [func] add `-a` short flag for auto
- [func] replace short flags `-M` for model, `-m` for message
- [func] reduce context window for gpt-4 model
- [docs] update arch diagrams
- [tool] improve `web_fetch` tool (or add another tool - e.g. `web_extract`) - it often says that page is "too technical", so in case it thinks this way, it should remove all the technical details and try to extract non-technical info relevant for the query
- [tool] add `get_datetime` tool to fetch current date and time and use it for consecutive queries
- [llm] add Gemini support
- [llm] add Mistral support
- [llm] add Z.ai support
- [llm] add DeepSeek support
- [llm] add Ollama support
- [llm] add native models support from disk (e.g. with ONNX or other format like that)
- [config] add possibility to configure additional customizable ASSISTANT PROMPT, which will be added to the system prompt
- [config] add possibility to manage multiple ASSISTANT PROMPTS saved in the config, which user can use depending on the use case
- [marketing] create project website
- [ci] create GH Action for project versioning based on git tags
- [tests] add some tests
- [config] add support for `AICTL.md` file (init/read) for providing context for the current dir (similar to claude/codex)
