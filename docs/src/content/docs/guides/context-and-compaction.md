---
title: Context and compaction
description: Context limits, automatic summarization, manual /compact, and overflow recovery.
---

Long sessions eventually exceed what the model can accept. niminal estimates token
use from session content, compacts older turns into a summary, and keeps recent
history verbatim.

## When compaction runs

Automatic compaction runs when all of these are true:

- `compaction_enabled` is `true` (the default)
- Estimated session tokens exceed `context_window - reserve_tokens`

Defaults:

| Setting | Default |
| --- | --- |
| `context_window` | `128000` |
| `reserve_tokens` | `16384` |
| `keep_recent_tokens` | `20000` |

Token estimates use roughly four characters per token. The summary generation
itself is capped at 4096 output tokens.

If a provider request fails with a context overflow error, niminal compacts and
retries once even when automatic compaction did not run earlier.

## Manual compaction

```text
/compact
/compact focus on the parser changes only
```

Manual compaction uses the same keep-recent logic. An optional instruction steers
what the summary emphasizes.

The JSONL file still holds the full history. The model only sees the latest summary
plus the kept tail.

Compaction appends a `{"type":"compaction",...}` record to the session file with
the summary text and bookkeeping fields.

## Extension hooks

Extensions can subscribe to:

- `session_before_compact` to block compaction or add an instruction
- `session_compact` to supply a complete compaction result

See [Extensions and hooks](/guides/extensions-and-hooks/) for hook payloads.

## Disabling automatic compaction

Set `"compaction_enabled": false` in `~/.niminal/config.json`. Manual `/compact`
and overflow recovery still work when you ask for them or when the provider rejects
an oversized request.

## Next steps

- [Configuration](/guides/configuration/) for compaction tuning keys
- [Sessions](/guides/sessions/) for what stays on disk after compaction
