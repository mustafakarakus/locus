#!/bin/sh
# Locus install script (U-014).
#
# Builds release binaries (or copies prebuilt ones) and installs `locus`,
# `locusd`, `locus-mcp`, and `locus-viz` under a prefix.
#
# Usage:
#   install.sh [--prefix DIR] [--bin-dir DIR] [--from DIR] [--skip-build] [--no-init]
#
#   --prefix DIR    install prefix            (default: $HOME/.local)
#   --bin-dir DIR   binary directory         (default: <prefix>/bin)
#   --from DIR      copy prebuilt binaries from DIR instead of building
#   --skip-build    same as --from (uses target/release)
#   --no-init       do not initialize the Git project in the current directory
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
invocation_dir=$(pwd)

prefix="${HOME}/.local"
bin_dir=""
from_dir=""
build=1
auto_init=1

usage() {
    cat <<'EOF'
Usage: install.sh [--prefix DIR] [--bin-dir DIR] [--from DIR] [--skip-build] [--no-init]

  --prefix DIR    install prefix            (default: $HOME/.local)
  --bin-dir DIR   binary directory          (default: <prefix>/bin)
  --from DIR      copy prebuilt binaries from DIR instead of building
  --skip-build    same as --from (uses target/release)
    --no-init       do not initialize the Git project in the current directory
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix) prefix="$2"; shift 2 ;;
        --bin-dir) bin_dir="$2"; shift 2 ;;
        --from) from_dir="$2"; build=0; shift 2 ;;
        --skip-build) build=0; shift ;;
        --no-init) auto_init=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *)
            echo "install.sh: unknown option: $1" >&2
            usage
            exit 2
            ;;
    esac
done

[ -z "$bin_dir" ] && bin_dir="$prefix/bin"

binaries="locus locusd locus-mcp locus-viz"

if [ "$build" -eq 1 ]; then
    echo "Building release binaries (this may take a while)…"
    (
        cd "$repo_root"
        cargo build --release --bin locus --bin locusd --bin locus-mcp --bin locus-viz
    )
    from_dir="$repo_root/target/release"
elif [ -z "$from_dir" ]; then
    from_dir="$repo_root/target/release"
fi

missing=""
for b in $binaries; do
    if [ ! -f "$from_dir/$b" ]; then
        missing="$missing $b"
    fi
done

initialized=""
if [ "$auto_init" -eq 1 ] && command -v git >/dev/null 2>&1 &&
   git -C "$invocation_dir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    "$bin_dir/locus" init --yes --path "$invocation_dir"
    initialized="$invocation_dir"
fi
if [ -n "$missing" ]; then
    echo "install.sh: missing prebuilt binaries in $from_dir:$missing" >&2
    echo "  Build them first (cargo build --release) or point --from at a built dir." >&2
    exit 1
fi

mkdir -p "$bin_dir"
for b in $binaries; do
    cp "$from_dir/$b" "$bin_dir/$b"
    chmod 755 "$bin_dir/$b"
    echo "  installed $bin_dir/$b"
done

cat <<EOF

Installed Locus into $bin_dir.
  - Add $bin_dir to your PATH if it is not there already.
  - Memory data lives in ~/.locus (override with LOCUS_HOME).
  - Run 'locus init' in a project to install agent rules and MCP config.
EOF

if [ -n "$initialized" ]; then
    echo "  - Automatic agent, MCP, and post-commit setup enabled in $initialized."
else
    echo "  - No Git project was detected in the invocation directory; project setup was skipped."
fi
