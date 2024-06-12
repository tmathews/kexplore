#pragma once

#include <cairo/cairo.h>

#include "klib/keyhold.h"
#include "klib/waywrap.h"

struct pointer {
	int x, y, dx, dy;
	bool is_pressed;
	bool is_released;
	bool is_down;
	uint32_t last_time;
};

struct app {
	struct client_state *state;
	struct surface_state *surf_state;
	struct keyhold *keyhold_root;
	struct pointer pointer;
	void (*draw)(cairo_t *, struct surface_state *);
};

void app_init();
void app_free();
void app_on_draw(struct surface_state *, unsigned char *);
void app_on_keyboard(uint32_t, xkb_keysym_t, const char *);
bool app_running();
void app_process();
