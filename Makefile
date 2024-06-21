FLAGS = $(shell pkg-config --cflags --libs librsvg-2.0 pangocairo)
FILES = $(shell find src -type f -iname *.c -o -iname *.h)

build: protocol
	echo $(FILES)
	cc -g -o kexplore $(FILES) \
		-lwayland-client -lwayland-cursor -lrt -lxkbcommon -lcairo -lm $(FLAGS)

protocol:
	wayland-scanner private-code \
		< /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml \
		> src/klib/xdg-shell-protocol.c
	wayland-scanner client-header \
		< /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml \
		> src/klib/xdg-shell-client-protocol.h
	wayland-scanner private-code \
		< /usr/share/wayland-protocols/unstable/xdg-decoration/xdg-decoration-unstable-v1.xml \
		> src/klib/xdg-decoration-protocol.c
	wayland-scanner client-header \
		< /usr/share/wayland-protocols/unstable/xdg-decoration/xdg-decoration-unstable-v1.xml \
		> src/klib/xdg-decoration-client-protocol.h

clean:
	rm -f src/klib/*-protocol.h src/klib/*-protocol.c kexplore

install:
	#mkdir -p /usr/local/share/kallos/
	#cp -r data /usr/local/share/kallos/data
	cp kexplore /usr/local/bin/

.PHONY: build protocol clean install
