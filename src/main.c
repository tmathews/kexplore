// kexplore is a graph based file explorer with the ability to tag and manage
// collections of files. Freely explore your file system like it's a big map.
//
// TODO
//  - load files in a separate thread
//  - upon open center camera on target node
//  - have icons for directories
//  - show a nice button for opening files (that are recognized with handler)
//  - node navigation buttons:
//  	* jump to parent
//  	* jump to home
//  	* bookmark current url
//  	* copy current url
//  - keyboard navigation:
//  	* q or esc to quit
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
#include <librsvg/rsvg.h>
#include <pango/pango-font.h>
#include <pango/pangocairo.h>
#include <pwd.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <wayland-util.h>
#include <webp/decode.h>

#include "app.h"
#include "cairo_jpg.h"
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
	float pan_speed;
	struct point camera;
	struct node *root;
	struct hitbox *boxes;
	char *selected_file;
	struct ev_entry selection;
	bool is_dragging;
	bool first_draw;
	struct fonts fonts;
	struct file_handler *fhandlers;
	cairo_surface_t *preview_surface;
	RsvgHandle *preview_svg;
} core;

struct app app;
void process();
void preview_create();
void preview_destroy();
void draw(cairo_t *, struct surface_state *);
void draw_entries(cairo_t *, struct node *, struct point, struct rectangle);

int main(int argc, char *argv[])
{
	{
		PangoFontDescription *fd = pango_font_description_new();
		pango_font_description_set_family(fd, "Noto Sans");
		pango_font_description_set_weight(fd, PANGO_WEIGHT_NORMAL);
		pango_font_description_set_absolute_size(fd, 18 * PANGO_SCALE);
		core.fonts.normal = fd;
	}
	core.x              = 0;
	core.y              = 0;
	core.camera.x       = 0;
	core.camera.y       = 0;
	core.pan_speed      = 1.5;
	core.boxes          = NULL;
	core.selected_file  = NULL;
	core.is_dragging    = false;
	struct passwd *info = getpwuid(getuid());
	core.root           = node_open(info->pw_dir);
	if (core.root == NULL) {
		printf("failed to load node\n");
		exit(1);
	}
	{
		char *fname    = string_path_join(info->pw_dir, ".config/kallos/handlers");
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
	app.draw        = &draw;
	core.first_draw = true;
	while (app_running()) {
		app_process();
		process();
	}
	app_free();
	pango_font_description_free(core.fonts.normal);
	return 0;
}

void process()
{
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
	core.camera.x = -(wl_fixed_to_int(core.x)); // * core.pan_speed);
	core.camera.y = -(wl_fixed_to_int(core.y)); // * core.pan_speed);
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
							e->n->filepath, e->n->items[e->i].info.d_name);
						open_file(filepath, core.fhandlers);
						free(filepath);
					} else {
						printf("hit selection\n");
						core.selection = *e;
						if (core.selected_file != NULL) {
							free(core.selected_file);
						}
						core.selected_file = string_path_join(
							e->n->filepath, e->n->items[e->i].info.d_name);
						preview_destroy();
						if (e->n->items[e->i].info.d_type == DT_DIR) {
							printf("opening...\n");
							node_open_child(
								e->n, e->n->items[e->i].info.d_name);
						} else {
							if (e->n->next != NULL) {
								node_close(e->n->next);
								e->n->next = NULL;
							}
							preview_create();
						}
					}
					break;
				}
			}
		}
	}
}

void draw_selection(cairo_t *cr, struct point surf_size)
{
	char *text = NULL;
	if (core.selected_file == NULL) {
		text = "No file selected";
	} else {
		text = core.selected_file;
	}
	const float padding  = 80;
	struct point size    = text_size(cr, core.fonts.normal, text);
	struct fpoint origin = {
		.x = padding,
		.y = 15,
	};
	struct fpoint extends = {
		.x = surf_size.x - padding,
		.y = origin.y + size.y + 10,
	};
	cairo_save(cr);
	cairo_set_source_rgba(cr, 1, 1, 1, 0.1);
	cairo_rectangle(cr, 0, 0, surf_size.x, extends.y + 15);
	cairo_fill(cr);
	cairo_restore(cr);
	cairo_save(cr);
	path_rounded_rect_ab(cr, origin, extends, 6);
	cairo_set_source_rgba(cr, 1, 1, 1, 0.8);
	cairo_fill_preserve(cr);
	cairo_set_line_width(cr, 1);
	cairo_stroke(cr);
	cairo_move_to(cr, origin.x + 15, origin.y + ((extends.y - origin.y) / 2) - (int)(size.y / 2));
	cairo_set_source_rgba(cr, 0, 0, 0, 1);
	draw_text2(cr, core.fonts.normal, text);
	cairo_restore(cr);
}

