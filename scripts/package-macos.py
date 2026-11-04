#!/usr/bin/env python3
"""Build a complete, ad-hoc-signed Apple Silicon alpha download (a .dmg). Publishes nothing."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def run(*args, cwd=ROOT, env=None, capture=False):
    print("+", shlex.join(map(str, args)), flush=True)
    result = subprocess.run(list(map(str, args)), cwd=cwd, env=env, check=True,
                            text=True, stdout=subprocess.PIPE if capture else None)
    return result.stdout.strip() if capture else None


def digest(path):
    with path.open("rb") as file:
        hasher = hashlib.sha256()
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            hasher.update(chunk)
        return hasher.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", help="release version, e.g. 0.0.1-alpha.1")
    parser.add_argument("--allow-dirty", action="store_true", help="build a local preview from uncommitted sources")
    args = parser.parse_args()
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?", args.version):
        parser.error("use a semantic version such as 0.0.1-alpha.1")
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        parser.error("the alpha package must be built on an Apple Silicon Mac")
    dirty = bool(run("git", "status", "--porcelain", capture=True))
    if dirty and not args.allow_dirty:
        parser.error("commit the sources first, or pass --allow-dirty for a local preview")
    commit = run("git", "rev-parse", "HEAD", capture=True)
    target = ROOT / "target" / "macos-alpha"
    output = ROOT / "releases"
    output.mkdir(exist_ok=True)
    archive = output / f"Niminal-{args.version}-macos-arm64.dmg"
    if archive.exists():
        parser.error(f"{archive.name} already exists; choose a new version or move the previous archive")
    env = os.environ.copy()
    # Override the repo's target-cpu=native, and share dependencies across the workspaces.
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env.update(RUSTFLAGS="-C target-cpu=apple-m1", CARGO_TARGET_DIR=str(target),
               MACOSX_DEPLOYMENT_TARGET="13.0", NIMINAL_RELEASE_VERSION=args.version, APPLE_SIGNING_IDENTITY="-")
    run("cargo", "build", "--locked", "--release", "-p", "niminal-cli", cwd=ROOT, env=env)
    apps = []
    for folder, name in [("stage", "Niminal Stage")]:
        run("npm", "ci", cwd=ROOT / folder)
        config = {
            "version": args.version, "productName": name,
            "build": {
                "beforeBuildCommand": "npm run build -- --outDir " + shlex.quote(str(target / "frontend" / folder)),
                "frontendDist": str(target / "frontend" / folder),
            },
            "bundle": {"macOS": {"signingIdentity": "-", "minimumSystemVersion": "13.0"}},
        }
        run("npm", "run", "tauri", "--", "build", "--bundles", "app", "--config", json.dumps(config),
            cwd=ROOT / folder, env=env)
        apps.append(target / "release" / "bundle" / "macos" / f"{name}.app")
    extension = ROOT / "editors" / "vscode"
    run("npm", "ci", cwd=extension)
    with tempfile.TemporaryDirectory(prefix="niminal-alpha-") as temp:
        temp = Path(temp)
        bundle = temp / "Niminal"
        bundle.mkdir()
        (bundle / "bin").mkdir()
        shutil.copy2(target / "release" / "niminal", bundle / "bin" / "niminal")
        run("codesign", "--force", "--sign", "-", "--timestamp=none", bundle / "bin" / "niminal")
        for app in apps:
            run("ditto", app, bundle / app.name)
        # Materialize the local file: dependency; the VSIX must work without the checkout.
        staged_extension = temp / "extension"
        staged_extension.mkdir()
        for name in ["package.json", "package-lock.json", "README.md", "extension.js", "blocks.js", "project.js",
                     "panel.js", "learn.js", "compat.js", "live-client.js", "binaries.js", "language-configuration.json",
                     "score-language-configuration.json", "syntaxes", "snippets", "media"]:
            source = extension / name
            if source.is_dir():
                shutil.copytree(source, staged_extension / name)
            else:
                shutil.copy2(source, staged_extension / name)
        shutil.copy2(ROOT / "LICENSE", staged_extension / "LICENSE")
        # Install only runtime dependencies into a separate staging tree.
        package = json.loads((staged_extension / "package.json").read_text())
        package.pop("devDependencies", None)
        # the extension and Stage of one download say they are the same release, so a mismatch is noticed
        package["version"] = args.version
        package["dependencies"]["niminal-client"] = "file:" + str(ROOT / "clients" / "js")
        (staged_extension / "package.json").write_text(json.dumps(package, indent=2) + "\n")
        (staged_extension / "package-lock.json").unlink()
        run("npm", "install", "--omit=dev", "--ignore-scripts", "--install-links", cwd=staged_extension)
        package["dependencies"]["niminal-client"] = json.loads((ROOT / "clients" / "js" / "package.json").read_text())["version"]
        (staged_extension / "package.json").write_text(json.dumps(package, indent=2) + "\n")
        # This staging tree contains just distributable files and production dependencies.
        (staged_extension / ".vscodeignore").write_text("package-lock.json\n")
        run(extension / "node_modules" / ".bin" / "vsce", "package", "--allow-missing-repository",
            "--baseContentUrl", f"https://github.com/martineastwood/niminal/blob/{commit}/editors/vscode/",
            "--baseImagesUrl", f"https://raw.githubusercontent.com/martineastwood/niminal/{commit}/editors/vscode/",
            "--out", bundle / "niminal-vscode.vsix", cwd=staged_extension)
        shutil.copytree(ROOT / "examples", bundle / "Examples", ignore=shutil.ignore_patterns(".DS_Store"))
        shutil.copy2(ROOT / "LICENSE", bundle / "LICENSE")
        shutil.copy2(ROOT / "scripts" / "macos-getting-started.txt", bundle / "Getting Started.txt")
        manifest = {
            "version": args.version, "commit": commit, "dirty": dirty,
            "architecture": "arm64", "minimum_macos": "13.0", "build_macos": platform.mac_ver()[0],
            "signing": "ad-hoc", "notarized": False,
            "extension_version": package["version"],
        }
        (bundle / "BUILD.json").write_text(json.dumps(manifest, indent=2) + "\n")
        # Verify the staged artifact, including dependencies after extracting the VSIX.
        run("python3", ROOT / "scripts" / "verify-macos-package.py", bundle, env=env)
        # A disk image with the folder beside an Applications shortcut: drag the folder across to install.
        disk = temp / "disk"
        disk.mkdir()
        shutil.move(bundle, disk / "Niminal")
        (disk / "Applications").symlink_to("/Applications")
        run("hdiutil", "create", "-volname", "Niminal", "-srcfolder", disk, "-format", "UDZO", "-ov", archive)
    checksum = archive.with_suffix(".dmg.sha256")
    checksum.write_text(f"{digest(archive)}  {archive.name}\n")
    print(f"\nCreated {archive}\nSHA-256: {digest(archive)}\nNo files have been published.")


if __name__ == "__main__":
    main()
