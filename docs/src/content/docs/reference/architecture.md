---
title: Architecture
description: How niminal fits together as a native coding agent.
---

niminal is a native C++ coding agent built on the `niminal::ai` library. The
executable handles workspace tools, sessions, permissions, extensions, and the
TUI. The library owns provider-neutral streaming, HTTP, provider adapters, and
the tool-capable agent loop.

## Surfaces

The same agent core runs in four modes:

| Mode | When |
| --- | --- |
| Interactive TUI | No CLI prompt and a TTY |
| Print mode | CLI prompt or piped stdin, one turn, then exit |
| JSON mode | `--mode json`, versioned JSONL on stdout |
| RPC mode | `--mode rpc`, JSON commands on stdin |

Headless modes have no approval UI. The TUI prompts before shell commands and
non-read-only extension tools unless YOLO mode is on.

## Sessions

Conversations are append-only JSONL files under `~/.niminal/sessions/`. Each file
records user messages, assistant replies, tool calls, tool results, compaction
summaries, and usage metadata. Resuming reloads provider and model from the
session, not from global defaults.

## Workspace tools

Built-in tools stay inside the workspace directory. `grep` and `glob` build their
file list from git-tracked and untracked paths and honor `.gitignore`.

External tools and extensions add capabilities through manifests in well-known
folders. Project copies load only after trust.

## Extensions

Extensions are separate processes that speak JSON lines. They can register tools,
slash commands, lifecycle hooks, footer status, and transcript widgets. External
tools are simpler one-shot executables invoked only when the model calls them.

## Providers

niminal normalizes requests to OpenAI-style messages and tools, then adapts them
per provider (Anthropic Messages, OpenAI Chat Completions, Google Gemini, and
others). Prompt caching breakpoints are applied on supported providers.

## Out of scope

niminal does not ship a hosted service, repo index daemon, LSP integration, or
built-in MCP client. Codex App Server is not wired. Plan mode is not exposed;
the agent always runs in act mode.

Other programs can link the `niminal::ai` CMake target for the provider SDK
without pulling in the CLI application.

## Next steps

- [Install](/guides/install/) to get the binary
- [Extensions and hooks](/guides/extensions-and-hooks/) to extend the agent
