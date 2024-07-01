#include "draw.h"

void path_rounded_rect_ab(cairo_t *cr, struct point a, struct point b, double r)
{
	double deg = M_PI / 180.0;
	cairo_new_sub_path(cr);
	cairo_arc(cr, b.x - r, a.y + r, r, -90 * deg, 0 * deg);
	cairo_arc(cr, b.x - r, b.y - r, r, 0 * deg, 90 * deg);
	cairo_arc(cr, a.x + r, b.y - r, r, 90 * deg, 180 * deg);
	cairo_arc(cr, a.x + r, a.y + r, r, 180 * deg, 270 * deg);
	cairo_close_path(cr);
}

void path_rounded_rect(cairo_t *cr, double x, double y, double width, double height, double radius)
{
	double degrees = M_PI / 180.0;
	cairo_new_sub_path(cr);
	cairo_arc(cr, x + width - radius, y + radius, radius, -90 * degrees, 0 * degrees);
	cairo_arc(cr, x + width - radius, y + height - radius, radius, 0 * degrees, 90 * degrees);
	cairo_arc(cr, x + radius, y + height - radius, radius, 90 * degrees, 180 * degrees);
	cairo_arc(cr, x + radius, y + radius, radius, 180 * degrees, 270 * degrees);
	cairo_close_path(cr);
}

double draw_text(cairo_t *cr, const char *str, int origin_x, int origin_y)
{
	cairo_font_extents_t fe;
	cairo_text_extents_t te;
	char letter[2];
	double x = 0;
	int len  = strlen(str);
	cairo_font_extents(cr, &fe);
	cairo_move_to(cr, 0, 0);
	for (int i = 0; i < len; i++) {
		*letter = '\0';
		strncat(letter, str + i, 1);
		cairo_text_extents(cr, letter, &te);
		cairo_move_to(cr, origin_x + x, origin_y);
		x += te.x_advance;
		cairo_show_text(cr, letter);
	}
	return x;
}

struct point text_size(cairo_t *cr, PangoFontDescription *desc, const char *str)
{
	struct point pt;
	int x, y;
	PangoLayout *layout = pango_cairo_create_layout(cr);
	pango_layout_set_font_description(layout, desc);
	pango_layout_set_text(layout, str, -1);
	pango_layout_get_size(layout, &x, &y);
	g_object_unref(layout);
	pt.x = (double)x / PANGO_SCALE;
	pt.y = (double)y / PANGO_SCALE;
	return pt;
}

struct point draw_text2(cairo_t *cr, PangoFontDescription *desc, const char *str)
{
	struct point pt;
	int x, y;
	PangoLayout *layout = pango_cairo_create_layout(cr);
	pango_layout_set_font_description(layout, desc);
	pango_layout_set_text(layout, str, -1);
	pango_layout_get_size(layout, &x, &y);
	pango_cairo_show_layout(cr, layout);
	g_object_unref(layout);
	pt.x = (double)x / PANGO_SCALE;
	pt.y = (double)y / PANGO_SCALE;
	return pt;
}
