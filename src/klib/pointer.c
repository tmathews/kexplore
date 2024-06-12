#include "waywrap.h"

void wl_pointer_enter(
	void *data, struct wl_pointer *wl_pointer, uint32_t serial,
	struct wl_surface *surface, wl_fixed_t surface_x, wl_fixed_t surface_y
) {
	struct client_state *client_state = data;
	client_state->pointer_event.event_mask |= POINTER_EVENT_ENTER;
	client_state->pointer_event.serial = serial;
	client_state->pointer_event.surface_x = surface_x;
	client_state->pointer_event.surface_y = surface_y;
	client_state->active_surface_pointer =
		surface_state_findby_wl_surface(client_state->root_surface, surface);
}

void wl_pointer_leave(
	void *data, struct wl_pointer *wl_pointer, uint32_t serial,
	struct wl_surface *surface
) {
	struct client_state *client_state = data;
	client_state->pointer_event.serial = serial;
	client_state->pointer_event.event_mask |= POINTER_EVENT_LEAVE;
	client_state->active_surface_pointer = NULL;
}

void wl_pointer_motion(
	void *data, struct wl_pointer *wl_pointer, uint32_t time,
	wl_fixed_t surface_x, wl_fixed_t surface_y
) {
	struct client_state *client_state = data;
	client_state->pointer_event.event_mask |= POINTER_EVENT_MOTION;
	client_state->pointer_event.time = time;
	client_state->pointer_event.surface_x = surface_x;
	client_state->pointer_event.surface_y = surface_y;
}

void wl_pointer_button(
	void *data, struct wl_pointer *wl_pointer, uint32_t serial, uint32_t time,
	uint32_t button, uint32_t state
) {
	struct client_state *client_state = data;
	client_state->pointer_event.event_mask |= POINTER_EVENT_BUTTON;
	client_state->pointer_event.time = time;
	client_state->pointer_event.serial = serial;
	client_state->pointer_event.button = button;
	client_state->pointer_event.state = state;
}

void wl_pointer_axis(
	void *data, struct wl_pointer *wl_pointer, uint32_t time, uint32_t axis,
	wl_fixed_t value
) {
	struct client_state *client_state = data;
	client_state->pointer_event.event_mask |= POINTER_EVENT_AXIS;
	client_state->pointer_event.time = time;
	client_state->pointer_event.axes[axis].valid = true;
	client_state->pointer_event.axes[axis].value = value;
}

void wl_pointer_axis_source(
	void *data, struct wl_pointer *wl_pointer, uint32_t axis_source
) {
	struct client_state *client_state = data;
	client_state->pointer_event.event_mask |= POINTER_EVENT_AXIS_SOURCE;
	client_state->pointer_event.axis_source = axis_source;
}

void wl_pointer_axis_stop(
	void *data, struct wl_pointer *wl_pointer, uint32_t time, uint32_t axis
) {
	struct client_state *client_state = data;
	client_state->pointer_event.time = time;
	client_state->pointer_event.event_mask |= POINTER_EVENT_AXIS_STOP;
	client_state->pointer_event.axes[axis].valid = true;
}

void wl_pointer_axis_discrete(
	void *data, struct wl_pointer *wl_pointer, uint32_t axis, int32_t discrete
) {
	struct client_state *client_state = data;
	client_state->pointer_event.event_mask |= POINTER_EVENT_AXIS_DISCRETE;
	client_state->pointer_event.axes[axis].valid = true;
	client_state->pointer_event.axes[axis].discrete = discrete;
}

void wl_pointer_frame(void *data, struct wl_pointer *wl_pointer) {
	struct client_state *client_state = data;
	struct pointer_event *event = &client_state->pointer_event;
	if (event->event_mask & POINTER_EVENT_ENTER) {
		wl_pointer_set_cursor(
			wl_pointer, event->serial, client_state->cursor_surface, 0, 0
		);
	}
	if (client_state->active_surface_pointer != NULL) {
		struct pointer_event *p = calloc(1, sizeof(struct pointer_event));
		memcpy(p, event, sizeof(*event));
		if (client_state->active_surface_pointer->pointer == NULL) {
			client_state->active_surface_pointer->pointer = p;
		} else {
			struct pointer_event *next =
				client_state->active_surface_pointer->pointer;
			while (next->next != NULL) {
				next = next->next;
			}
			next->next = p;
		}
	}
	memset(event, 0, sizeof(*event));
}

void pointer_event_free(struct pointer_event *ev) {
	if (ev == NULL)
		return;
	if (ev->next != NULL)
		pointer_event_free(ev->next);
	free(ev);
}
