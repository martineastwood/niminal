#!/usr/bin/env python3
"""Point the Homebrew cask at a release: update-cask.py Casks/niminal.rb 0.0.1-alpha.7 <sha256 of the dmg>."""

import re
import sys

path, version, sha = sys.argv[1:4]
if not re.fullmatch(r"[0-9a-f]{64}", sha):
    sys.exit(f"{sha!r} is not a SHA-256")
text = open(path).read()
text, versions = re.subn(r'(?m)^(\s*version )"[^"]*"', rf'\1"{version}"', text)
text, shas = re.subn(r'(?m)^(\s*sha256 )"[^"]*"', rf'\1"{sha}"', text)
if (versions, shas) != (1, 1):
    sys.exit(f"expected one version and one sha256 line in {path}, found {versions} and {shas}")
open(path, "w").write(text)
