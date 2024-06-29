#include <dirent.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

#include "klib/draw.h"
#include "klib/geometry.h"
#include "node.h"
#include "stb_ds.h"
#include "utils.h"

int node_item_sort(const void *a, const void *b)
{
	struct node_item *aa = (struct node_item *)a;
	struct node_item *bb = (struct node_item *)b;
	return strcmp(aa->info.d_name, bb->info.d_name);
}

bool node_is_item(struct node *n, struct node_item *item)
{
	if (n == NULL || item == NULL)
		return false;
	char *name = strrchr(n->filepath, '/');
	name += 1;
	return strcmp(name, item->info.d_name) == 0;
}

struct node *node_open(const char *filepath)
{
	DIR *dp;
	struct dirent *ep;
	dp = opendir(filepath);
	if (dp == NULL) {
		return NULL;
	}
	struct node *n = calloc(1, sizeof(struct node));
	n->filepath    = malloc(strlen(filepath) + 1);
	n->rect        = rectangle_zero();
	strcpy(n->filepath, filepath);
	while ((ep = readdir(dp)) != NULL) {
		if (strcmp(ep->d_name, ".") == 0 || strcmp(ep->d_name, "..") == 0) {
			continue;
		}
		struct node_item item = {
			.info = *ep,
			.rect = rectangle_zero(),
			.next = NULL,
		};
		arrput(n->items, item);
	}
	closedir(dp);
	qsort(n->items, arrlen(n->items), sizeof(struct node_item), node_item_sort);
	return n;
}

void node_close(struct node *n)
{
	int len;
	// TODO this is hacky find a better way
	if (n->parent != NULL) {
		len = arrlen(n->parent->items);
		for (int i = 0; i < len; i++) {
			if (n->parent->items[i].next == n) {
				n->parent->items[i].next = NULL;
			}
		}
	}
	len = arrlen(n->items);
	for (int i = 0; i < len; i++) {
		if (n->items[i].next != NULL) {
			node_close(n->items[i].next);
		}
	}
	arrfree(n->items);
	free(n->filepath);
	free(n);
}

bool node_open_child(struct node *n, int index)
{
	char *npath        = string_path_join(n->filepath, n->items[index].info.d_name);
	struct node *child = node_open(npath);
	free(npath);
	if (child == NULL) {
		return false;
	}
	child->parent        = n;
	n->items[index].next = child;
	return false;
}

void node_calc_size(struct node *n, cairo_t *cr, PangoFontDescription *desc)
{
	const int padding     = 5;
	int oy                = padding;
	int mx                = 0;
	struct rectangle rect = rectangle_from_abxy(0, 0, 0, 0);
	for (int i = 0; i < arrlen(n->items); i++) {
		struct point size = text_size(cr, desc, n->items[i].info.d_name);
		n->items[i].rect  = rectangle_from_abxy(
            padding, oy,
            padding + size.x, oy + size.y);
		oy += size.y;
		if (size.x > mx) {
			mx = size.x;
		}
	}
	rect.max.x = mx + padding + padding;
	rect.max.y = oy + padding;
	// Set initial position based on parent
	if (n->parent != NULL) {
		struct node *p = n->parent;
		struct point pt;
		pt.x = p->rect.max.x + 20;
		pt.y = p->rect.min.y;
		for (int i = 0; i < arrlen(p->items); i++) {
			if (p->items[i].next != n) {
				continue;
			}
			pt.y += p->items[i].rect.min.y;
		}
		rect = rectangle_add_point(rect, pt);
	}
	n->rect = rect;
}
