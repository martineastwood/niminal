"""Check the actual release binary's startup and HTTPS on supported Linux families."""

import argparse
import pathlib
import platform
import subprocess

from verify_https import verify_https

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("binary", type=pathlib.Path)
parser.add_argument("--platform", choices=["linux/amd64", "linux/arm64"],
                    default="linux/arm64" if platform.machine() in {"arm64", "aarch64"}
                    else "linux/amd64")
args = parser.parse_args()
binary = args.binary.resolve()
images = {
    "ubuntu:18.04": "apt-get -qq update && apt-get -qq install -y ca-certificates >/dev/null",
    "ubuntu:20.04": "apt-get -qq update && apt-get -qq install -y ca-certificates >/dev/null",
    "ubuntu:22.04": "apt-get -qq update && apt-get -qq install -y ca-certificates >/dev/null",
    "debian:10": "",
    "debian:11": "",
    "debian:12": "apt-get -qq update && apt-get -qq install -y ca-certificates >/dev/null",
    "rockylinux:8": "",
    "rockylinux:9": "",
    "almalinux:9": "",
    "amazonlinux:2023": "",
    "fedora:38": "",
    "opensuse/leap:15.6": "",
}
for image, install_ca in images.items():
    command = ["docker", "run", "--rm", "--platform", args.platform, "-e", "HOME=/tmp", "-e",
               "OPENROUTER_API_KEY=bogus", "-v", f"{binary}:/x/niminal:ro", image]
    subprocess.run(command + ["/x/niminal", "--version"], check=True, timeout=120)
    if image in {"debian:10", "debian:11"}:
        suite = "buster" if image == "debian:10" else "bullseye"
        install_ca = (
            f"printf 'deb http://archive.debian.org/debian {suite} main\\n' > /etc/apt/sources.list"
            " && apt-get -o Acquire::Check-Valid-Until=false -qq update"
            " && apt-get -qq install -y ca-certificates >/dev/null"
        )
    script = (install_ca + " && " if install_ca else "") + \
        '/x/niminal --provider openrouter --mode json --no-session "say hi"'
    verify_https(command + ["sh", "-c", script])
    print(f"{image}: startup and verified HTTPS passed", flush=True)