void draw_preview(cairo_t *cr, const char *filename, struct point window_size)
{
	int size = 400;
	double x = 10;
	double y, w, h = 0;
	double scale = 1;
	cairo_save(cr);
	if (core.preview_surface != NULL) {
		w     = cairo_image_surface_get_width(core.preview_surface);
		h     = cairo_image_surface_get_height(core.preview_surface);
		scale = (float)size / (float)w;
		y     = window_size.y - 10 - (h * scale);
		cairo_translate(cr, x, y);
		cairo_scale(cr, scale, scale);
		cairo_set_source_surface(cr, core.preview_surface, 0, 0);
		cairo_paint(cr);
		cairo_restore(cr);

	} else if (core.preview_svg != NULL) {
		y                  = window_size.y - 10 - size;
		w                  = size;
		h                  = size;
		int padding        = 40;
		RsvgRectangle rect = {
			.x      = x + padding,
			.y      = y + padding,
			.width  = size - padding * 2,
			.height = size - padding * 2,
		};
		cairo_set_source_rgba(cr, 1, 1, 1, 1);
		rsvg_handle_render_document(core.preview_svg, cr, &rect, NULL);
	}
	cairo_save(cr);
	cairo_rectangle(cr, x + .5, y + .5, w * scale, h * scale);
	cairo_set_line_width(cr, 1);
	cairo_set_source_rgba(cr, 1, 1, 1, 1);
	cairo_stroke(cr);
	cairo_restore(cr);
}

void draw(cairo_t *cr, struct surface_state *state)
{
	int w             = state->width;
	int h             = state->height;
	struct point size = {.x = w, .y = h};
	if (core.first_draw) {
		printf("First draw: %d %d\n", w, h);
		core.x          = wl_fixed_from_int(w * 0.5);
		core.y          = wl_fixed_from_int(h * 0.5);
		core.first_draw = false;
	}
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
	struct rectangle cam;
	cam.min = core.camera;
	cam.max = point_add(core.camera, (struct point){.x = w, .y = h});
	draw_entries(cr, core.root, (struct point){.x = 0, .y = 0}, cam);
	draw_selection(cr, size);
	draw_preview(cr, core.selected_file, size);
}

