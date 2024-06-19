#pragma once

#include <stdbool.h>

struct point {
	int x, y;
};

struct rectangle {
	struct point min;
	struct point max;
};

bool rectangle_contains(const struct rectangle *r, int x, int y);
