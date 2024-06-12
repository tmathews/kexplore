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

void pointer_event_free(struct pointer_event *ev) {
	if (ev == NULL)
		return;
	if (ev->next != NULL)
		pointer_event_free(ev->next);
	free(ev);
}

void wl_pointer_frame(void *data, struct wl_pointer *wl_pointer) {
	struct client_state *client_state = data;
	struct pointer_event *event = &client_state->pointer_event;
	if (event->event_mask & POINTER_EVENT_ENTER) {
		wl_pointer_set_cursor(
			wl_pointer, event->serial, client_state->cursor_surface, 0, 0
		);
	}
	// printf("wl_pointer_FRAME: %d [m: %d]\n", event->time, event->event_mask);
	if (client_state->active_surface_pointer != NULL &&
		client_state->active_surface_pointer->pointer != NULL &&
		client_state->active_surface_pointer->pointer->time == event->time) {
		struct pointer_event *ev =
			client_state->active_surface_pointer->pointer;
		while (ev->next != NULL) {
			ev = ev->next;
		}
		ev->next = calloc(1, sizeof(struct pointer_event));
		memcpy(ev->next, event, sizeof(*event));
		return;
	}
	struct surface_state *next = client_state->root_surface;
	while (next != NULL) {
		if (next->pointer != NULL) {
			pointer_event_free(next->pointer);
			// free(next->pointer);
			next->pointer = NULL;
		}
		next = next->next;
	}
	if (client_state->active_surface_pointer != NULL) {
		// printf("setting mouse event for surface\n");
		client_state->active_surface_pointer->pointer =
			calloc(1, sizeof(struct pointer_event));
		memcpy(
			client_state->active_surface_pointer->pointer, event, sizeof(*event)
		);
	}
	memset(event, 0, sizeof(*event));
	return;

	/*struct client_state *client_state = data;
	struct pointer_event *event = &client_state->pointer_event;
	fprintf(stderr, "pointer frame @ %d: ", event->time);

	if (event->event_mask & POINTER_EVENT_ENTER) {
		wl_pointer_set_cursor(wl_pointer, event->serial,
			client_state->cursor_surface, 0, 0);
	}

	if (event->event_mask & POINTER_EVENT_ENTER) {
		fprintf(stderr, "entered %f, %f ",
			wl_fixed_to_double(event->surface_x),
			wl_fixed_to_double(event->surface_y));
	}

	if (event->event_mask & POINTER_EVENT_LEAVE) {
		fprintf(stderr, "leave");
	}

	if (event->event_mask & POINTER_EVENT_MOTION) {
		fprintf(stderr, "motion %f, %f ",
			wl_fixed_to_double(event->surface_x),
			wl_fixed_to_double(event->surface_y));
	}

	if (event->event_mask & POINTER_EVENT_BUTTON) {
		char *state = event->state == WL_POINTER_BUTTON_STATE_RELEASED ?
			"released" : "pressed";
		fprintf(stderr, "button %d %s ", event->button, state);
	}

	uint32_t axis_events = POINTER_EVENT_AXIS
			| POINTER_EVENT_AXIS_SOURCE
			| POINTER_EVENT_AXIS_STOP
			| POINTER_EVENT_AXIS_DISCRETE;
	char *axis_name[2] = {
			[WL_POINTER_AXIS_VERTICAL_SCROLL] = "vertical",
			[WL_POINTER_AXIS_HORIZONTAL_SCROLL] = "horizontal",
	};
	char *axis_source[4] = {
			[WL_POINTER_AXIS_SOURCE_WHEEL] = "wheel",
			[WL_POINTER_AXIS_SOURCE_FINGER] = "finger",
			[WL_POINTER_AXIS_SOURCE_CONTINUOUS] = "continuous",
			[WL_POINTER_AXIS_SOURCE_WHEEL_TILT] = "wheel tilt",
	};
	if (event->event_mask & axis_events) {
			for (size_t i = 0; i < 2; ++i) {
					if (!event->axes[i].valid) {
							continue;
					}
					fprintf(stderr, "%s axis ", axis_name[i]);
					if (event->event_mask & POINTER_EVENT_AXIS) {
							fprintf(stderr, "value %f ", wl_fixed_to_double(
													event->axes[i].value));
					}
					if (event->event_mask & POINTER_EVENT_AXIS_DISCRETE) {
							fprintf(stderr, "discrete %d ",
											event->axes[i].discrete);
					}
					if (event->event_mask & POINTER_EVENT_AXIS_SOURCE) {
							fprintf(stderr, "via %s ",
											axis_source[event->axis_source]);
					}
					if (event->event_mask & POINTER_EVENT_AXIS_STOP) {
							fprintf(stderr, "(stopped) ");
					}
			}
	}

	fprintf(stderr, "\n");
	memset(event, 0, sizeof(*event));*/
}
