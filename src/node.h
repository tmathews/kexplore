#pragma once

#include <cairo/cairo.h>
#include <dirent.h>
#include <pango/pango-font.h>
#include <stdbool.h>

#include "klib/geometry.h"

struct node_pos {
	struct node_item *item;
	int index;
};

struct node_item {
	struct dirent info;
	struct rectangle rect;
	struct node *next;
};

struct node {
	char *filepath;
	struct node_item *items;
	struct node *parent;
	struct rectangle rect;
	bool busy;
};

bool node_is_item(struct node *n, struct node_item *item);
struct node *node_open(const char *);
bool node_open_child(struct node *, int);
void node_close(struct node *);
struct node_pos node_find_in_parent(const struct node *n);
void node_calc_size(struct node *n, cairo_t *cr, PangoFontDescription *desc);
