#!/usr/bin/env python3
"""Start a child process tree and stop it again on shutdown.

The child ignores SIGTERM and runs in its own session, like a subagent started
with setsid, so only this extension can clean it up: it SIGTERMs the group, then
SIGKILLs it after 2.5 seconds. The child pid lands in $NIMINAL_ORPHAN_PID_FILE so
a test can check the tree is gone; without that variable the fixture starts
nothing.
"""
import json
import os
import signal
import subprocess
import sys
import time

child = None
if os.environ.get("NIMINAL_ORPHAN_PID_FILE"):
    child = subprocess.Popen(["/bin/sh", "-c", 'trap "" TERM INT; sleep 60'],
                             start_new_session=True)
    with open(os.environ["NIMINAL_ORPHAN_PID_FILE"], "w") as out:
        out.write(str(child.pid))


def send(value):
    print(json.dumps(value), flush=True)


send({"type": "register", "commands": []})


def stop_tree():
    if child is None:
        return
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.time() + 2.5
    while time.time() < deadline and child.poll() is None:
        time.sleep(0.05)
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()


for line in sys.stdin:
    if json.loads(line).get("type") == "shutdown":
        break
stop_tree()
