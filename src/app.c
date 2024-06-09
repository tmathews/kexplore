#include "app.h"

extern struct app app;

void app_init() {
	const int width = 600;
	const int height = 440;
	struct client_state *state = client_state_new();
	state->on_keyboard = app_on_keyboard;
	app.state = state;
	app.draw = NULL;
	struct surface_state *a;
	a = surface_state_new(state, "Kallos Explore", width, height);
	a->on_draw = app_on_draw;
	xdg_toplevel_set_app_id(a->xdg_toplevel, "kallos-explore");
	zxdg_toplevel_decoration_v1_set_mode(a->decos, 1);
	app.surf_state = a;
}

void app_free() {
	client_state_destroy(app.state);
}

void app_on_draw(struct surface_state *state, unsigned char *data) {
	cairo_surface_t *csurf = cairo_image_surface_create_for_data(
		data, CAIRO_FORMAT_ARGB32, state->width, state->height, state->width * 4
	);
	cairo_t *cr = cairo_create(csurf);
	if (app.draw != NULL) {
		app.draw(cr, state);
	}
	cairo_surface_flush(csurf);
	cairo_destroy(cr);
	cairo_surface_destroy(csurf);
}

void app_on_keyrepeat(xkb_keysym_t sym) {
	// printf("got key repeat! %d\n", sym);
	// if (sym == XKB_KEY_BackSpace) {
	//	app->search_str[strlen(app->search_str) - 1] = '\0';
	//}
}

void app_on_keyboard(uint32_t state, xkb_keysym_t sym, const char *utf8) {
	if (state == WL_KEYBOARD_KEY_STATE_PRESSED) {
		app.keyhold_root = keyhold_add(app.keyhold_root, sym);
	} else {
		app.keyhold_root = keyhold_remove(app.keyhold_root, sym);
	}
}

bool app_running() {
	return app.state->root_surface != NULL;
}

void app_process() {
	wl_display_dispatch(app.state->wl_display);
	keyhold_process(
		app.keyhold_root, app.state->key_repeat_delay,
		app.state->key_repeat_rate, app_on_keyrepeat
	);
}