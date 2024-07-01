#pragma once

#include <stdbool.h>

#define M_PI 3.14159265358979323846

struct point {
	double x, y;
};

struct rectangle {
	struct point min;
	struct point max;
};

double double_lerp(double a, double b, double t);

struct point point_zero();
bool point_is_zero(const struct point *pt);
bool point_equal(const struct point *, const struct point *);
struct point point_add(struct point, struct point);
struct point point_sub(struct point, struct point);
struct point point_lerp(const struct point *a, const struct point *b, double t);

struct rectangle rectangle_zero();
struct rectangle rectangle_from_abxy(int, int, int, int);
struct rectangle rectangle_from_abwh(int a, int b, int w, int h);
struct rectangle rectangle_add_point(struct rectangle r, struct point);
struct point rectangle_size(const struct rectangle *r);
bool rectangle_contains(const struct rectangle *r, int x, int y);
bool rectangle_contains_point(const struct rectangle *, const struct point *);
bool rectangle_intersects(const struct rectangle *, const struct rectangle *);
bool rectangle_is_zero(const struct rectangle *r);
