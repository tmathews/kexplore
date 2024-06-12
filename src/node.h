#pragma once

#include <stdbool.h>

struct node {
	int x, y, w, h;
	char **filepath;
	struct dirent *items;
	struct node *parent;
	struct node *children;
	bool open;
};

struct node *node_open(const char *);
void node_close(struct node *);
void node_calc_box(struct node *);
