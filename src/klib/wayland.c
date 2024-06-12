#include "waywrap.h"

/* Methods for memory used to draw */

static void randname(char *buf) {
	struct timespec ts;
	clock_gettime(CLOCK_REALTIME, &ts);
	long r = ts.tv_nsec;
	for (int i = 0; i < 6; ++i) {
		buf[i] = 'A' + (r & 15) + (r & 16) * 2;
		r >>= 5;
	}
}

static int create_shm_file(void) {
	int retries = 100;
	do {
		char name[] = "/wl_shm-XXXXXX";
		randname(name + sizeof(name) - 7);
		--retries;
		int fd = shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0600);
		if (fd >= 0) {
			shm_unlink(name);
			return fd;
		}
	} while (retries > 0 && errno == EEXIST);
	return -1;
}

static int allocate_shm_file(size_t size) {
	int fd = create_shm_file();
	if (fd < 0)
		return -1;
	int ret;
	do {
		ret = ftruncate(fd, size);
	} while (ret < 0 && errno == EINTR);
	if (ret < 0) {
		close(fd);
		return -1;
	}
	return fd;
}

/* Wayland Client Things */

static void init_cursor(struct client_state *state) {
	struct wl_cursor_theme *theme =
		wl_cursor_theme_load(NULL, 24, state->wl_shm);
	state->cursor = wl_cursor_theme_get_cursor(theme, "left_ptr");
	struct wl_cursor_image *img = state->cursor->images[0];
	struct wl_buffer *buf = wl_cursor_image_get_buffer(img);
	state->cursor_surface = wl_compositor_create_surface(state->wl_compositor);
	wl_surface_attach(state->cursor_surface, buf, 0, 0);
	wl_surface_commit(state->cursor_surface);
}

struct client_state *client_state_new() {
	struct client_state *state = calloc(1, sizeof(struct client_state));
	if (state == NULL)
		return NULL;
	state->registry_listener = (struct wl_registry_listener){
		.global = registry_global,
		.global_remove = registry_global_remove,
	};
	state->wm_base_listener = (struct xdg_wm_base_listener){
		.ping = xdg_wm_base_ping,
	};
	state->seat_listener = (struct wl_seat_listener){
		.capabilities = wl_seat_capabilities,
		.name = wl_seat_name,
	};
	state->pointer_listener = (struct wl_pointer_listener){
		.enter = wl_pointer_enter,
		.leave = wl_pointer_leave,
		.motion = wl_pointer_motion,
		.button = wl_pointer_button,
		.axis = wl_pointer_axis,
		.frame = wl_pointer_frame,
		.axis_source = wl_pointer_axis_source,
		.axis_stop = wl_pointer_axis_stop,
		.axis_discrete = wl_pointer_axis_discrete,
	};
	state->keyboard_listener = (struct wl_keyboard_listener){
		.keymap = wl_keyboard_keymap,
		.enter = wl_keyboard_enter,
		.leave = wl_keyboard_leave,
		.key = wl_keyboard_key,
		.modifiers = wl_keyboard_modifiers,
		.repeat_info = wl_keyboard_repeat_info,
	};
	state->wl_display = wl_display_connect(NULL);
	state->wl_registry = wl_display_get_registry(state->wl_display);
	state->xkb_context = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
	wl_registry_add_listener(
		state->wl_registry, &state->registry_listener, state
	);
	wl_display_roundtrip(state->wl_display);
	init_cursor(state);
	return state;
}

void client_state_destroy(struct client_state *state) {
	struct surface_state *next, *tmp;
	tmp = NULL;
	next = state->root_surface;
	while (next != NULL) {
		tmp = next->next;
		surface_state_destroy(next);
		next = tmp;
	}
	// TODO figure out if we should be calling wl_display_dispatch in distroying
	// the client state
	// wl_display_dispatch(state->wl_display);
	free(state);
}

