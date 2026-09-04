# Agent Joe

An open source TUI-based coding tool that only works with rust
and does not have access to the shell.

## Why

I just hate the fact that cli tools with connections to llm providers can execute any arbitrary command on my terminal.

With the rust only requirement, I can also reduce the number of possible actions to just rust specific ones, lowering the total tool count.

And this is a fun project to work on.

## State
Works pretty well currently, still doesn't do as good of a job as codex,
I think the main reason being the prompts not being as good and not having a plan mode.

The [implementation plan](IMPLEMENTATION_PLAN.md) covers the runtime, context,
validation, and collaboration improvements, with dependencies and acceptance criteria.

## Flags
The tui supports some flags

| Flag       | Description                                                     |
|------------|-----------------------------------------------------------------|
| --debug    | Adds significantly more logging and prints out thinking tokens. |
| --simple   | Multi-agent mode will be disabled, will work just like codex    |

## Supported llm providers

| Provider   | Support                                                |
|------------|--------------------------------------------------------|
| OpenAI     | Fully supported                                        |
| Codex      | Fully supported                                        |
| Claude     | Should work, haven't tested it in a minute due to cost |
| OpenRouter | Does not support web_search but everything should work |
| Local      | Response api fully supported                           |

Joe preserves returned OpenAI reasoning state and assistant message phases between
tool calls, along with typed JSON arguments and Claude thinking signatures.
Requests to the public OpenAI API ask for encrypted reasoning state automatically.
For Codex auth, OpenRouter, local servers, and custom API endpoints, set
`request_encrypted_reasoning = true` in the existing OpenAI provider configuration
only when that endpoint supports `include: ["reasoning.encrypted_content"]`.
Those routes omit the optional request field by default and preserve any reasoning
state the server returns. Setting the option to `false` disables the request field.

`/clear` refreshes workspace context and retains the worker's operating instructions.

## Build & Run

```sh
cargo run --relase
```

## Keybindings

The TUI is similar to claude code and codex with one major difference. Vim bindings are foced upon you.

## Tools

TBD
