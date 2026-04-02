# ISSUES

features/ideas/bug-fixes to be added in the future:

- [func] [config] research & implement more secure way of storing api keys instead of the plain text - consider using keyring, and use env in plain text as a fallback - add some note in the welcome banner and info that keys are secure or not
- [func] [ui] [config] once secure keys storage will be implemented in the app, in the `/info` command we can show info that specified API key is set or not and if it's secured (saved in keyring) or not (saved in config file in plain text)
- [func] [config] add configuration functionality, so when user runs app for the first time, he can configure it with api keys, provider and model - do this with `/setup` command
- [config] add possibility to manage multiple AGENT PROMPTS saved in the config, which user can use depending on the use case - it can be done, by providing prompt file while running the program - this will give the user flexibility in terms of managing prompts and storing them and we avoid complexity of managing this on the app level - think if we should provide prompt file by param or by convention or both - this can be done via keeping prompt definitions in `~/.aictl/agents/` dir, and there should be commands allowing user to manage assistants: create, use, discard, delete
- [func] consider adding session persistence/restoration - maybe consider this per assistant (it's related to another issue in this backlog) - messages can be stored in `.aictl.session` file in the current dir and after session restoration it should be read, compacted saved and used for the future conversations - persistence should be invoked on purpose - e.g. with `--session`/`--memory` param - it can be done via `~/.aictl/sessions/` dir, where user can persist all the sessions with name and datetime, if there's no name defined, there should be random name; later user can restore session
- [llm] add Gemini support
- [llm] add Grok support
- [llm] add Mistral support
- [llm] add Z.ai support
- [llm] add DeepSeek support
- [llm] add Ollama support
- [llm] add native models support from disk (e.g. with ONNX or other format like that)
- [tool] [func] add image processing capability
- [tool] [func] add document processing capability (pdf/docx)
- [optimization] implement prompt caching
- [optimization] implement selective history with last messages window and optional compact - allow user to choose thinking/reasoning mode: smart/fast or memory: long/short
- [marketing] create project website
