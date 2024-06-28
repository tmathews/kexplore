#pragma once

#include <cairo/cairo.h>
#include <dirent.h>
#include <pango/pango-font.h>
#include <stdbool.h>

#include "klib/geometry.h"

struct node_item {
	struct dirent info;
	bool selected;
	struct rectangle rect;
};

struct node {
	char *filepath;
	struct node_item *items;
	struct node *parent;
	struct node *next; // next is the next open node
	bool open;
	struct rectangle rect;
};

bool node_is_item(struct node *n, struct node_item *item);
struct node *node_open(const char *);
void node_close(struct node *);
void node_calc_size(struct node *n, cairo_t *cr, PangoFontDescription *desc);
bool node_open_child(struct node *, const char *);
