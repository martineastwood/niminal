"""A finite tracker phrase in Python. Rows, tempo and note lengths belong entirely to this client.
Run with a server listening on port 7401: PYTHONPATH=clients/python python3 clients/examples/tracker.py [BPM]"""
import sys
import time

from niminal import Client

bpm = float(sys.argv[1]) if len(sys.argv) > 1 else 125.0
row_seconds = 60 / bpm / 4
rows = ["c3", None, "c3", "g3", None, "eb3", None, "bb3",
        "c4", None, "g3", None, "eb3", "g3", None, None]

engine = Client()
engine.connect()
try:
    engine.send_events([{
        "op": "define", "at": "now",
        "source": "instr tracker_tone(freq: hz) { osc(saw, freq) * env[0.15 | 80ms 0] }",
    }])
    clock = engine.clock()
    start = clock["sample"] + round(clock["sample_rate"] * 0.2)
    engine.send_events([
        {"op": "note", "target": "tracker_tone",
         "at": start + round(row * row_seconds * clock["sample_rate"]),
         "dur": f"{row_seconds * 0.8}sec", "args": {"freq": pitch}, "tag": "phrase"}
        for row, pitch in enumerate(rows) if pitch is not None
    ])
    # keep the connection open until the phrase and its release have sounded
    time.sleep(0.4 + len(rows) * row_seconds)
finally:
    engine.close()
