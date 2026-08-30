#!/bin/sh
set -eu

REPO="fredrir/ui-box"
BINARIES="ui-box ui-box-mcp"

say() { printf '%s\n' "$*"; }
info() { printf 'ui-box: %s\n' "$*" >&2; }
warn() { printf 'ui-box: warning: %s\n' "$*" >&2; }

die() {
  printf 'ui-box: error: %s\n' "$1" >&2
  shift
  while [ $# -gt 0 ]; do
    printf '  %s\n' "$1" >&2
    shift
  done
  exit 1
}

have() { command -v "$1" >/dev/null 2>&1; }

usage() {
  cat <<EOF
Install the ui-box binaries.

  curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | sh

Options, each with an environment equivalent:

  --version TAG    UIBOX_VERSION       release to install, default: latest
  --dir DIR        UIBOX_INSTALL_DIR   where to install, default: ~/.local/bin
  --help
EOF
}

VERSION="${UIBOX_VERSION:-}"
INSTALL_DIR="${UIBOX_INSTALL_DIR:-}"

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || die "--version needs a release tag"
      VERSION="$2"
      shift 2
      ;;
    --version=*)
      VERSION="${1#--version=}"
      shift
      ;;
    --dir)
      [ $# -ge 2 ] || die "--dir needs a directory"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --dir=*)
      INSTALL_DIR="${1#--dir=}"
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1" "run with --help for usage"
      ;;
  esac
done

if [ -z "$INSTALL_DIR" ]; then
  [ -n "${HOME:-}" ] || die "HOME is not set, so there is no default install directory" \
    "Choose one:  UIBOX_INSTALL_DIR=/usr/local/bin sh install.sh"
  INSTALL_DIR="${HOME}/.local/bin"
fi

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin)
      if [ "$arch" = "x86_64" ] &&
        [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
        arch="arm64"
      fi
      case "$arch" in
        arm64) printf 'aarch64-apple-darwin\n' ;;
        x86_64) printf 'x86_64-apple-darwin\n' ;;
        *) return 1 ;;
      esac
      ;;
    Linux)
      case "$arch" in
        x86_64 | amd64) printf 'x86_64-unknown-linux-gnu\n' ;;
        aarch64 | arm64) printf 'aarch64-unknown-linux-gnu\n' ;;
        *) return 1 ;;
      esac
      ;;
    *) return 1 ;;
  esac
}

if [ "$(uname -s)" = "Linux" ] &&
  [ -n "$(find /lib /lib64 -maxdepth 1 -name 'ld-musl-*' -print 2>/dev/null | head -n 1)" ]; then
  die "this is a musl system and the published Linux binaries are linked against glibc" \
    "There is no musl release yet. Build from source instead:" \
    "  nix profile install github:${REPO}#ui-box"
fi

TARGET="$(detect_target)" || die \
  "no ui-box release is published for $(uname -s) $(uname -m)" \
  "Released targets:" \
  "  x86_64-unknown-linux-gnu    aarch64-unknown-linux-gnu" \
  "  x86_64-apple-darwin         aarch64-apple-darwin" \
  "Build from source instead:" \
  "  nix profile install github:${REPO}#ui-box"

if have curl; then
  DOWNLOADER="curl"
elif have wget; then
  DOWNLOADER="wget"
else
  die "neither curl nor wget is on PATH" "install one of them and run this again"
fi

have tar || die "tar is not on PATH" "install tar and run this again"

fetch() {
  case "$DOWNLOADER" in
    curl) curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1" ;;
    wget) wget -q -O "$2" "$1" ;;
  esac
}

resolve_latest() {
  if [ "$DOWNLOADER" = "curl" ]; then
    resolved="$(curl -fsSL --proto '=https' --tlsv1.2 -o /dev/null -w '%{url_effective}' \
      "https://github.com/${REPO}/releases/latest")" || return 1
    case "$resolved" in
      */tag/*) printf '%s\n' "${resolved##*/tag/}" ;;
      *) return 1 ;;
    esac
  else
    wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" |
      sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
      head -n 1
  fi
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | awk '{print $1}'
  elif have shasum; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif have openssl; then
    openssl dgst -sha256 "$1" | awk '{print $NF}'
  else
    return 1
  fi
}

if [ -z "$VERSION" ]; then
  VERSION="$(resolve_latest || true)"
  [ -n "$VERSION" ] || die \
    "cannot work out the latest ui-box release" \
    "GitHub is unreachable, rate-limiting this host, or ${REPO} has no release yet." \
    "Pin one explicitly:" \
    "  curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | UIBOX_VERSION=v0.1.0 sh"
fi

case "$VERSION" in
  v*) ;;
  *) VERSION="v${VERSION}" ;;
