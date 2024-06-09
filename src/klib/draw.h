#pragma once

#include <cairo/cairo.h>
#include <string.h>

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
