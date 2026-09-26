---
title: Architecture
description: How niminal fits together as a native coding agent.
---

You can use niminal interactively, run it from scripts, or embed its agent loop
in a C++ application. All modes support conversations with tool calls and
streamed model output.

## Surfaces

The same agent core runs in four modes:

| Mode | When |
| --- | --- |
| Interactive TUI | No CLI prompt and both stdin and stdout are TTYs |
| Print mode | CLI prompt, one turn, then exit |
| JSON mode | `--mode json`, versioned JSONL on stdout |
| RPC mode | `--mode rpc`, JSON commands on stdin |

Headless modes have no approval UI. The TUI prompts before shell commands,
extension tools, and external tools unless YOLO mode is on.

Print mode reads only the CLI prompt. JSON mode merges piped stdin with a CLI
prompt when stdin is not a TTY.

## Sessions

Conversations are append-only JSONL files under `~/.niminal/sessions/`. Each file
records user messages, assistant replies, tool calls, tool results, compaction
summaries, and usage metadata. Resuming reloads provider and model from the
session, not from global defaults.

## Workspace tools

Built-in tools stay inside the workspace directory. In a git repository, `grep`
and `glob` build their file list from the workspace index and honor `.gitignore`.
Outside git, the index comes from a directory walk without `.gitignore`
filtering.

External tools and extensions add capabilities through manifests in well-known
folders. Project copies load only after trust.

## Extensions

Extensions are separate processes that speak JSON lines. They can register tools,
slash commands, lifecycle hooks, footer status, and transcript widgets. External
tools are simpler one-shot executables invoked only when the model calls them.

## Providers

niminal keeps provider and model selection, credentials, and session history.
Each model step maps that history and the runtime tools into CAIL's public
generation API. Provider round-trip data stays attached to the message and is
sent back unchanged. CAIL owns the provider wire formats (Anthropic Messages,
OpenAI Responses, Chat Completions, Gemini, and others). Prompt caching
breakpoints are applied on supported providers before the CAIL call.

## Out of scope

niminal does not ship a hosted service, repo index daemon, LSP integration, or
built-in MCP client. Codex App Server is not wired. Plan mode is not exposed;
the agent always runs in act mode.

## Embed the agent

Link your C++23 application to the `niminal::ai` CMake target and supply a
configured CAIL model:

```cpp
#include <niminal/ai.hpp>
#include <cail/openai.hpp>
#include <iostream>

int main() {
  niminal::Agent agent;
  agent.language_model = cail::openai("gpt-5");
  agent.model = "gpt-5";
  agent.system = "Give short, practical answers.";
  std::cout << agent.run("How do I list files in a directory?") << '\n';
}
```

Set `OPENAI_API_KEY` before running this example. `agent.model` labels events
and saved replies; `agent.language_model` determines which model receives calls.
You can supply any CAIL language model, including one configured with a custom
endpoint. For standalone model calls without an agent loop, use CAIL directly.

## Next steps

- [Install](/guides/install/) to get the binary
- [Extensions and hooks](/guides/extensions-and-hooks/) to extend the agent
