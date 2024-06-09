#define STB_DS_IMPLEMENTATION

#include <cairo/cairo.h>
#include <dirent.h>
#include <stdio.h>
#include <sys/types.h>

#include "app.h"
#include "klib/draw.h"
#include "klib/waywrap.h"
#include "stb_ds.h"

struct app app;
void draw(cairo_t *, struct surface_state *);
void draw_entries(cairo_t *, int ox, int oy);
void open_dir(const char *);

struct entry {
	char **name;
};

struct core {
	char *cur_directory;
	// struct entry *items;
	struct dirent *items;
} core;

int main(int argc, char *argv[]) {
	// core.cur_directory = "/home/thomas";
	core.items = NULL;
	open_dir("/home/thomas");
	app_init();
	app.draw = &draw;
	while (app_running()) {
		app_process();
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
	draw_entries(cr, 10, 10);
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