struct surface_state *surface_state_new(
	struct client_state *client_state, const char *title, unsigned int width,
	unsigned int height
) {
	struct surface_state *state, *parent;
	state = calloc(1, sizeof(struct surface_state));
	if (state == NULL)
		return NULL;
	if (client_state->root_surface == NULL) {
		client_state->root_surface = state;
	} else {
		parent = client_state->root_surface;
		for (; parent->next != NULL; parent = parent->next)
			;
		parent->next = state;
	}
	state->client = client_state;
	state->next = NULL;
	state->pointer = NULL;
	state->width = width;
	state->height = height;
	state->frame_listener = (struct wl_callback_listener){
		.done = wl_surface_frame_done,
	};
	state->buffer_listener = (struct wl_buffer_listener){
		.release = wl_buffer_release,
	};
	state->surface_listener = (struct xdg_surface_listener){
		.configure = xdg_surface_configure,
	};
	state->toplevel_listener = (struct xdg_toplevel_listener){
		.configure = xdg_toplevel_configure,
		.close = xdg_toplevel_close,
	};
	state->deco_listener = (struct zxdg_toplevel_decoration_v1_listener){
		.configure = zxdg_toplevel_decoration_configure,
	};
	// Initial top level window surface
	state->wl_shm_format = WL_SHM_FORMAT_ARGB8888;
	state->wl_surface =
		wl_compositor_create_surface(client_state->wl_compositor);
	state->xdg_surface = xdg_wm_base_get_xdg_surface(
		client_state->xdg_wm_base, state->wl_surface
	);
	xdg_surface_add_listener(
		state->xdg_surface, &state->surface_listener, state
	);
	state->xdg_toplevel = xdg_surface_get_toplevel(state->xdg_surface);
	xdg_toplevel_add_listener(
		state->xdg_toplevel, &state->toplevel_listener, state
	);
	xdg_toplevel_set_title(state->xdg_toplevel, title);
	// Decorations
	state->decos = zxdg_decoration_manager_v1_get_toplevel_decoration(
		client_state->deco_manager, state->xdg_toplevel
	);
	zxdg_toplevel_decoration_v1_add_listener(
		state->decos, &state->deco_listener, NULL
	);
	wl_surface_commit(state->wl_surface);
	struct wl_callback *cb = wl_surface_frame(state->wl_surface);
	wl_callback_add_listener(cb, &state->frame_listener, state);
	return state;
}

void surface_state_destroy(struct surface_state *state) {
	struct surface_state *n = state->client->root_surface;
	if (n == state) {
		state->client->root_surface = state->next;
	} else {
		while (n != NULL) {
			if (n->next == state) {
				n->next = state->next;
				break;
			}
			n = n->next;
		}
	}
	zxdg_toplevel_decoration_v1_destroy(state->decos);
	xdg_toplevel_destroy(state->xdg_toplevel);
	// printf("buffer address: %p\n", state->wl_buffer);
	// wl_buffer_destroy(state->wl_buffer);
	xdg_surface_destroy(state->xdg_surface);
	wl_surface_destroy(state->wl_surface);
	free(state);
}

struct surface_state *surface_state_findby_wl_surface(
	struct surface_state *root, struct wl_surface *wl_surface
) {
	while (root != NULL) {
		if (root->wl_surface == wl_surface)
			return root;
		root = root->next;
	}
	return NULL;
}

void wl_buffer_release(void *data, struct wl_buffer *wl_buffer) {
	/* Sent by the compositor when it's no longer using this buffer */
	wl_buffer_destroy(wl_buffer);
}

struct wl_buffer *draw_frame(struct surface_state *state) {
	const int width = state->width, height = state->height;
	int stride = width * 4;
	int size = stride * height;

	int fd = allocate_shm_file(size);
	if (fd == -1) {
		return NULL;
	}

	// uint32_t *data = mmap(NULL, size,
	unsigned char *data =
		mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (data == MAP_FAILED) {
		close(fd);
		return NULL;
	}

	if (state->on_draw != NULL) {
		state->on_draw(state, data);
	} else {
		// TODO draw nothing
	}

	struct wl_shm_pool *pool =
		wl_shm_create_pool(state->client->wl_shm, fd, size);
	struct wl_buffer *buffer = wl_shm_pool_create_buffer(
		pool, 0, width, height, stride, state->wl_shm_format
	); // WL_SHM_FORMAT_XRGB8888);
	wl_shm_pool_destroy(pool);
	close(fd);

	/* Draw checkerboxed background */
	/*int offset = (int)state->offset % 8;
	for (int y = 0; y < height; ++y) {
		for (int x = 0; x < width; ++x) {
			// If its under the mouse make it red!
			if (x >= xx - 10 && x <= xx + 10  && y >= yy - 10 && y <= yy + 10) {
				data[y * width +x] = 0xFFFF0000;
				continue;
			}

			if (((x + offset) + (y + offset) / 8 * 8) % 16 < 8)
				data[y * width + x] = 0xFF666666;
			else
				data[y * width + x] = 0xFFEEEEEE;
		}
	}*/

	munmap(data, size);
	wl_buffer_add_listener(buffer, &state->buffer_listener, NULL);
	return buffer;
}

void xdg_toplevel_configure(
	void *data, struct xdg_toplevel *xdg_toplevel, int32_t width,
	int32_t height, struct wl_array *states
) {
	struct surface_state *state = data;
	if (width == 0 || height == 0) {
		return;
	}
	state->width = width;
	state->height = height;
}

