#pragma once

#include <cairo/cairo.h>
#include <wayland-util.h>

#include "klib/keyhold.h"
#include "klib/waywrap.h"

struct pointer {
	wl_fixed_t x, y;
	bool is_pressed;
	bool is_released;
	bool is_down;
	uint32_t last_time;
};

struct draw_context {
	struct surface_state *surface_state;
	cairo_t *cr;
	cairo_surface_t *cr_surface;
};

struct app {
	struct client_state *state;
	struct surface_state *surf_state;
	struct keyhold *keyhold_root;
	struct pointer pointer;
	void (*draw)(struct draw_context);
};

void app_init();
void app_free();
void app_on_draw(struct surface_state *, unsigned char *);
void app_on_keyboard(uint32_t, xkb_keysym_t, const char *);
bool app_running();
void app_process();
