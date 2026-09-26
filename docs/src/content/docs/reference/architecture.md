---
title: Architecture
description: How niminal fits together as a native coding agent.
---

niminal is a native C++ coding agent built on the `niminal::ai` library. The
executable handles workspace tools, sessions, permissions, extensions, and the
TUI. The library owns the tool-capable agent loop, application HTTP, and the
adapter that maps niminal sessions and tools onto CAIL, the C++ AI SDK used for
model transport and provider APIs.

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

niminal keeps provider and model selection, credentials, and session history in
OpenAI-style JSON. Each model step maps that history and the runtime tools into
CAIL's public generation API. CAIL owns the provider wire formats (Anthropic
Messages, OpenAI Responses, Chat Completions, Gemini, and others). Prompt
caching breakpoints are applied on supported providers before the CAIL call.

## Out of scope

niminal does not ship a hosted service, repo index daemon, LSP integration, or
built-in MCP client. Codex App Server is not wired. Plan mode is not exposed;
the agent always runs in act mode.

Other programs can link the `niminal::ai` CMake target for the agent loop
without pulling in the CLI application. Standalone model clients should use
CAIL directly.

## Next steps

- [Install](/guides/install/) to get the binary
- [Extensions and hooks](/guides/extensions-and-hooks/) to extend the agent
