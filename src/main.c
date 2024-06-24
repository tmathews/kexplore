// kexplore is a graph based file explorer with the ability to tag and manage
// collections of files. Freely explore your file system like it's a big map.
//
// TODO
//  - upon open center camera on target node
//  - have icons for directories
//  - show a nice button for opening files (that are recognized)
//  - node navigation buttons:
//  	* jump to parent
//  	* jump to home
//  	* bookmark current url
//  	* copy current url
//  - keyboard navigation:
//  	* h/left, l/right for out of and into directory
//  	* j/k/up/down for current node index traversal
// 	- preview image pane with support for: svg, gif, jpeg, png, webp, etc.
// 	- middle mouse button to pan
// 	- customize pan speed
// 	- tag file by selecting it and then going to the tag pane and editing them
// 	- add file to collection by selecting it and adding it to collection
// 	- collection & tag view
// 	- bookmarks pane that allows you to jump to a directory quickly
// 	- zoom?
#define STB_DS_IMPLEMENTATION

#include <cairo/cairo.h>
#include <dirent.h>
#include <pango/pango-font.h>
#include <pango/pangocairo.h>
#include <pwd.h>
#include <stdio.h>
#include <sys/types.h>
#include <unistd.h>
#include <wayland-util.h>

#include "app.h"
#include "klib/draw.h"
#include "klib/geometry.h"
#include "klib/waywrap.h"
#include "node.h"
#include "stb_ds.h"
#include "utils.h"

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

struct core {
	wl_fixed_t x, y, lx, ly;
	struct point camera;
	struct node *root;
	struct hitbox *boxes;
	char *selected_file;
	struct ev_entry selection;
	bool is_dragging;
	struct fonts fonts;
	struct file_handler *fhandlers;
	cairo_surface_t *preview_surface;
} core;

struct app app;
void process();
void draw(cairo_t *, struct surface_state *);
void draw_entries(cairo_t *, struct node *, struct point);

int main(int argc, char *argv[]) {
	{
		PangoFontDescription *fd = pango_font_description_new();
		pango_font_description_set_family(fd, "Noto Sans");
		pango_font_description_set_weight(fd, PANGO_WEIGHT_NORMAL);
		pango_font_description_set_absolute_size(fd, 18 * PANGO_SCALE);
		core.fonts.normal = fd;
	}
	core.x = 0;
	core.y = 0;
	core.camera.x = 0;
	core.camera.y = 0;
	core.boxes = NULL;
	core.selected_file = NULL;
	core.is_dragging = false;
	struct passwd *info = getpwuid(getuid());
	core.root = node_open(info->pw_dir);
	if (core.root == NULL) {
		printf("failed to load node\n");
		exit(1);
	}
	{
		char *fname = string_path_join(info->pw_dir, ".config/kallos/handlers");
		core.fhandlers = read_handlers(fname);
		free(fname);
		// for (int i = 0; i < arrlen(core.fhandlers); i++) {
		//	printf("new handler: '%s'!\n", core.fhandlers[i].command);
		//	for (int n = 0; n < arrlen(core.fhandlers[i].exts); n++) {
		//		printf("\t'%s'\n", core.fhandlers[i].exts[n]);
		//	}
		// }
	}
	app_init();
	app.draw = &draw;
	while (app_running()) {
		app_process();
		process();
	}
	app_free();
	pango_font_description_free(core.fonts.normal);
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
		// printf("touch at %d %d\n", x, y);
		if (!core.is_dragging) {
			for (int i = 0; i < arrlen(core.boxes); i++) {
				struct hitbox box = core.boxes[i];
				// printf(
				//	"x: %d y: %d x2: %d y2: %d\n", box.area.min.x,
				//	box.area.min.y, box.area.max.x, box.area.max.y
				//);
				if (rectangle_contains(&box.area, x, y)) {
					struct ev_entry *e = box.userdata;
					// printf("hit: %d!\n", box.trigger);
					//  printf(
					//	"hit info: %s, index: %d, filename: %s\n",
					//	e->n->filepath, e->i, e->n->items[e->i].info.d_name
					//);
					if (e->type == 1) {
						// printf("hit button\n");
						char *filepath = string_path_join(
							e->n->filepath, e->n->items[e->i].info.d_name
						);
						open_file(filepath, core.fhandlers);
						free(filepath);
					} else {
						printf("hit selection\n");
						core.selection = *e;
						if (core.selected_file != NULL) {
							free(core.selected_file);
						}
						core.selected_file = string_path_join(
							e->n->filepath, e->n->items[e->i].info.d_name
						);
						if (core.preview_surface != NULL) {
							cairo_surface_destroy(core.preview_surface);
							core.preview_surface = NULL;
						}
						if (e->n->items[e->i].info.d_type == DT_DIR) {
							printf("opening...\n");
							node_open_child(
								e->n, e->n->items[e->i].info.d_name
							);
						} else {
							if (e->n->next != NULL) {
								node_close(e->n->next);
								e->n->next = NULL;
							}
							if (is_file_ext(core.selected_file, ".png")) {
								core.preview_surface =
									cairo_image_surface_create_from_png(
										core.selected_file
									);
							}
						}
					}
					break;
				}
			}
		}
	}
}