void xdg_toplevel_close(void *data, struct xdg_toplevel *toplevel) {
	// printf("closed recieved\n");
	struct surface_state *state = data;
	surface_state_destroy(state);
}

void xdg_surface_configure(
	void *data, struct xdg_surface *xdg_surface, uint32_t serial
) {
	struct surface_state *state = data;
	xdg_surface_ack_configure(xdg_surface, serial);
	struct wl_buffer *buffer = draw_frame(state);
	wl_surface_attach(state->wl_surface, buffer, 0, 0);
	wl_surface_commit(state->wl_surface);
	state->wl_buffer = buffer;
}

void xdg_wm_base_ping(
	void *data, struct xdg_wm_base *xdg_wm_base, uint32_t serial
) {
	xdg_wm_base_pong(xdg_wm_base, serial);
}

void wl_surface_frame_done(void *data, struct wl_callback *cb, uint32_t time) {
	wl_callback_destroy(cb);

	struct surface_state *state = data;
	cb = wl_surface_frame(state->wl_surface);
	wl_callback_add_listener(cb, &state->frame_listener, state);

	// if (state->last_frame != 0) {
	//	int elapsed = time - state->last_frame;
	//	state->offset += elapsed / 1000.0 * 24;
	// }

	struct wl_buffer *buffer = draw_frame(state);
	wl_surface_attach(state->wl_surface, buffer, 0, 0);
	wl_surface_damage_buffer(state->wl_surface, 0, 0, INT32_MAX, INT32_MAX);
	wl_surface_commit(state->wl_surface);
	state->wl_buffer = buffer;
	state->last_frame = time;
}

void registry_global(
	void *data, struct wl_registry *wl_registry, uint32_t name,
	const char *interface, uint32_t version
) {
	// printf("interface: %s, version: %d, name: %d\n",
	//	interface, version, name);
	struct client_state *state = data;
	if (strcmp(interface, wl_shm_interface.name) == 0) {
		state->wl_shm =
			wl_registry_bind(wl_registry, name, &wl_shm_interface, 1);
	} else if (strcmp(interface, wl_compositor_interface.name) == 0) {
		state->wl_compositor =
			wl_registry_bind(wl_registry, name, &wl_compositor_interface, 4);
	} else if (strcmp(interface, xdg_wm_base_interface.name) == 0) {
		state->xdg_wm_base =
			wl_registry_bind(wl_registry, name, &xdg_wm_base_interface, 1);
		xdg_wm_base_add_listener(
			state->xdg_wm_base, &state->wm_base_listener, state
		);
	} else if (strcmp(interface, wl_seat_interface.name) == 0) {
		state->wl_seat =
			wl_registry_bind(wl_registry, name, &wl_seat_interface, 7);
		wl_seat_add_listener(state->wl_seat, &state->seat_listener, state);
	} else if (strcmp(interface, zxdg_decoration_manager_v1_interface.name) == 0) {
		state->deco_manager = wl_registry_bind(
			wl_registry, name, &zxdg_decoration_manager_v1_interface, 1
		);
	}
}

void registry_global_remove(
	void *data, struct wl_registry *wl_registry, uint32_t name
) {
	/* This space deliberately left blank */
}

void zxdg_toplevel_decoration_configure(
	void *data, struct zxdg_toplevel_decoration_v1 *deco, uint32_t mode
) {
	printf("server told me to configure mode: %d\n", mode);
}

void wl_seat_capabilities(
	void *data, struct wl_seat *wl_seat, uint32_t capabilities
) {
	struct client_state *state = data;
	bool have_pointer = capabilities & WL_SEAT_CAPABILITY_POINTER;
	bool have_keyboard = capabilities & WL_SEAT_CAPABILITY_KEYBOARD;

	if (have_pointer && state->wl_pointer == NULL) {
		state->wl_pointer = wl_seat_get_pointer(state->wl_seat);
		wl_pointer_add_listener(
			state->wl_pointer, &state->pointer_listener, state
		);
	} else if (!have_pointer && state->wl_pointer != NULL) {
		wl_pointer_release(state->wl_pointer);
		state->wl_pointer = NULL;
	}

	if (have_keyboard && state->wl_keyboard == NULL) {
		state->wl_keyboard = wl_seat_get_keyboard(state->wl_seat);
		wl_keyboard_add_listener(
			state->wl_keyboard, &state->keyboard_listener, state
		);
	} else if (!have_keyboard && state->wl_keyboard != NULL) {
		wl_keyboard_release(state->wl_keyboard);
		state->wl_keyboard = NULL;
	}
}

void wl_seat_name(void *data, struct wl_seat *wl_seat, const char *name) {
	fprintf(stderr, "seat name: %s\n", name);
}
