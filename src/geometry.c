#include "geometry.h"

bool rectangle_contains(const struct rectangle *r, int x, int y) {
	if (r->min.x < x && r->max.x > x && r->min.y < y && r->max.y > y) {
		return true;
	}
	return false;
}
