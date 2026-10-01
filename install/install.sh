#!/bin/sh
# HooCode one-click installer (macOS / Linux). Installs the Rust build.
#
#   curl -fsSL https://kolisachint.github.io/hoocode/install.sh | sh
#
# What it does:
#   1. Picks the release archive for this OS and CPU. Linux builds are static
#      (musl), so one binary runs on any distro.
#   2. Downloads it and checks it against the release's SHA256SUMS.
#   3. Puts the binary at ~/.hoocode/bin/hoocode, with `hoo` linked to it.
#   4. Adds ~/.hoocode/bin to PATH in your shell rc files (unless told not to).
#
# No root, no Node, nothing outside the install dir except the PATH line.
# The TypeScript build is separate: `hoocode-ts`, from
#   curl -fsSL https://kolisachint.github.io/hoocode-ts/install.sh | sh
#
# Options (flags or environment variables):
#   --version <v>        HOOCODE_VERSION      release tag to install (default: latest)
#   --dir <path>         HOOCODE_INSTALL_DIR  install root (default: ~/.hoocode)
#   --no-modify-path     HOOCODE_NO_MODIFY_PATH=1  do not touch shell rc files
#   -h, --help           show this help
#
#   HOOCODE_RELEASE_BASE_URL   fetch archives from <url>/<tag>/ instead of
#                              GitHub releases (mirrors, offline tests)

set -eu

REPO="kolisachint/hoocode"
WEBSITE="https://kolisachint.github.io/hoocode/"

VERSION="${HOOCODE_VERSION:-latest}"
INSTALL_DIR="${HOOCODE_INSTALL_DIR:-$HOME/.hoocode}"
NO_MODIFY_PATH="${HOOCODE_NO_MODIFY_PATH:-0}"

# ---------------------------------------------------------------- output ----
# Colour only on a terminal, and never as the only signal: every line also says
# what happened in words.
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_DIM='\033[2m'; C_B='\033[1m'; C_OK='\033[32m'; C_WARN='\033[33m'; C_ERR='\033[31m'; C_ACC='\033[36m'; C_0='\033[0m'
else
    C_DIM=''; C_B=''; C_OK=''; C_WARN=''; C_ERR=''; C_ACC=''; C_0=''
fi

say()  { printf '%b\n' "$*"; }
step() { printf '%b\n' "${C_ACC}==>${C_0} $*"; }
warn() { printf '%b\n' "${C_WARN}warning:${C_0} $*" >&2; }
die()  { printf '%b\n' "${C_ERR}error:${C_0} $*" >&2; exit 1; }

usage() {
    awk 'NR==1 {next} /^[^#]/ {exit} {sub(/^# ?/, ""); print}' "$0"
    exit 0
}

# ------------------------------------------------------------- arguments ----
while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
        --version=*) VERSION="${1#*=}"; shift ;;
        --dir) [ $# -ge 2 ] || die "--dir needs a value"; INSTALL_DIR="$2"; shift 2 ;;
        --dir=*) INSTALL_DIR="${1#*=}"; shift ;;
        --no-modify-path) NO_MODIFY_PATH=1; shift ;;
        -h|--help) usage ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

# ------------------------------------------------------------ toolchecks ----
have() { command -v "$1" >/dev/null 2>&1; }

if have curl; then
    DL="curl"
elif have wget; then
    DL="wget"
else
    die "need curl or wget on PATH to download anything."
fi

fetch() {
    # fetch <url> <dest>
    if [ "$DL" = "curl" ]; then
        curl -fsSL --retry 3 --retry-delay 1 -o "$2" "$1"
    else
        wget -q -t 3 -O "$2" "$1"
    fi
}

fetch_stdout() {
    if [ "$DL" = "curl" ]; then
        curl -fsSL --retry 3 --retry-delay 1 "$1"
    else
        wget -q -t 3 -O - "$1"
    fi
}

have tar || die "need tar on PATH to unpack the release."

if have sha256sum; then
    SHA_CMD="sha256sum"
elif have shasum; then
    SHA_CMD="shasum -a 256"
else
    SHA_CMD=""
fi

# --------------------------------------------------------------- platform ---
UNAME_S="$(uname -s)"
UNAME_M="$(uname -m)"

