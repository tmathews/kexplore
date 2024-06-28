#pragma once

#include <stdbool.h>

#define M_PI 3.14159265358979323846

struct fpoint {
	double x, y;
};

struct point {
	int x, y;
};

struct rectangle {
	struct point min;
	struct point max;
};

struct fpoint point_to_fpoint(struct point);
struct point point_add(struct point, struct point);

struct rectangle rectangle_zero();
struct rectangle rectangle_from_abxy(int, int, int, int);
struct rectangle rectangle_add_point(struct rectangle r, struct point);
struct point rectangle_size(const struct rectangle *r);
bool rectangle_contains(const struct rectangle *r, int x, int y);
bool rectangle_contains_point(const struct rectangle *, const struct point *);
bool rectangle_intersects(const struct rectangle *, const struct rectangle *);
bool rectangle_is_zero(const struct rectangle *r);
