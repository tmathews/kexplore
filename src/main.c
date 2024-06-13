// kexplore is a graph based file explorer with the ability to tag and manage
// collections of files. Freely explore your file system like it's a big map.
//
// TODO
// 	- open nodes by clicking on directory
// 	- double click to open file
// 	- preview image pane
// 	- quick go to home directory node
// 	- middle mouse button to pan
// 	- tag file by selecting it and then going to the tag pane and editing them
// 	- add file to collection by selecting it and adding it to collection
// 	- collection & tag view
// 	- bookmarks pane that allows you to jump to a directory quickly
// 	- add bookmark by selecting directory and clicking a star/bookmark icon
#define STB_DS_IMPLEMENTATION

#include <cairo/cairo.h>
#include <dirent.h>
#include <pwd.h>
#include <stdio.h>
#include <sys/types.h>
#include <unistd.h>
#include <wayland-util.h>

#include "app.h"
#include "klib/draw.h"
#include "klib/waywrap.h"
#include "node.h"
#include "stb_ds.h"

struct core {
	wl_fixed_t x, y, lx, ly;
	struct node *root;
} core;

struct app app;
void draw(cairo_t *, struct surface_state *);
void draw_entries(cairo_t *, struct node *, struct point);
void open_dir(const char *);

int main(int argc, char *argv[]) {
	core.x = 0;
	core.y = 0;
	struct passwd *info = getpwuid(getuid());
	core.root = node_open(info->pw_dir);
	if (core.root == NULL) {
		printf("failed to load node\n");
		exit(1);
	}
	node_open_child(core.root, "tmp");
	printf("loaded %d items\n", (int)arrlen(core.root->items));
	app_init();
	app.draw = &draw;
	while (app_running()) {
		app_process();
		if (app.pointer.is_pressed) {
			core.lx = app.pointer.x;
			core.ly = app.pointer.y;
		}
		if (app.pointer.is_down) {
			core.x += app.pointer.x - core.lx;
			core.y += app.pointer.y - core.ly;
			core.lx = app.pointer.x;
			core.ly = app.pointer.y;
		}
	}
	app_free();
	return 0;
}

void draw(cairo_t *cr, struct surface_state *state) {
	int w = state->width;
	int h = state->height;
	// Draw background
	cairo_rectangle(cr, 0, 0, w, h);
	cairo_set_source_rgba(cr, 0, 0, 0, 0.8);
	cairo_fill(cr);
	draw_entries(cr, core.root, (struct point){});
}

void draw_entries(cairo_t *cr, struct node *n, struct point offset) {
	struct point size = node_calc_size(n);
	int ox = offset.x + wl_fixed_to_int(core.x);
	int oy = offset.y + wl_fixed_to_int(core.y);

	cairo_save(cr);
	// draw bounding box
	cairo_rectangle(cr, ox, oy, size.x, size.y);
	cairo_set_source_rgb(cr, 1, 1, 1);
	cairo_set_line_width(cr, 1);
	cairo_stroke(cr);

	// draw contents
	oy += 16; // For the fact that it draws Y at the base line
			  // TODO get proper offset via bounding box
	int len = arrlen(n->items);
	cairo_select_font_face(
		cr, "Noto Sans", CAIRO_FONT_SLANT_NORMAL, CAIRO_FONT_WEIGHT_NORMAL
	);
	cairo_set_font_size(cr, 18);
	cairo_set_source_rgb(cr, 1, 1, 1);
	for (int i = 0; i < len; i++) {
		struct node_item item = core.root->items[i];
		draw_text(cr, item.info.d_name, ox + 0, oy);
		oy += 24;
	}
	cairo_restore(cr);

	offset.x += size.x;
	offset.y += size.y;
	for (int i = 0; i < arrlen(n->children); i++) {
		draw_entries(cr, &n->children[i], offset);
	}
}