case "$UNAME_M" in
    x86_64|amd64) ARCH="x86_64" ;;
    arm64|aarch64) ARCH="aarch64" ;;
    *) die "unsupported CPU: $UNAME_M. HooCode ships x86_64 and arm64 builds." ;;
esac

case "$UNAME_S" in
    Linux)
        OS="linux"
        TARGET="$ARCH-unknown-linux-musl"
        ;;
    Darwin)
        OS="darwin"
        # A shell running under Rosetta reports x86_64 on Apple silicon; the
        # native build is the better choice there.
        if [ "$ARCH" = "x86_64" ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
            ARCH="aarch64"
        fi
        TARGET="$ARCH-apple-darwin"
        ;;
    *) die "unsupported OS: $UNAME_S. The Rust build ships for macOS and Linux.
    On Windows, use the TypeScript build: see https://kolisachint.github.io/hoocode-ts/" ;;
esac

step "Platform: ${C_B}$TARGET${C_0}"

# ---------------------------------------------------------------- version ---
if [ "$VERSION" = "latest" ]; then
    step "Resolving the latest release..."
    # The API first; the /releases/latest redirect when rate-limited.
    TAG="$(fetch_stdout "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null \
        | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1 || true)"
    if [ -z "$TAG" ] && [ "$DL" = "curl" ]; then
        TAG="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null \
            | sed -n 's#.*/tag/##p' || true)"
    fi
    [ -n "$TAG" ] || die "could not resolve the latest release. Pass one explicitly: --version v1.2.3"
else
    case "$VERSION" in v*) TAG="$VERSION" ;; *) TAG="v$VERSION" ;; esac
fi

say "    version ${C_B}$TAG${C_0}"

NAME="hoocode-$TARGET"
ASSET="$NAME.tar.gz"
if [ -n "${HOOCODE_RELEASE_BASE_URL:-}" ]; then
    BASE="${HOOCODE_RELEASE_BASE_URL%/}/$TAG"
else
    BASE="https://github.com/$REPO/releases/download/$TAG"
fi

# ------------------------------------------------------------------ paths ---
BIN_DIR="$INSTALL_DIR/bin"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hoocode-install.XXXXXX")"
cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT INT TERM

# ------------------------------------------------------------- download -----
step "Downloading $ASSET..."
fetch "$BASE/$ASSET" "$TMP_DIR/$ASSET" || die "no build for $TARGET in $TAG.
    Checked: $BASE/$ASSET"

if [ -z "$SHA_CMD" ]; then
    warn "no sha256 tool found; skipping checksum verification."
elif fetch "$BASE/SHA256SUMS" "$TMP_DIR/SHA256SUMS" 2>/dev/null; then
    EXPECTED="$(grep " \*\{0,1\}$ASSET\$" "$TMP_DIR/SHA256SUMS" | awk '{print $1}' | head -n 1 || true)"
    [ -n "$EXPECTED" ] || die "$ASSET is not listed in SHA256SUMS for $TAG. Refusing to install."
    ACTUAL="$($SHA_CMD "$TMP_DIR/$ASSET" | awk '{print $1}')"
    [ "$EXPECTED" = "$ACTUAL" ] || die "checksum mismatch for $ASSET.
    expected $EXPECTED
    actual   $ACTUAL
    Refusing to install. Re-run, and if it persists open an issue:
      https://github.com/$REPO/issues"
    say "    ${C_OK}checksum verified${C_0}"
else
    # Releases before v0.0.3 have no SHA256SUMS.
    warn "$TAG has no SHA256SUMS; skipping checksum verification."
fi

# -------------------------------------------------------------- install -----
tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
NEW="$TMP_DIR/$NAME/hoocode"
[ -f "$NEW" ] || die "the archive did not contain $NAME/hoocode. Please report this: https://github.com/$REPO/issues"
chmod 755 "$NEW"

# Clear quarantine before the first run; macOS would otherwise block it.
if [ "$OS" = "darwin" ] && have xattr; then
    xattr -d com.apple.quarantine "$NEW" 2>/dev/null || true
fi

# Run it before replacing anything, so a binary that cannot start on this
# machine never replaces one that can.
INSTALLED_VERSION="$("$NEW" --version 2>&1)" || die "the downloaded binary does not run on this machine ($TARGET)."

step "Installing into $BIN_DIR..."
mkdir -p "$BIN_DIR"

