# kexplore as a system file chooser

Makes GTK/Qt apps open kexplore instead of their own file dialog, by plugging
into `xdg-desktop-portal` as the `org.freedesktop.impl.portal.FileChooser`
backend.

kexplore ships no D-Bus service of its own. It borrows
[xdg-desktop-portal-termfilechooser], which already implements the backend and
shells out to a command — so all we provide is the wrapper in this directory.

```
app --(GTK_USE_PORTAL=1)--> xdg-desktop-portal --(portals.conf)-->
    termfilechooser --(cmd=)--> kexplore-termfilechooser-wrapper.sh -->
        kexplore --pick-file|--pick-files|--pick-dir|--pick-dirs|--save
```

## Setup

**1. Install the portal backend** (AUR):

```sh
yay -S xdg-desktop-portal-termfilechooser-git
```

**2. Point it at the wrapper** —
`~/.config/xdg-desktop-portal-termfilechooser/config`:

```ini
[filechooser]
cmd=/path/to/kexplore/contrib/kexplore-termfilechooser-wrapper.sh
```

The wrapper finds the binary in this order:

1. `$KEXPLORE_BIN`, if set.
2. `kexplore` on `$PATH`.
3. `../target/release/kexplore`, then `../target/debug/kexplore`, relative to
   the wrapper itself.

Step 3 exists because the portal launches the wrapper from a **systemd user
service**, whose `PATH` is usually just `/usr/local/bin:/usr/bin` — so a
kexplore installed anywhere else (`~/.local/bin`, a `bin` dir on your shell
`PATH`) is invisible here even though it runs fine in a terminal. Check with:

```sh
systemctl --user show-environment | grep ^PATH=
```

If none of the three resolve, the wrapper exits 127 and prints the `PATH` it
searched, rather than failing silently inside the portal.

**3. Select the backend** — `~/.config/xdg-desktop-portal/portals.conf`:

```ini
[preferred]
default=gtk
org.freedesktop.impl.portal.FileChooser=termfilechooser
```

Keeping `default=gtk` matters: it only redirects the *file chooser*, leaving
screencast, notifications and the rest with their usual backends.

**4. Restart the portal:**

```sh
systemctl --user restart xdg-desktop-portal
```

GTK3 apps additionally need `GTK_USE_PORTAL=1` in their environment.

## Checking it works

The wrapper is a plain script, so it can be exercised without any of the above.
`KEXPLORE_BIN` accepts a stub that just prints its arguments:

```sh
printf '#!/bin/sh\nfor a in "$@"; do printf "[%%s]" "$a"; done; echo\n' > /tmp/stub.sh
chmod +x /tmp/stub.sh

# args are: multiple directory save path out
KEXPLORE_BIN=/tmp/stub.sh ./kexplore-termfilechooser-wrapper.sh 0 0 0 "" /tmp/o
# -> [--pick-file][--out][/tmp/o]
KEXPLORE_BIN=/tmp/stub.sh ./kexplore-termfilechooser-wrapper.sh 0 0 1 ~/Downloads/p.html /tmp/o
# -> [--save][p.html][--start][/home/you/Downloads][--out][/tmp/o]
```

Then for real, with the portal running:

```sh
KEXPLORE_BIN=$(which kexplore) ./kexplore-termfilechooser-wrapper.sh \
    1 0 0 "" /tmp/out.txt && cat /tmp/out.txt
```

## Reverting

Delete the `org.freedesktop.impl.portal.FileChooser` line from `portals.conf`
and restart the portal. Nothing else on the system is touched.

## Notes

- Cancelling writes nothing to the output file, which is exactly how the portal
  detects cancellation — kexplore's Cancel/Esc already behave that way.
- In save mode the portal hands over a full recommended path that it guarantees
  does not yet exist (it appends `_` until that holds). The wrapper splits it
  into `--save <name>` and `--start <dir>`.
- `src/cli.rs` has a test (`parses_what_the_termfilechooser_wrapper_emits`)
  pinning the exact argument vectors this wrapper produces.

[xdg-desktop-portal-termfilechooser]: https://github.com/GermainZ/xdg-desktop-portal-termfilechooser