esac

ASSET="ui-box-${VERSION}-${TARGET}.tar.gz"
BASE="https://github.com/${REPO}/releases/download/${VERSION}"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/uibox-install.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT INT TERM

info "installing ${VERSION} for ${TARGET}"

fetch "${BASE}/${ASSET}" "${WORK}/${ASSET}" || die \
  "cannot download ${BASE}/${ASSET}" \
  "Check that ${VERSION} exists and ships an asset for ${TARGET}:" \
  "  https://github.com/${REPO}/releases"

fetch "${BASE}/SHA256SUMS" "${WORK}/SHA256SUMS" || die \
  "cannot download ${BASE}/SHA256SUMS" \
  "The release exists but nothing can verify it, so this script will not install it."

EXPECTED="$(awk -v want="$ASSET" '$2 == want || $2 == "*" want { print $1; exit }' \
  "${WORK}/SHA256SUMS")"
[ -n "$EXPECTED" ] || die \
  "SHA256SUMS for ${VERSION} does not list ${ASSET}" \
  "Refusing to install a binary nothing vouches for."

ACTUAL="$(sha256_of "${WORK}/${ASSET}")" || die \
  "no SHA-256 tool on PATH (looked for sha256sum, shasum, openssl)" \
  "Refusing to install without verifying the download."

if [ "$EXPECTED" != "$ACTUAL" ]; then
  die "checksum mismatch for ${ASSET}" \
    "expected  ${EXPECTED}" \
    "got       ${ACTUAL}" \
    "Refusing to install. Try again; if it persists, report it at" \
    "  https://github.com/${REPO}/issues"
fi

tar -xzf "${WORK}/${ASSET}" -C "$WORK" || die "cannot unpack ${ASSET}"

mkdir -p "$INSTALL_DIR" || die \
  "cannot create ${INSTALL_DIR}" \
  "Choose a writable directory:  UIBOX_INSTALL_DIR=/somewhere sh install.sh"

for binary in $BINARIES; do
  [ -f "${WORK}/${binary}" ] || die "${ASSET} does not contain ${binary}"
  cp "${WORK}/${binary}" "${INSTALL_DIR}/${binary}.new" || die \
    "cannot write to ${INSTALL_DIR}" \
    "Choose a writable directory:  UIBOX_INSTALL_DIR=/somewhere sh install.sh"
  chmod 0755 "${INSTALL_DIR}/${binary}.new"
  mv -f "${INSTALL_DIR}/${binary}.new" "${INSTALL_DIR}/${binary}"
done

if ! SMOKE="$("${INSTALL_DIR}/ui-box" --version 2>&1)"; then
  case "$SMOKE" in
    *GLIBC* | *libc*)
      die "the binary installed but will not run here" \
        "$SMOKE" \
        "The Linux releases are built against the glibc of the GitHub ubuntu runner." \
        "On an older distribution, build from source instead:" \
        "  nix profile install github:${REPO}#ui-box"
      ;;
    *)
      die "the binary installed but will not run here" "$SMOKE"
      ;;
  esac
fi

DOM="$(command -v ui-box-dom 2>/dev/null || true)"
VISION="$(command -v uibox-vision 2>/dev/null || true)"

say ""
say "  ${SMOKE}"
for binary in $BINARIES; do
  say "    ${INSTALL_DIR}/${binary}"
done
say ""

case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    warn "${INSTALL_DIR} is not on your PATH"
    say "    export PATH=\"${INSTALL_DIR}:\$PATH\""
    say ""
    ;;
esac

say "  Remote backend (ssh://): ready."
say "    The driver runs on the lab host, where the display is; these binaries"
say "    are the client that drives it over ssh."
say "      export UIBOX_BACKEND=ssh://fredrir@ui-box-backend"
say ""

if [ -n "$DOM" ] && [ -n "$VISION" ]; then
  say "  Local backend (local://): ready."
  say "      ui-box-dom    ${DOM}"
  say "      uibox-vision  ${VISION}"
else
  say "  Local backend (local://): not ready."
  say "    local:// also needs a Node driver and a Python tool, which are not part"
  say "    of these binaries:"
  if [ -n "$DOM" ]; then
    say "      ui-box-dom    ${DOM}"
  else
    say "      ui-box-dom    missing"
  fi
  if [ -n "$VISION" ]; then
    say "      uibox-vision  ${VISION}"
  else
    say "      uibox-vision  missing"
  fi
  say "    Install them with Nix:"
  say "      nix profile install github:${REPO}#ui-box-dom github:${REPO}#uibox-vision"
fi

say ""
say "  Next:  ui-box doctor"
say "  MCP:   claude mcp add ui-box -- ${INSTALL_DIR}/ui-box-mcp"
