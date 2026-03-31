# ISSUES

features/ideas/bug-fixes to be added in the future:

- [config] add possibility to manage multiple ASSISTANT PROMPTS saved in the config, which user can use depending on the use case - it can be done, by providing prompt file while running the program - this will give the user flexibility in terms of managing prompts and storing them and we avoid complexity of managing this on the app level - think if we should provide prompt file by param or by convention or both
- [config] [func] add `/init` tool, which will help to generate assistant prompt with the dir contents and/or general user instruction - filename can be by convention e.g. `AICTL.md`
- [func] research possibilities of adding new caching and data compression capabilities
- [func] consider adding session persistence/restoration - maybe consider this per assistant (it's related to another issue in this backlog) - messages can be stored in `.aict.session` file in the current dir and after session restoration it should be read, compacted saved and used for the future conversations - persistence should be invoked on purpose - e.g. with `--session` param
- [llm] add Gemini support
- [llm] add Mistral support
- [llm] add Z.ai support
- [llm] add DeepSeek support
- [llm] add Ollama support
- [llm] add native models support from disk (e.g. with ONNX or other format like that)
- [marketing] create project website
