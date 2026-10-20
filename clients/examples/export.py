"""Run a daemon, then PYTHONPATH=clients/python python3 clients/examples/export.py."""
import os
import time
from niminal import Client

client = Client(url=f"ws://127.0.0.1:{os.environ.get("NIMINAL_ENGINE_PORT", "7401")}/")
client.connect()
try:
    job = client.render(**{
        "source": "instr tone(freq: hz) { osc(sine, freq) * env[0.2 | 100ms 0] }",
        "events": [{"op": "note", "target": "tone", "at": 0, "dur": "250ms", "args": {"freq": "440hz"}}],
        "frames": 24000, "sample_rate": 48000, "channels": 2, "tail": False,
    })
    while True:
        status = client.job_status(job["id"])
        if status["state"] != "running":
            break
        time.sleep(0.02)
    if status["state"] != "completed":
        raise RuntimeError(status.get("error", status["state"]))
    handle = status["outputs"][0]["file"]
    client.download_file(handle, "niminal-export.wav")
    client.remove_file(handle)
finally:
    client.close()
