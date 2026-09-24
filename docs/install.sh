#!/bin/sh
set -eu

BIN=niminal
REPO="${NIMINAL_REPO:-martineastwood/niminal}"
INSTALL_DIR="${NIMINAL_INSTALL_DIR:-$HOME/.local/bin}"

log() {
  printf 'niminal: %s\n' "$1"
}

fail() {
  printf 'niminal: %s\n' "$1" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "'$1' is required"
}

main() {
  need curl
  need tar
  need awk

  case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) fail "unsupported operating system: $(uname -s)" ;;
  esac

  case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=arm64 ;;
    *) fail "unsupported architecture: $(uname -m)" ;;
  esac

  asset="${BIN}-${os}-${arch}.tar.gz"
  if [ -n "${NIMINAL_VERSION:-}" ]; then
    version="$NIMINAL_VERSION"
    case "$version" in
      v*) ;;
      *) version="v${version}" ;;
    esac
    base="https://github.com/${REPO}/releases/download/${version}"
    log "downloading ${version} (${os}/${arch})"
  else
    base="https://github.com/${REPO}/releases/latest/download"
    log "downloading latest release (${os}/${arch})"
  fi

  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT HUP INT TERM

  curl -fsSL --retry 3 --connect-timeout 10 --max-time 120 \
    "${base}/${asset}" -o "${tmp}/${asset}" \
    || fail "download failed from ${base}/${asset}"
  curl -fsSL --retry 3 --connect-timeout 10 --max-time 20 \
    "${base}/${asset}.sha256" -o "${tmp}/${asset}.sha256" \
    || fail "checksum download failed"

  expected="$(awk 'NF { print tolower($1); exit }' "${tmp}/${asset}.sha256")"
  [ "$(printf '%s' "$expected" | awk '{ print length }')" -eq 64 ] ||
    fail "release checksum is invalid"
  case "$expected" in
    *[!0-9a-f]*) fail "release checksum is invalid" ;;
  esac

  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${tmp}/${asset}" | awk '{ print $1 }')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "${tmp}/${asset}" | awk '{ print $1 }')"
  else
    fail "SHA-256 verification requires sha256sum or shasum"
  fi
  [ "$actual" = "$expected" ] || fail "checksum verification failed"

  tar -xzf "${tmp}/${asset}" -C "$tmp"
  mkdir -p "$INSTALL_DIR"
  mv "${tmp}/${BIN}-${os}-${arch}/${BIN}" "${INSTALL_DIR}/${BIN}"
  chmod +x "${INSTALL_DIR}/${BIN}"

  log "installed ${INSTALL_DIR}/${BIN}"
  case ":${PATH:-}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
      log "add ${INSTALL_DIR} to PATH to run ${BIN} from any shell"
      ;;
  esac
}

main "$@"
