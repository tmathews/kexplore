#pragma once

#include <cairo/cairo.h>
#include <string.h>

#define M_PI 3.14159265358979323846

void path_rounded_rect(
	cairo_t *cr, double x, double y, double width, double height, double radius
) {
	double degrees = M_PI / 180.0;
	cairo_new_sub_path(cr);
	cairo_arc(
		cr, x + width - radius, y + radius, radius, -90 * degrees, 0 * degrees
	);
	cairo_arc(
		cr, x + width - radius, y + height - radius, radius, 0 * degrees,
		90 * degrees
	);
	cairo_arc(
		cr, x + radius, y + height - radius, radius, 90 * degrees, 180 * degrees
	);
	cairo_arc(cr, x + radius, y + radius, radius, 180 * degrees, 270 * degrees);
	cairo_close_path(cr);
}

double draw_text(cairo_t *cr, const char *str, int origin_x, int origin_y) {
	cairo_font_extents_t fe;
	cairo_text_extents_t te;
	char letter[2];
	double x = 0;
	int len = strlen(str);
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