# Earlier TypeScript installs linked hoocode/hoo to ~/.hoocode/lib/hoocode. Those
# names are this build's now; the TS build is `hoocode-ts`.
for _name in hoocode hoo; do
    if [ -L "$BIN_DIR/$_name" ] && [ "$(readlink "$BIN_DIR/$_name")" = "$INSTALL_DIR/lib/hoocode/hoocode" ]; then
        rm -f "$BIN_DIR/$_name"
        say "    ${C_DIM}replaced old TypeScript link $BIN_DIR/$_name${C_0}"
    fi
done
# ...and that TS payload, matched by its package name so nothing else is touched.
if grep -q '"@kolisachint/hoocode-agent"' "$INSTALL_DIR/lib/hoocode/package.json" 2>/dev/null; then
    rm -rf "$INSTALL_DIR/lib/hoocode"
fi

# Copy then rename: atomic, and safe while an older hoocode is running.
cp "$NEW" "$BIN_DIR/.hoocode.new"
mv -f "$BIN_DIR/.hoocode.new" "$BIN_DIR/hoocode"
ln -sf hoocode "$BIN_DIR/hoo"

say "    ${C_OK}installed${C_0} hoocode $INSTALLED_VERSION"

# ------------------------------------------------------------------ PATH ----
add_to_path() {
    _line="export PATH=\"\$HOME/.hoocode/bin:\$PATH\""
    [ "$INSTALL_DIR" = "$HOME/.hoocode" ] || _line="export PATH=\"$BIN_DIR:\$PATH\""

    _touched=""
    # zsh reads its rc from $ZDOTDIR when that is set.
    for rc in "$HOME/.bashrc" "${ZDOTDIR:-$HOME}/.zshrc" "$HOME/.profile"; do
        [ -f "$rc" ] || continue
        grep -qF "$BIN_DIR" "$rc" 2>/dev/null && continue
        grep -qF '.hoocode/bin' "$rc" 2>/dev/null && continue
        printf '\n# Added by the HooCode installer\n%s\n' "$_line" >> "$rc"
        _touched="$_touched $rc"
    done

    _fish="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"
    if [ -f "$_fish" ] && ! grep -qF "$BIN_DIR" "$_fish" 2>/dev/null; then
        printf '\n# Added by the HooCode installer\nfish_add_path %s\n' "$BIN_DIR" >> "$_fish"
        _touched="$_touched $_fish"
    fi

    printf '%s' "$_touched"
}

case ":$PATH:" in
    *":$BIN_DIR:"*) ON_PATH=1 ;;
    *) ON_PATH=0 ;;
esac

if [ "$ON_PATH" = "1" ]; then
    :
elif [ "$NO_MODIFY_PATH" = "1" ]; then
    warn "$BIN_DIR is not on your PATH. Add it yourself:
    export PATH=\"$BIN_DIR:\$PATH\""
else
    TOUCHED="$(add_to_path)"
    if [ -n "$TOUCHED" ]; then
        step "Added $BIN_DIR to PATH in:$TOUCHED"
        say "    ${C_DIM}open a new terminal, or: export PATH=\"$BIN_DIR:\$PATH\"${C_0}"
    else
        warn "could not find a shell rc file to update. Add this yourself:
    export PATH=\"$BIN_DIR:\$PATH\""
    fi
fi

# Another hoocode earlier on PATH (an npm/bun install of the TS build, say)
# would shadow this one.
_found="$(command -v hoocode 2>/dev/null || true)"
if [ "$ON_PATH" = "1" ] && [ -n "$_found" ] && [ "$_found" != "$BIN_DIR/hoocode" ]; then
    warn "another hoocode comes first on PATH: $_found
    Remove it, or put $BIN_DIR earlier on PATH."
fi

# ------------------------------------------------------------------ done ----
say ""
say "${C_OK}${C_B}HooCode is installed.${C_0}"
say ""
say "  ${C_B}hoocode${C_0}          start in build mode ${C_DIM}(or ${C_B}hoo${C_0}${C_DIM}, same thing)${C_0}"
say "  ${C_B}hoocode --help${C_0}   every flag"
say ""
say "  Docs      ${C_ACC}$WEBSITE${C_0}"
say "  Source    ${C_ACC}https://github.com/$REPO${C_0}"
say ""
