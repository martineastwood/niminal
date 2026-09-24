#!/usr/bin/env python3
"""Exit only when stdin closes, and record that it did.

Niminal sends "shutdown" and closes stdin. An extension that watches EOF rather
than the message has to see it, which a sibling holding the pipe open would
hide. $NIMINAL_EOF_MARKER is written at EOF so a test can tell the difference.
"""
import json
import os
import sys

print(json.dumps({"type": "register", "commands": []}), flush=True)

for _ in sys.stdin:
    pass

marker = os.environ.get("NIMINAL_EOF_MARKER", "")
if marker:
    with open(marker, "w") as out:
        out.write("eof")
