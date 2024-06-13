#pragma once

#include <dirent.h>
#include <stdbool.h>

struct point {
	int x, y;
};

struct node_item {
	struct dirent info;
	bool selected;
};

struct node {
	char *filepath;
	struct node_item *items;
	struct node *parent;
	struct node *children;
	bool open;
};

struct node *node_open(const char *);
void node_close(struct node *);
struct point node_calc_size(struct node *);
bool node_open_child(struct node *, const char *);
