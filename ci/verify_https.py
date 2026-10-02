"""Verify DNS, TLS and provider HTTP without a real API key."""

import json
import os
import pathlib
import subprocess
import sys
import tempfile


def verify_https(command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=180)
    events = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
    errors = [event.get("message", "") for event in events if event.get("type") == "error"]
    if result.returncode != 1 or not any(message.startswith("http 401:") for message in errors):
        raise RuntimeError(f"HTTPS verification failed\n{result.stdout}\n{result.stderr}")


if __name__ == "__main__":
    binary = pathlib.Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as home:
        os.environ.update(HOME=home, OPENROUTER_API_KEY="bogus", SSL_CERT_FILE="", SSL_CERT_DIR="")
        verify_https([str(binary), "--provider", "openrouter", "--mode", "json", "--no-session", "say hi"])
    print("Verified HTTPS passed")
