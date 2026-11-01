#!/usr/bin/env python3
"""Smoke-test an extracted Niminal macOS download without installing it."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile


def run(*args, **kwargs):
    try:
        return subprocess.run(list(map(str, args)), check=True, text=True, capture_output=True, timeout=90, **kwargs)
    except subprocess.CalledProcessError as error:
        print(error.stdout, error.stderr, file=sys.stderr)
        raise


def system_libraries_only(binary):
    for line in run("otool", "-L", binary).stdout.splitlines()[1:]:
        dependency = line.strip().split(" (", 1)[0]
        assert dependency.startswith(("/System/Library/", "/usr/lib/")), f"external dependency in {binary}: {dependency}"


def main():
    bundle = Path(sys.argv[1]).resolve()
    manifest = json.loads((bundle / "BUILD.json").read_text())
    for program in ["niminal", "niminal-live"]:
        binary = bundle / "bin" / program
        run("codesign", "--verify", "--strict", binary)
        assert run("lipo", "-archs", binary).stdout.strip() == "arm64", binary
        system_libraries_only(binary)
        version = run(binary, "--version").stdout.strip()
        assert version == f"{program} {manifest['version']}", version
    for name, executable in [("Niminal Stage", "niminal-stage"), ("Niminal Tracker", "niminal-tracker")]:
        app = bundle / f"{name}.app"
        run("codesign", "--verify", "--deep", "--strict", app)
        binary = app / "Contents" / "MacOS" / executable
        assert run("lipo", "-archs", binary).stdout.strip() == "arm64"
        system_libraries_only(binary)
    for name in ["Getting Started.txt", "LICENSE", "Examples/saw_lead.nml"]:
        assert (bundle / name).is_file(), name
    with tempfile.TemporaryDirectory(prefix="niminal-verify-") as temp:
        temp = Path(temp)
        with zipfile.ZipFile(bundle / "niminal-vscode.vsix") as archive:
            for name in ["extension/extension.js", "extension/binaries.js", "extension/node_modules/niminal-client/index.js",
                         "extension/node_modules/vscode-languageclient/package.json", "extension/node_modules/ws/package.json"]:
                assert name in archive.namelist(), f"VSIX is missing {name}"
            assert not any("@vscode/vsce" in name for name in archive.namelist()), "VSIX contains development dependencies"
            archive.extractall(temp)
        run("node", "-e", "require('./node_modules/niminal-client'); require('./node_modules/ws')", cwd=temp / "extension")
        run(bundle / "bin" / "niminal-live", "render", bundle / "Examples" / "saw_lead.nml", "--bars", "1", "--out", temp / "smoke.wav", cwd=temp)
        assert (temp / "smoke.wav").stat().st_size > 44, "render produced no audio file"
        messages = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"capabilities": {}}},
            {"jsonrpc": "2.0", "id": 2, "method": "shutdown"},
            {"jsonrpc": "2.0", "method": "exit"},
        ]
        frames = ""
        for message in messages:
            body = json.dumps(message)
            frames += f"Content-Length: {len(body.encode())}\r\n\r\n{body}"
        reply = run(bundle / "bin" / "niminal-live", "lsp", input=frames, cwd=temp).stdout
        assert '"capabilities"' in reply, "LSP did not answer initialize"
    print(f"Verified {manifest['version']}: signatures, arm64 binaries, CLI versions, VSIX dependencies, offline render and LSP.")


if __name__ == "__main__":
    main()
