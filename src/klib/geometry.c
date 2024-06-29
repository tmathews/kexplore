#include "geometry.h"

struct fpoint point_to_fpoint(struct point p)
{
	return (struct fpoint){
		.x = p.x,
		.y = p.y,
	};
}

struct point point_add(struct point a, struct point b)
{
	a.x += b.x;
	a.y += b.y;
	return a;
}

struct rectangle rectangle_zero()
{
	return rectangle_from_abxy(0, 0, 0, 0);
}

struct rectangle rectangle_from_abxy(int a, int b, int c, int d)
{
	return (struct rectangle){
		.min = (struct point){
			.x = a,
			.y = b,
		},
		.max = (struct point){
			.x = c,
			.y = d,
		},
	};
}

struct rectangle rectangle_add_point(struct rectangle r, struct point p)
{
	r.min = point_add(r.min, p);
	r.max = point_add(r.max, p);
	return r;
}

struct point rectangle_size(const struct rectangle *r)
{
	struct point pt;
	pt.x = r->max.x - r->min.x;
	pt.y = r->max.y - r->min.y;
	return pt;
}

bool rectangle_contains(const struct rectangle *r, int x, int y)
{
	if (r->min.x < x && r->max.x > x && r->min.y < y && r->max.y > y) {
		return true;
	}
	return false;
}

bool rectangle_contains_point(const struct rectangle *r, const struct point *p)
{
	return rectangle_contains(r, p->x, p->y);
}

bool rectangle_intersects(const struct rectangle *a, const struct rectangle *b)
{
	return (!(
		b->min.x > a->max.x ||
		b->max.x < a->min.x ||
		b->min.y > a->max.y ||
		b->max.y < a->min.y));
}

bool rectangle_is_zero(const struct rectangle *r)
{
	if (r->min.x == r->max.x && r->min.y == r->max.y)
		return true;
	return false;
}
