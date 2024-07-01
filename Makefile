FLAGS = $(shell pkg-config --cflags --libs librsvg-2.0 pangocairo libjpeg librsvg-2.0 libwebp)
FILES = $(shell find src -type f -iname *.c -o -iname *.h)
VENDOR = $(shell find vendor -type f -iname *.o)

build:
	cc -g -o kexplore $(FILES) $(VENDOR) \
		-lwayland-client -lwayland-cursor -lrt -lxkbcommon -lcairo -lm $(FLAGS)\
		-Ivendor/cairo_jpeg/src

# TODO get better at make files or accept defeat and use CMake
vendor:
	cc -Wall -c vendor/cairo_jpeg/src/cairo_jpg.c -o vendor/cairo_jpeg/src/cairo_jpg.o `pkg-config cairo libjpeg --cflags --libs`

wayland:
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
	mkdir -p /usr/local/share/kallos/data
	cp -r data/* /usr/local/share/kallos/data/
	cp kexplore /usr/local/bin/

.PHONY: build wayland clean install vendor