void draw_entries(cairo_t *cr, struct node *n, struct point offset, struct rectangle camera)
{
	// Calculate the area if it's the first time we are encountering it.
	if (rectangle_is_zero(&n->rect)) {
		node_calc_size(n, cr, core.fonts.normal);
		n->rect = rectangle_add_point(n->rect, offset);
	}
	// Draw our node if in camera
	if (rectangle_intersects(&camera, &n->rect)) {
		struct point render_offset;
		render_offset.x = -camera.min.x;
		render_offset.y = -camera.min.y;
		// Draw the box
		cairo_save(cr);
		// TODO if the path is outside the render camera, then squash it to just
		// outside to save drawing. Find out if this is a micro optimization.
		path_rounded_rect_ab(cr,
			point_to_fpoint(point_add(n->rect.min, render_offset)),
			point_to_fpoint(point_add(n->rect.max, render_offset)), 5);
		cairo_set_source_rgb(cr, 1, 1, 1);
		cairo_set_line_width(cr, 3);
		cairo_stroke(cr);
		cairo_restore(cr);
		// Draw each item
		cairo_save(cr);
		int len = arrlen(n->items);
		for (int i = 0; i < len; i++) {
			struct node_item item = n->items[i];
			// Add our parent offset to get the global rect
			struct rectangle rect = rectangle_add_point(item.rect, n->rect.min);
			if (!rectangle_intersects(&camera, &rect)) {
				continue;
			}
			bool selected       = core.selection.n == n && i == core.selection.i;
			bool is_highlighted = node_is_item(n->next, &item);
			if (is_highlighted) {
				offset.y = rect.min.y;
			}
			if (selected || is_highlighted) {
				cairo_set_source_rgb(cr, 1, 0, 0);
			} else {
				cairo_set_source_rgb(cr, 1, 1, 1);
			}
			cairo_move_to(cr, rect.min.x + render_offset.x, rect.min.y + render_offset.y);
			draw_text2(cr, core.fonts.normal, item.info.d_name);
			{
				// TODO make hitbox generation a method or something
				struct hitbox hitbox;
				hitbox.area        = rect;
				hitbox.trigger     = 1;
				struct ev_entry *e = calloc(1, sizeof(struct ev_entry));
				e->n               = n;
				e->i               = i;
				e->type            = 0;
				hitbox.userdata    = e;
				arrput(core.boxes, hitbox);
			}
			if (selected) { // open button
				struct hitbox hitbox;
				hitbox.area.min.x  = rect.max.x + 10;
				hitbox.area.min.y  = rect.min.y;
				hitbox.area.max.x  = hitbox.area.min.x + 20;
				hitbox.area.max.y  = hitbox.area.min.y + 20;
				hitbox.trigger     = 1;
				struct ev_entry *e = calloc(1, sizeof(struct ev_entry));
				e->n               = n;
				e->i               = i;
				e->type            = 1;
				hitbox.userdata    = e;
				arrput(core.boxes, hitbox);
				// draw button
				struct point btnSize = rectangle_size(&hitbox.area);
				path_rounded_rect(
					cr,
					hitbox.area.min.x + .5 + render_offset.x,
					hitbox.area.min.y + .5 + render_offset.y,
					btnSize.x,
					btnSize.y,
					5);
				cairo_set_source_rgb(cr, 1, 0, 1);
				cairo_set_line_width(cr, 3);
				cairo_stroke(cr);
			}
		}
		cairo_restore(cr);
	}
	if (n->next != NULL) {
		struct point size = rectangle_size(&n->rect);
		offset.x += size.x + 20;
		draw_entries(cr, n->next, offset, camera);
	}
}

void preview_destroy()
{
	if (core.preview_svg != NULL) {
		g_object_unref(core.preview_svg);
		core.preview_svg = NULL;
	}
	if (core.preview_surface != NULL) {
		cairo_surface_destroy(core.preview_surface);
		core.preview_surface = NULL;
	}
}

cairo_surface_t *cairo_image_surface_create_from_webp(const char *filename)
{
	void *data;
	int infile;
	struct stat stat;
	if ((infile = open(filename, 0 | O_RDONLY)) == -1) {
		return NULL;
	}
	if (fstat(infile, &stat) == -1) {
		return NULL;
	}
	if ((data = malloc(stat.st_size)) == NULL) {
		return NULL;
	}
	if (read(infile, data, stat.st_size) < stat.st_size) {
		return NULL;
	}
	close(infile); // TODO clean this better
	{
		int w, h;
		uint8_t *buf         = WebPDecodeBGRA(data, stat.st_size, &w, &h);
		cairo_surface_t *sfc = cairo_image_surface_create_for_data(buf, CAIRO_FORMAT_ARGB32, w, h, w * 4);
		if (cairo_surface_status(sfc) != CAIRO_STATUS_SUCCESS) {
			return NULL;
		}
		cairo_surface_mark_dirty(sfc);
		cairo_surface_set_mime_data(sfc, "image/webp", data, stat.st_size, WebPFree, data);
		return sfc;
	}
	return NULL;
}

void preview_create()
{
	const char *filename = core.selected_file;
	if (is_file_ext(filename, ".png")) {
		core.preview_surface = cairo_image_surface_create_from_png(filename);
	} else if (is_file_ext(filename, ".svg")) {
		RsvgHandle *h = rsvg_handle_new_from_file(filename, NULL);
		if (h != NULL) {
			rsvg_handle_set_dpi(h, 72.0);
			core.preview_svg = h;
		}
	} else if (is_file_ext(filename, ".jpg") || is_file_ext(filename, ".jpeg") || is_file_ext(filename, ".jfif")) {
		core.preview_surface = cairo_image_surface_create_from_jpeg(filename);
	} else if (is_file_ext(filename, ".webp")) {
		core.preview_surface = cairo_image_surface_create_from_webp(filename);
	}
}
