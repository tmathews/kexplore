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
#include "stb_ds.h"

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
#include "klib/geometry.h"
#include "main.h"
#include "node.h"
#include "utils.h"

struct app app;
struct core core;
double icon_rotation = 0;

RsvgHandle *load_svg(const char *filename)
{
	RsvgHandle *h = rsvg_handle_new_from_file(filename, NULL);
	if (h == NULL) {
		return NULL;
	}
	rsvg_handle_set_dpi(h, 72.0);
	return h;
}

int main(int argc, char *argv[])
{
	{
		PangoFontDescription *fd = pango_font_description_new();
		pango_font_description_set_family(fd, "Noto Sans");
		pango_font_description_set_weight(fd, PANGO_WEIGHT_NORMAL);
		pango_font_description_set_absolute_size(fd, 18 * PANGO_SCALE);
		core.fonts.normal = fd;
	}
	{
		core.icons.home      = load_svg("/usr/local/share/kallos/data/home.svg");
		core.icons.close     = load_svg("/usr/local/share/kallos/data/close.svg");
		core.icons.selection = load_svg("/usr/local/share/kallos/data/selection.svg");
		core.icons.top       = load_svg("/usr/local/share/kallos/data/top.svg");
		core.icons.parent    = load_svg("/usr/local/share/kallos/data/parent.svg");
		core.icons.copy      = load_svg("/usr/local/share/kallos/data/copy.svg");
		core.icons.open      = load_svg("/usr/local/share/kallos/data/open.svg");
		core.icons.terminal  = load_svg("/usr/local/share/kallos/data/terminal.svg");
		core.icons.busy      = load_svg("/usr/local/share/kallos/data/busy.svg");
	}
	core.camera.min     = point_zero();
	core.pan_speed      = 1.5;
	core.boxes          = NULL;
	core.selected_file  = NULL;
	core.drag_target    = NULL;
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
	}
	app_init();
	app.draw = &draw;
	while (app_running()) {
		app_process();
		process();
		icon_rotation += 0.2;
	}
	app_free();
	pango_font_description_free(core.fonts.normal);
	// TODO free icons
	return 0;
}

void process()
{
	if (!point_equal(&core.camera_target, &core.camera.min) && core.camera_refocus) {
		core.camera.min = point_lerp(&core.camera.min, &core.camera_target, 0.16);
		if (point_equal(&core.camera_target, &core.camera.min)) {
			core.camera_refocus = false;
		}
	}
	struct point cursor = {
		.x = core.camera.min.x + wl_fixed_to_double(app.pointer.x),
		.y = core.camera.min.y + wl_fixed_to_double(app.pointer.y),
	};
	struct hitbox *hb  = NULL;
	struct ev_entry *e = NULL;
	for (int i = arrlen(core.boxes) - 1; i >= 0; i--) {
		struct hitbox box = core.boxes[i];
		if (rectangle_contains(&box.area, cursor.x, cursor.y)) {
			hb = &box;
			e  = box.userdata;
			break;
		}
	}
	if (app.pointer.is_pressed) {
		if (e != NULL) {
			core.drag_target = e->n;
		} else {
			core.drag_target = NULL;
		}
	}
	if (app.pointer.is_down) {
		struct point pt_delta = {
			.x = wl_fixed_to_double(app.pointer.x - core.lx),
			.y = wl_fixed_to_double(app.pointer.y - core.ly),
		};
		if ((int)pt_delta.x != 0 || (int)pt_delta.y != 0) // TODO check pecision
			core.is_dragging = true;
		if (core.drag_target == NULL) {
			core.camera.min = point_sub(core.camera.min, pt_delta);
		} else {
			struct node *n = core.drag_target;
			n->rect        = rectangle_add_point(n->rect, pt_delta);
		}
	}
	core.lx = app.pointer.x;
	core.ly = app.pointer.y;
	if (app.pointer.is_released) {
		// printf("touch at %d %d\n", x, y);
		if (!core.is_dragging && hb != NULL) {
			switch (hb->trigger) {
			case 7: {
				if (core.selected_file != NULL) {
					char *cmd = string_concat("foot -D ", core.selected_file);
					run_cmd(cmd);
					free(cmd);
				}
			} break;
			case 6: {
				if (core.selected_file != NULL) {
					char *cmd = string_concat("wl-copy ", core.selected_file);
					run_cmd(cmd);
					free(cmd);
				}
			} break;
			case 5: { // Go to top of current node
				if (core.selection.n != NULL) {
					focus_to_rect(&core.selection.n->rect, &core.camera);
				}
			} break;
			case 4: { // Focus current selection's parent
				if (core.selection.n != NULL) {
					struct node_pos npos = node_find_in_parent(core.selection.n);
					struct rectangle r   = npos.item->rect;
					r                    = rectangle_add_point(r, core.selection.n->rect.min);
					focus_to_rect(&r, &core.camera);
				}
			} break;
			case 3: { // Focus current selection
				if (core.selection.n != NULL) {
					struct rectangle r = core.selection.n->items[core.selection.i].rect;
					r                  = rectangle_add_point(r, core.selection.n->rect.min);
					focus_to_rect(&r, &core.camera);
				}
			} break;
			case 2: { // Focus root
				focus_to_rect(&core.root->rect, &core.camera);
			} break;
			case 1: { // Do other things lol
				switch (e->type) {
				case 0: { // Hit a node item
					// printf("hit selection\n");
					core.selection = *e;
					if (core.selected_file != NULL) {
						free(core.selected_file);
					}
					core.selected_file = string_path_join(
						e->n->filepath, e->n->items[e->i].info.d_name);
					preview_destroy();
					if (e->n->items[e->i].info.d_type == DT_DIR) {
						pthread_t tid;
						pthread_create(&tid, NULL, &threaded_open_child_node, e);
					} else {
						// TODO do proper threading this is kinda fire and forget
						// resolve by marking busy or kill current and run new
						pthread_t tid;
						pthread_create(&tid, NULL, &preview_create, NULL);
					}
				} break;
				case 1: { // Hit open button
					// printf("hit button\n");
					char *filepath = string_path_join(
						e->n->filepath, e->n->items[e->i].info.d_name);
					open_file(filepath, core.fhandlers);
					free(filepath);
				} break;
				case 3: {
					node_close(e->n);
				} break;
				default: // NOOP
					break;
				}
			} break;
			}
		}
		core.is_dragging = false;
		core.drag_target = NULL;
	}
}

void *threaded_open_child_node(void *userdata)
{
	struct ev_entry *e = userdata;
	node_open_child(e->n, e->i);
	return NULL;
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

void *preview_create()
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
	return NULL;
}
