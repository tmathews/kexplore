#pragma once

#include <stdbool.h>

struct point {
	int x, y;
};

struct rectangle {
	struct point min;
	struct point max;
};

struct point rectangle_size(const struct rectangle *r);
bool rectangle_contains(const struct rectangle *r, int x, int y);
