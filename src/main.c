// TODO
//  * Tabs
// 	* Clicking item opens it or goes into directory
// 	* Preview pane (or window?) to display visual file preview
// 	* Navigation Area:
// 		- Home button (go to user directory)
// 		- Up button (go up 1 level)
// 		- Refresh button (if for some reason it doesn't trigger?)
// 		- URL bar (with button to favorite current directory)
// 		- Toggle Sidebar
// 		- Terminal button
// 	* Sidebar:
// 		- Sections can be collapsed
// 		- Sections have little number indicators
// 		- Sections:
// 			- Bookmarked Directories
// 			- Collections
// 			- Tags
// 	* Status bar (number of items in view, directory size in storage?)
// 	* File List: (should be similar to ls)
// 		- Icon, filename, created date, permissions, owner/group
// 		- Scrollable (horzontally, vertically)

#include <wayland-util.h>
#define STB_DS_IMPLEMENTATION

#include <cairo/cairo.h>
#include <dirent.h>
#include <pwd.h>
#include <stdio.h>
#include <sys/types.h>
#include <unistd.h>

#include "app.h"
#include "klib/draw.h"
#include "klib/waywrap.h"
#include "stb_ds.h"

struct app app;
void draw(cairo_t *, struct surface_state *);
void draw_entries(cairo_t *, int ox, int oy);
void open_dir(const char *);

struct core {
	char *cur_directory;
	struct dirent *items;
	wl_fixed_t x, y, lx, ly;
} core;

int main(int argc, char *argv[]) {
	core.items = NULL;
	core.x = 0;
	core.y = 0;
	struct passwd *info = getpwuid(getuid());
	open_dir(info->pw_dir);
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
	draw_entries(cr, wl_fixed_to_int(core.x), wl_fixed_to_int(core.y));
}

void draw_entries(cairo_t *cr, int ox, int oy) {
	cairo_save(cr);
	cairo_set_source_rgb(cr, 1, 1, 1);
	cairo_select_font_face(
		cr, "Noto Sans", CAIRO_FONT_SLANT_NORMAL, CAIRO_FONT_WEIGHT_NORMAL
	);
	cairo_set_font_size(cr, 18);
	oy += 16; // For the fact that it draws Y at the base line
			  // TODO get proper offset via bounding box
	int len = arrlen(core.items);
	for (int i = 0; i < len; i++) {
		struct dirent ent = core.items[i];
		draw_text(cr, ent.d_name, ox + 0, oy + (24 * i));
	}
	cairo_restore(cr);
}

void open_dir(const char *path) {
	DIR *dp;
	struct dirent *ep;
	dp = opendir(path);
	if (dp == NULL) {
		printf("failed to open dir!");
		return;
	}
	while ((ep = readdir(dp)) != NULL) {
		// printf("item: %s\n", ep->d_name);
		if (strcmp(ep->d_name, ".") == 0 || strcmp(ep->d_name, "..") == 0) {
			continue;
		}
		arrput(core.items, *ep);
	}
	closedir(dp);
}
