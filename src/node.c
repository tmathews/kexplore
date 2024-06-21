#include <cairo/cairo.h>
#include <dirent.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

#include "klib/geometry.h"
#include "node.h"
#include "stb_ds.h"
#include "utils.h"

int node_item_sort(const void *a, const void *b) {
	struct node_item *aa = (struct node_item *)a;
	struct node_item *bb = (struct node_item *)b;
	return strcmp(aa->info.d_name, bb->info.d_name);
}

bool node_is_item(struct node *n, struct node_item *item) {
	if (n == NULL || item == NULL)
		return false;
	char *name = strrchr(n->filepath, '/');
	name += 1;
	return strcmp(name, item->info.d_name) == 0;
}

struct node *node_open(const char *filepath) {
	DIR *dp;
	struct dirent *ep;
	dp = opendir(filepath);
	if (dp == NULL) {
		return NULL;
	}
	struct node *n = calloc(1, sizeof(struct node));
	n->filepath = malloc(strlen(filepath) + 1);
	strcpy(n->filepath, filepath);
	while ((ep = readdir(dp)) != NULL) {
		if (strcmp(ep->d_name, ".") == 0 || strcmp(ep->d_name, "..") == 0) {
			continue;
		}
		struct node_item item = {
			.info = *ep,
			.selected = false,
		};
		arrput(n->items, item);
	}
	closedir(dp);
	qsort(n->items, arrlen(n->items), sizeof(struct node_item), node_item_sort);
	return n;
}

void node_close(struct node *n) {
	if (n->next != NULL) {
		node_close(n->next);
	}
	arrfree(n->items);
	free(n->filepath);
	free(n);
}

bool node_open_child(struct node *n, const char *name) {
	if (n->next != NULL) {
		node_close(n->next);
	}
	char *npath = string_path_join(n->filepath, name);
	struct node *child = node_open(npath);
	free(npath);
	if (child == NULL) {
		return false;
	}
	child->parent = n;
	n->next = child;
	// TODO finish my implementation
	return false;
}

struct point node_calc_size(struct node *n) {
	// TODO this is just a dummy method, to replace
	struct point p = {
		.y = arrlen(n->items) * 24,
		.x = 300,
	};
	return p;
}
