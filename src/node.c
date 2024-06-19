#include <cairo/cairo.h>
#include <dirent.h>
#include <pwd.h>
#include <stdio.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

#include "geometry.h"
#include "node.h"
#include "stb_ds.h"

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
	char *npath = malloc(strlen(n->filepath) + strlen(name) + 2);
	npath[0] = 0;
	strcpy(npath, n->filepath);
	strcat(npath, "/");
	strcat(npath, name);
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
