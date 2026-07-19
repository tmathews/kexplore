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

kexplore="${KEXPLORE_BIN:-kexplore}"

if [ "$save" = "1" ]; then
    if [ -n "$path" ]; then
        # Split the recommended path into the name to prefill and the
        # directory to land in.
        set -- --save "$(basename -- "$path")" --start "$(dirname -- "$path")"
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
