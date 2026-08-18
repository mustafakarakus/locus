#!/bin/sh
# Locus uninstall script (U-014).
#
# Removes the binaries installed by install.sh. NEVER touches your memory
# data (~/.locus/locus.db or $LOCUS_HOME) — that is yours and is left intact.
#
# Usage:
#   uninstall.sh [--prefix DIR] [--bin-dir DIR]
set -eu

prefix="${HOME}/.local"
bin_dir=""

usage() {
    cat <<'EOF'
Usage: uninstall.sh [--prefix DIR] [--bin-dir DIR]

  --prefix DIR    install prefix            (default: $HOME/.local)
  --bin-dir DIR   binary directory          (default: <prefix>/bin)
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix) prefix="$2"; shift 2 ;;
        --bin-dir) bin_dir="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *)
            echo "uninstall.sh: unknown option: $1" >&2
            usage
            exit 2
            ;;
    esac
done

[ -z "$bin_dir" ] && bin_dir="$prefix/bin"

binaries="locus locusd locus-mcp locus-viz"

removed=0
for b in $binaries; do
    if [ -f "$bin_dir/$b" ] || [ -L "$bin_dir/$b" ]; then
        rm -f "$bin_dir/$b"
        echo "  removed $bin_dir/$b"
        removed=1
    fi
done

if [ "$removed" -eq 0 ]; then
    echo "No Locus binaries found in $bin_dir — nothing to remove."
else
    echo
    echo "Locus binaries removed from $bin_dir."
fi
echo "Your memory data (~/.locus/locus.db or \$LOCUS_HOME) was NOT touched."
