---
title: RPC mode
description: Drive nimlet as a long-running JSONL process.
---

:::caution[Placeholder]
This page is a stub. Content is planned but not written yet.
:::

## Planned content

Port of `nimlet/docs/rpc.md` once the page is written.

- `nimlet --mode rpc`: one command per stdin line, JSON only on stdout
- Commands: `prompt`, `interrupt`, `get_state`, `shutdown`
- Required `id` and `type` fields, and response correlation
- Prompt acceptance (`started` vs `queued`), the one-deep queue, and rejection
- Queue events carrying `request_id`
- `interrupt` keeping the queued successor; `shutdown` clearing the queue
- EOF behaving like shutdown
- `get_state` payload: `session_id`, `mode`, `busy`, `queued`
- Why turn completion arrives as events rather than a second response
- Where diagnostics and startup messages are written
