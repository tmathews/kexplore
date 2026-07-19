#!/bin/sh
# Wrapper that lets kexplore serve as the file chooser for GTK/Qt apps, via
# xdg-desktop-portal-termfilechooser.
#
# Chain: app --(GTK_USE_PORTAL=1)--> xdg-desktop-portal --(portals.conf)-->
#        termfilechooser --(cmd=)--> this script --> kexplore --pick-*
#
# Inputs, as documented by xdg-desktop-portal-termfilechooser:
#   $1  "1" if multiple files may be chosen, "0" otherwise.
#   $2  "1" if a directory should be chosen, "0" otherwise.
#   $3  "0" to open, "1" to save.
#   $4  When saving, the caller's recommended *full path* (e.g.
#       ~/Downloads/page.html). The portal guarantees it does not already
#       exist -- it appends "_" until that holds.
#   $5  The output path to write results to.
#
# Output: the chosen paths, one per line, in $5. Writing nothing means the
# operation was cancelled -- which is exactly what kexplore does on Cancel/Esc,
# so no special handling is needed here.
#
# Set KEXPLORE_BIN to point at a specific binary (e.g. a debug build).

set -eu

multiple="$1"
directory="$2"
save="$3"
path="$4"
out="$5"

# Resolving the binary matters more than it looks: the portal launches this
# from a systemd user service whose PATH is typically just /usr/local/bin and
# /usr/bin, so a kexplore installed anywhere else is invisible here even though
# it works fine in a login shell. Fall back to a build sitting next to this
# script before giving up.
if [ -n "${KEXPLORE_BIN:-}" ]; then
    kexplore="$KEXPLORE_BIN"
elif command -v kexplore >/dev/null 2>&1; then
    kexplore=kexplore
else
    # Parameter expansion, not dirname: this branch is reached precisely when
    # PATH is unhelpful, so it must not depend on finding coreutils.
    case "$0" in
        */*) here="${0%/*}" ;;
        *) here="." ;;
    esac
    for candidate in \
        "$here/../target/release/kexplore" \
        "$here/../target/debug/kexplore"
    do
        if [ -x "$candidate" ]; then
            kexplore="$candidate"
            break
        fi
    done
    if [ -z "${kexplore:-}" ]; then
        echo "kexplore-wrapper: cannot find the kexplore binary." >&2
        echo "  PATH=$PATH" >&2
        echo "  Set KEXPLORE_BIN, or install kexplore into /usr/local/bin." >&2
        exit 127
    fi
fi

if [ "$save" = "1" ]; then
    if [ -n "$path" ]; then
        # Split the recommended path into the name to prefill and the directory
        # to land in. Done with parameter expansion so the two edge cases
        # basename/dirname handle -- a bare name, and a file directly under
        # "/" -- stay correct without shelling out.
        case "$path" in
            */*)
                name="${path##*/}"
                dir="${path%/*}"
                [ -n "$dir" ] || dir="/"
                ;;
            *)
                name="$path"
                dir="."
                ;;
        esac
        set -- --save "$name" --start "$dir"
    else
        set -- --save
    fi
elif [ "$directory" = "1" ]; then
    if [ "$multiple" = "1" ]; then
        set -- --pick-dirs
    else
        set -- --pick-dir
    fi
elif [ "$multiple" = "1" ]; then
    set -- --pick-files
else
    set -- --pick-file
fi

exec "$kexplore" "$@" --out "$out"