void draw_selection(cairo_t *cr, const char *filepath) {
	char *text = NULL;
	if (core.selected_file == NULL) {
		text = "No file selected";
	} else {
		text = core.selected_file;
	}
	cairo_move_to(cr, 5, 5);
	struct point size = text_size(cr, core.fonts.normal, text);
	path_rounded_rect(cr, 5.5, 5.5, size.x + 10, size.y + 5, 10);
	cairo_set_source_rgba(cr, 0, 0, 0, 1);
	cairo_set_line_width(cr, 2);
	cairo_fill_preserve(cr);
	cairo_set_source_rgba(cr, 1, 1, 1, 1);
	cairo_stroke(cr);
	cairo_set_source_rgb(cr, 1, 1, 1);
	cairo_move_to(cr, 10, 7);
	draw_text2(cr, core.fonts.normal, text);
}

void draw_preview(cairo_t *cr, const char *filename, struct point window_size) {
	if (core.preview_surface == NULL) {
		return;
	}
	int size = 400;
	int w = cairo_image_surface_get_width(core.preview_surface);
	int h = cairo_image_surface_get_height(core.preview_surface);
	float scale = (float)size / (float)w;
	cairo_save(cr);
	cairo_translate(cr, 10, window_size.y - 10 - (h * scale));
	cairo_scale(cr, scale, scale);
	cairo_set_source_surface(cr, core.preview_surface, 0, 0);
	cairo_paint(cr);
	cairo_restore(cr);
}

void draw(cairo_t *cr, struct surface_state *state) {
	int w = state->width;
	int h = state->height;
	struct point size = {.x = w, .y = h};
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
	draw_selection(cr, core.selected_file);
	draw_preview(cr, core.selected_file, size);
}

void draw_entries(cairo_t *cr, struct node *n, struct point offset) {
	// TODO if display area is not on screen skip drawing
	struct point origin, cursor;
	origin.x = offset.x - core.camera.x;
	origin.y = offset.y - core.camera.y;
	cursor.x = origin.x + 5;
	cursor.y = origin.y;
	struct point size = {
		.x = 0,
		.y = 0,
	};
	{
		cairo_save(cr);
		// draw contents
		int len = arrlen(n->items);
		cairo_set_source_rgb(cr, 1, 1, 1);
		for (int i = 0; i < len; i++) {
			struct node_item item = n->items[i];
			bool selected = core.selection.n == n && i == core.selection.i;
			bool is_highlighted = node_is_item(n->next, &item);
			cursor.y += 5;
			cairo_move_to(cr, cursor.x, cursor.y);
			if (is_highlighted) {
				offset.y = cursor.y + core.camera.y;
			}
			if (selected || is_highlighted) {
				cairo_set_source_rgb(cr, 1, 0, 0);
			} else {
				cairo_set_source_rgb(cr, 1, 1, 1);
			}
			struct point tsize =
				draw_text2(cr, core.fonts.normal, item.info.d_name);
			if (tsize.x > size.x) {
				size.x = tsize.x + 10;
			}
			{ // hit box
				struct hitbox hitbox;
				hitbox.area.min.x = cursor.x + core.camera.x;
				hitbox.area.min.y = cursor.y + core.camera.y;
				hitbox.area.max.x = cursor.x + tsize.x + core.camera.x;
				hitbox.area.max.y = cursor.y + tsize.y + core.camera.y;
				hitbox.trigger = 1;
				struct ev_entry *e = calloc(1, sizeof(struct ev_entry));
				e->n = n;
				e->i = i;
				e->type = 0;
				hitbox.userdata = e;
				arrput(core.boxes, hitbox);
			}
			if (selected) { // open button
				struct hitbox hitbox;
				hitbox.area.min.x = cursor.x + tsize.x + 10 + core.camera.x;
				hitbox.area.min.y = cursor.y + core.camera.y;
				hitbox.area.max.x = hitbox.area.min.x + 20;
				hitbox.area.max.y = cursor.y + tsize.y + core.camera.y;
				hitbox.trigger = 1;
				struct ev_entry *e = calloc(1, sizeof(struct ev_entry));
				e->n = n;
				e->i = i;
				e->type = 1;
				hitbox.userdata = e;
				arrput(core.boxes, hitbox);
				// draw button
				struct point btnSize = rectangle_size(&hitbox.area);
				path_rounded_rect(
					cr, hitbox.area.min.x + .5 - core.camera.x,
					hitbox.area.min.y + .5 - core.camera.y, btnSize.x,
					btnSize.y, 5
				);
				cairo_set_source_rgb(cr, 1, 0, 1);
				cairo_set_line_width(cr, 3);
				cairo_stroke(cr);
			}
			cursor.y += tsize.y;
			size.y += tsize.y + 5;
		}
		cairo_restore(cr);
	}
	// draw bounding box
	path_rounded_rect(cr, origin.x + .5, origin.y + .5, size.x, size.y, 5);
	cairo_set_source_rgb(cr, 1, 1, 1);
	cairo_set_line_width(cr, 3);
	cairo_stroke(cr);

	offset.x += size.x + 20;
	if (n->next != NULL) {
		draw_entries(cr, n->next, offset);
	}
	// for (int i = 0; i < arrlen(n->children); i++) {
	//	draw_entries(cr, &n->children[i], offset);
	// }
}
