#pragma once

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>
#include <wayland-cursor.h>
#include <xkbcommon/xkbcommon.h>

#include "xdg-decoration-client-protocol.h"
#include "xdg-shell-client-protocol.h"

enum pointer_event_mask {
	POINTER_EVENT_ENTER = 1 << 0,
	POINTER_EVENT_LEAVE = 1 << 1,
	POINTER_EVENT_MOTION = 1 << 2,
	POINTER_EVENT_BUTTON = 1 << 3,
	POINTER_EVENT_AXIS = 1 << 4,
	POINTER_EVENT_AXIS_SOURCE = 1 << 5,
	POINTER_EVENT_AXIS_STOP = 1 << 6,
	POINTER_EVENT_AXIS_DISCRETE = 1 << 7,
};

struct axe {
	bool valid;
	wl_fixed_t value;
	int32_t discrete;
};

struct pointer_event {
	uint32_t id;
	uint32_t event_mask;
	wl_fixed_t surface_x, surface_y;
	uint32_t button, state;
	uint32_t time;
	uint32_t serial;
	uint32_t axis_source;
	struct axe axes[2];
	struct pointer_event *next;
};

struct client_state {
	struct wl_display *wl_display;
	struct wl_registry *wl_registry;
	struct wl_shm *wl_shm;
	struct wl_compositor *wl_compositor;
	struct wl_seat *wl_seat;
	struct xdg_wm_base *xdg_wm_base;
	struct zxdg_decoration_manager_v1 *deco_manager;

	struct wl_cursor *cursor;
	struct wl_surface *cursor_surface;
	struct xkb_state *xkb_state;
	struct xkb_context *xkb_context;
	struct xkb_keymap *xkb_keymap;
	struct wl_keyboard *wl_keyboard;
	struct wl_pointer *wl_pointer;
	// TODO add touch

	struct wl_registry_listener registry_listener;
	struct xdg_wm_base_listener wm_base_listener;
	struct wl_seat_listener seat_listener;
	struct wl_pointer_listener pointer_listener;
	struct wl_keyboard_listener keyboard_listener;

	struct surface_state *root_surface;
	struct surface_state *active_surface_pointer;
	struct surface_state *active_surface_keyboard;
	struct pointer_event pointer_event;
	int32_t key_repeat_rate;
	int32_t key_repeat_delay;

	void (*on_keyboard)(uint32_t, xkb_keysym_t, const char *);
};

struct surface_state {
	struct client_state *client;
	struct surface_state *next;

	struct wl_surface *wl_surface;
	struct wl_buffer *wl_buffer;
	struct xdg_surface *xdg_surface;
	struct xdg_toplevel *xdg_toplevel;
	struct zxdg_toplevel_decoration_v1 *decos;

	struct wl_callback_listener frame_listener;
	struct wl_buffer_listener buffer_listener;
	struct xdg_surface_listener surface_listener;
	struct xdg_toplevel_listener toplevel_listener;
	struct zxdg_toplevel_decoration_v1_listener deco_listener;

	void (*on_draw)(struct surface_state *, unsigned char *);

	// float offset;
	uint32_t last_frame;
	int width, height;
	struct pointer_event *pointer;

	// Configuration Properties
	unsigned int wl_shm_format;
};

// METHODS

struct client_state *client_state_new();
void client_state_destroy(struct client_state *);
struct surface_state *surface_state_new(
	struct client_state *, const char *title, unsigned int width,
	unsigned int height
);
void surface_state_destroy(struct surface_state *);
struct surface_state *
surface_state_findby_wl_surface(struct surface_state *, struct wl_surface *);

// Surfaces
void registry_global(
	void *, struct wl_registry *, uint32_t name, const char *interface,
	uint32_t version
);
void registry_global_remove(void *, struct wl_registry *, uint32_t name);
struct wl_buffer *draw_frame(struct surface_state *);
void wl_buffer_release(void *, struct wl_buffer *);
void wl_surface_frame_done(void *, struct wl_callback *, uint32_t time);
void xdg_surface_configure(void *, struct xdg_surface *, uint32_t serial);
void xdg_wm_base_ping(void *, struct xdg_wm_base *, uint32_t serial);
void xdg_toplevel_configure(
	void *, struct xdg_toplevel *, int32_t width, int32_t height,
	struct wl_array *states
);
void xdg_toplevel_close(void *, struct xdg_toplevel *);
void zxdg_toplevel_decoration_configure(
	void *, struct zxdg_toplevel_decoration_v1 *, uint32_t mode
);

// Seat
void wl_seat_capabilities(void *, struct wl_seat *, uint32_t capabilities);
void wl_seat_name(void *, struct wl_seat *, const char *name);

// Pointer
void wl_pointer_enter(
	void *, struct wl_pointer *, uint32_t serial, struct wl_surface *,
	wl_fixed_t x, wl_fixed_t y
);
void wl_pointer_leave(void *, struct wl_pointer *, uint32_t serial, struct wl_surface *);
void wl_pointer_motion(
	void *, struct wl_pointer *, uint32_t time, wl_fixed_t x, wl_fixed_t y
);
void wl_pointer_button(
	void *, struct wl_pointer *, uint32_t serial, uint32_t time,
	uint32_t button, uint32_t state
);
void wl_pointer_axis(
	void *, struct wl_pointer *, uint32_t time, uint32_t axis, wl_fixed_t value
);
void wl_pointer_axis_source(void *, struct wl_pointer *, uint32_t axis_source);
void wl_pointer_axis_stop(
	void *, struct wl_pointer *, uint32_t time, uint32_t axis
);
void wl_pointer_axis_discrete(
	void *, struct wl_pointer *, uint32_t axis, int32_t discrete
);
void wl_pointer_frame(void *, struct wl_pointer *);

void pointer_event_free(struct pointer_event *ev);

// Keyboard
void wl_keyboard_keymap(
	void *, struct wl_keyboard *, uint32_t format, int32_t fd, uint32_t size
);
void wl_keyboard_enter(
	void *, struct wl_keyboard *, uint32_t serial, struct wl_surface *,
	struct wl_array *keys
);
void wl_keyboard_key(
	void *, struct wl_keyboard *, uint32_t serial, uint32_t time, uint32_t key,
	uint32_t state
);
void wl_keyboard_leave(void *, struct wl_keyboard *, uint32_t serial, struct wl_surface *);
void wl_keyboard_modifiers(
	void *, struct wl_keyboard *, uint32_t serial, uint32_t mods_depressed,
	uint32_t mods_latched, uint32_t mods_locked, uint32_t group
);
void wl_keyboard_repeat_info(
	void *data, struct wl_keyboard *wl_keyboard, int32_t rate, int32_t delay
);
