#pragma once

#include <cairo/cairo.h>
#include <pango/pango-layout.h>
#include <pango/pangocairo.h>

#include "geometry.h"

void path_rounded_rect_ab(cairo_t *cr, struct point a, struct point b, double r);
void path_rounded_rect(cairo_t *cr, double x, double y, double width, double height, double radius);
double draw_text(cairo_t *cr, const char *str, int origin_x, int origin_y);
struct point text_size(cairo_t *cr, PangoFontDescription *desc, const char *str);
struct point draw_text2(cairo_t *cr, PangoFontDescription *desc, const char *str);
