#pragma once

#include <librsvg/rsvg.h>
#include <pango/pango-font.h>
#include <pango/pangocairo.h>
#include <wayland-util.h>

#include "app.h"
#include "klib/geometry.h"

struct hitbox {
	struct rectangle area;
	int trigger;
	void *userdata;
};

struct ev_entry {
	struct node *n;
	int i;
	int type;
};

struct fonts {
	PangoFontDescription *normal;
};

struct icons {
	RsvgHandle *close;
	RsvgHandle *home;
	RsvgHandle *parent;
	RsvgHandle *top;
	RsvgHandle *selection;
	RsvgHandle *copy;
	RsvgHandle *open;
	RsvgHandle *terminal;
	RsvgHandle *busy;
};

struct core {
	wl_fixed_t lx, ly;
	float pan_speed;
	struct rectangle camera;
	struct node *root;
	struct hitbox *boxes;
	char *selected_file;
	struct ev_entry selection;
	struct fonts fonts;
	struct icons icons;
	struct file_handler *fhandlers;
	cairo_surface_t *preview_surface;
	RsvgHandle *preview_svg;
	void *drag_target;
	bool is_dragging;
	struct point camera_target;
	bool camera_refocus;
};

void process();
void *preview_create();
void preview_destroy();
void *threaded_open_child_node(void *);
void draw(struct draw_context);
void draw_entries(cairo_t *, struct node *, struct rectangle);
void draw_entries(cairo_t *cr, struct node *n, struct rectangle camera);
void draw_preview(cairo_t *cr, const char *filename, struct point window_size);
void draw_selection(cairo_t *cr, struct point surf_size);
void draw_navigation(cairo_t *cr, struct rectangle camera);
void focus_to_rect(const struct rectangle *rect, const struct rectangle *camera);
