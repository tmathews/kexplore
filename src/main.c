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
#include "geometry.h"
#include "klib/draw.h"
#include "klib/waywrap.h"
#include "node.h"
#include "stb_ds.h"

struct hitbox {
	struct rectangle area;
	int trigger;
	void *userdata;
};

struct ev_entry {
	struct node *n;
	int i;
};

struct core {
	wl_fixed_t x, y, lx, ly;
	struct point camera;
	struct node *root;
	struct hitbox *boxes;
} core;

struct app app;
void process();
void draw(cairo_t *, struct surface_state *);
void draw_entries(cairo_t *, struct node *, struct point);

int main(int argc, char *argv[]) {
	core.x = 0;
	core.y = 0;
	core.camera.x = 0;
	core.camera.y = 0;
	core.boxes = NULL;
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
		process();
	}
	app_free();
	return 0;
}

void process() {
	if (app.pointer.is_pressed) {
		core.lx = app.pointer.x;
		core.ly = app.pointer.y;
	}
	if (app.pointer.is_down) {
		core.x += app.pointer.x - core.lx;
		core.y += app.pointer.y - core.ly;
		core.lx = app.pointer.x;
		core.ly = app.pointer.y;
		core.camera.x = -wl_fixed_to_int(core.x);
		core.camera.y = -wl_fixed_to_int(core.y);
	}
	if (app.pointer.is_released) {
		int x = core.camera.x + wl_fixed_to_int(app.pointer.x);
		int y = core.camera.y + wl_fixed_to_int(app.pointer.y);
		// printf(
		//	"core: %d %d | released: %d, %d\n", -wl_fixed_to_int(core.x),
		//	-wl_fixed_to_int(core.y), x, y
		// );
		for (int i = 0; i < arrlen(core.boxes); i++) {
			struct hitbox box = core.boxes[i];
			// printf(
			//	"%d %d %d %d\n", box.area.min.x, box.area.min.y, box.area.max.x,
			//	box.area.max.y
			//);
			if (rectangle_contains(&box.area, x, y)) {
				printf("hit: %d!\n", box.trigger);
				// TODO trigger a thing!
				break;
			}
		}
	}
}

void draw(cairo_t *cr, struct surface_state *state) {
	int w = state->width;
	int h = state->height;
	// Draw background
	cairo_rectangle(cr, 0, 0, w, h);
	cairo_set_source_rgba(cr, 0, 0, 0, 0.8);
	cairo_fill(cr);
	// Reset all our hit boxes
	for (int i = 0; i < arrlen(core.boxes); i++) {
		free(core.boxes[i].userdata);
	}
	arrfree(core.boxes);
	core.boxes = NULL;
	// Draw things
	draw_entries(cr, core.root, (struct point){});
}

void draw_entries(cairo_t *cr, struct node *n, struct point offset) {
	int dx = offset.x - core.camera.x;
	int dy = offset.y - core.camera.y;
	struct point size = node_calc_size(n);
	size.x += 10;
	size.y += 10;
	// TODO if display area is not on screen skip drawing
	{
		cairo_save(cr);
		// draw bounding box
		path_rounded_rect(cr, dx + .5, dy + .5, size.x, size.y, 5);
		cairo_set_source_rgb(cr, 1, 1, 1);
		cairo_set_line_width(cr, 3);
		cairo_stroke(cr);
		// draw contents
		int len = arrlen(n->items);
		cairo_select_font_face(
			cr, "Noto Sans", CAIRO_FONT_SLANT_NORMAL, CAIRO_FONT_WEIGHT_NORMAL
		);
		cairo_set_font_size(cr, 18);
		cairo_set_source_rgb(cr, 1, 1, 1);
		for (int i = 0; i < len; i++) {
			struct node_item item = core.root->items[i];
			draw_text(cr, item.info.d_name, dx + 5, dy + 16 + 5);
			struct hitbox hitbox;
			hitbox.area.min.x = offset.x + 5;
			hitbox.area.min.y = offset.y + 5;
			hitbox.area.max.x = offset.x + 300;
			hitbox.area.max.y = offset.y + (i * 24) + 5;
			hitbox.trigger = 1;
			struct ev_entry *e = calloc(1, sizeof(struct ev_entry));
			e->n = n;
			e->i = i;
			hitbox.userdata = e;
			arrput(core.boxes, hitbox);
			dy += 24;
		}
		cairo_restore(cr);
	}
	offset.x += size.x + 20;
	if (n->next != NULL) {
		draw_entries(cr, n->next, offset);
	}
	// for (int i = 0; i < arrlen(n->children); i++) {
	//	draw_entries(cr, &n->children[i], offset);
	// }
}
