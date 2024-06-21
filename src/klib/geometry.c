#include "geometry.h"

struct point rectangle_size(const struct rectangle *r) {
	struct point pt;
	pt.x = r->max.x - r->min.x;
	pt.y = r->max.y - r->min.y;
	return pt;
}

bool rectangle_contains(const struct rectangle *r, int x, int y) {
	if (r->min.x < x && r->max.x > x && r->min.y < y && r->max.y > y) {
		return true;
	}
	return false;
}
