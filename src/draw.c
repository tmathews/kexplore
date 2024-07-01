#include "klib/draw.h"
#include "cairo.h"
#include "klib/geometry.h"
#include "main.h"
#include "node.h"
#include "stb_ds.h"
#include <dirent.h>

extern struct core core;

extern double icon_rotation;

void draw_svg(cairo_t *cr, RsvgHandle *h, const struct rectangle *r)
{
	struct point size  = rectangle_size(r);
	RsvgRectangle rect = {
		.x      = r->min.x,
		.y      = r->min.y,
		.width  = size.x,
		.height = size.y,
	};
	rsvg_handle_render_document(h, cr, &rect, NULL);
}

void draw_svg_colored(cairo_t *cr, RsvgHandle *h, const struct rectangle *rect, double r, double g, double b, double a)
{
	cairo_save(cr);
	cairo_push_group(cr);
	draw_svg(cr, h, rect);
	cairo_pattern_t *pat = cairo_pop_group(cr);
	cairo_set_source_rgba(cr, r, g, b, a);
	cairo_mask(cr, pat);
	cairo_fill(cr);
	cairo_restore(cr);
	cairo_pattern_destroy(pat);
}

void draw(struct draw_context ctx)
{
	int w             = ctx.surface_state->width;
	int h             = ctx.surface_state->height;
	struct point size = {.x = w, .y = h};
	//  Draw background
	cairo_rectangle(ctx.cr, 0, 0, w, h);
	cairo_set_source_rgba(ctx.cr, 0, 0, 0, 0.8);
	cairo_fill(ctx.cr);
	// Reset all our hit boxes
	for (int i = 0; i < arrlen(core.boxes); i++) {
		free(core.boxes[i].userdata);
	}
	arrfree(core.boxes);
	core.boxes      = NULL;
	core.camera.max = point_add(core.camera.min, (struct point){.x = w, .y = h});
	draw_entries(ctx.cr, core.root, core.camera);
	draw_preview(ctx.cr, core.selected_file, size);
	draw_navigation(ctx.cr, core.camera);
}

void draw_navigation(cairo_t *cr, struct rectangle camera)
{
	int ox             = 20;
	int oy             = 20;
	int size           = 22;
	int padding        = 20;
	struct point csize = rectangle_size(&camera);
	{ // Draw background bar
		cairo_save(cr);
		cairo_set_source_rgba(cr, 0, 0, 0, .9);
		cairo_rectangle(cr, 0, 0, csize.x, 62);
		cairo_fill(cr);
		cairo_restore(cr);
	}
	{
		cairo_save(cr);
		cairo_set_line_width(cr, 1);
		cairo_set_source_rgba(cr, 1, 1, 1, 0.3);
		cairo_move_to(cr, 0, 62);
		cairo_line_to(cr, csize.x, 62);
		cairo_stroke(cr);
		cairo_restore(cr);
	}
	cairo_save(cr);
	// Draw root button
	{
		struct rectangle rect = rectangle_from_abwh(ox, oy, size, size);
		struct hitbox hitbox  = {
			 .area    = rectangle_add_point(rect, camera.min),
			 .trigger = 2,
        };
		arrput(core.boxes, hitbox);
		// draw button
		// path_rounded_rect_ab(cr, rect.min, rect.max, 5);
		// cairo_set_source_rgb(cr, 1, 1, 1);
		// cairo_set_line_width(cr, 3);
		// cairo_stroke(cr);
		draw_svg_colored(cr, core.icons.home, &rect, 1, 1, 1, 1);
		ox += size + padding;
	}
	// Draw relative button
	{
		struct rectangle rect = rectangle_from_abwh(ox, oy, size, size);
		struct hitbox hitbox  = {
			 .area    = rectangle_add_point(rect, camera.min),
			 .trigger = 3,
        };
		arrput(core.boxes, hitbox);
		// draw button
		// path_rounded_rect_ab(cr, rect.min, rect.max, 5);
		// cairo_set_source_rgb(cr, 1, 1, 1);
		// cairo_set_line_width(cr, 3);
		// cairo_stroke(cr);
		draw_svg_colored(cr, core.icons.selection, &rect, 1, 1, 1, 1);
		ox += size + padding;
	}
	// Draw parent button
	{
		struct rectangle rect = rectangle_from_abwh(ox, oy, size, size);
		struct hitbox hitbox  = {
			 .area    = rectangle_add_point(rect, camera.min),
			 .trigger = 4,
        };
		arrput(core.boxes, hitbox);
		// draw button
		// path_rounded_rect_ab(cr, rect.min, rect.max, 5);
		// cairo_set_source_rgb(cr, 1, 1, 1);
		// cairo_set_line_width(cr, 3);
		// cairo_stroke(cr);
		draw_svg_colored(cr, core.icons.parent, &rect, 1, 1, 1, 1);
		ox += size + padding;
	}
	// Draw this node button
	{
		struct rectangle rect = rectangle_from_abwh(ox, oy, size, size);
		struct hitbox hitbox  = {
			 .area    = rectangle_add_point(rect, camera.min),
			 .trigger = 5,
        };
		arrput(core.boxes, hitbox);
		// draw button
		// path_rounded_rect_ab(cr, rect.min, rect.max, 5);
		// cairo_set_source_rgb(cr, 1, 1, 1);
		// cairo_set_line_width(cr, 3);
		// cairo_stroke(cr);
		draw_svg_colored(cr, core.icons.top, &rect, 1, 1, 1, 1);
		ox += size + padding;
	}
	// Draw this copy path button
	{
		struct rectangle rect = rectangle_from_abwh(ox, oy, size, size);
		struct hitbox hitbox  = {
			 .area    = rectangle_add_point(rect, camera.min),
			 .trigger = 6,
        };
		arrput(core.boxes, hitbox);
		// draw button
		// path_rounded_rect_ab(cr, rect.min, rect.max, 5);
		// cairo_set_source_rgb(cr, 1, 1, 1);
		// cairo_set_line_width(cr, 3);
		// cairo_stroke(cr);
		draw_svg_colored(cr, core.icons.copy, &rect, 1, 1, 1, 1);
		ox += size + padding;
	}
	// Draw open terminal button
	{
		struct rectangle rect = rectangle_from_abwh(ox, oy, size, size);
		struct hitbox hitbox  = {
			 .area    = rectangle_add_point(rect, camera.min),
			 .trigger = 7,
        };
		arrput(core.boxes, hitbox);
		draw_svg_colored(cr, core.icons.terminal, &rect, 1, 1, 1, 1);
		ox += size + padding;
	}
	// Draw URL bar
	{
		oy -= 4;
		struct rectangle rect = rectangle_from_abwh(
			ox, oy,
			csize.x - ox - padding, oy + 15);
		path_rounded_rect_ab(cr, rect.min, rect.max, 5);
		cairo_save(cr);
		cairo_set_source_rgb(cr, 1, 1, 1);
		cairo_set_line_width(cr, 1);
		cairo_stroke(cr);
		cairo_translate(cr, ox + 10, oy + 2);
		char *text;
		if (core.selected_file != NULL)
			text = core.selected_file;
		else
			text = "No selection...";
		draw_text2(cr, core.fonts.normal, text);
		cairo_restore(cr);
	}
	cairo_restore(cr);
}

void draw_entries(cairo_t *cr, struct node *n, struct rectangle camera)
{
	// Calculate the area if it's the first time we are encountering it.
	if (rectangle_is_zero(&n->rect)) {
		node_calc_size(n, cr, core.fonts.normal);
		focus_to_rect(&n->rect, &camera);
	}
	struct point render_offset;
	render_offset.x = -camera.min.x;
	render_offset.y = -camera.min.y;
	// Draw our node if in camera
	if (rectangle_intersects(&camera, &n->rect)) {
		// Draw the box
		cairo_save(cr);
		// TODO if the path is outside the render camera, then squash it to just
		// outside to save drawing. Find out if this is a micro optimization.
		path_rounded_rect_ab(cr,
			point_add(n->rect.min, render_offset),
			point_add(n->rect.max, render_offset), 5);
		cairo_set_source_rgba(cr, 0, 0, 0, 0.5);
		cairo_fill_preserve(cr);
		cairo_set_source_rgb(cr, 1, 1, 1);
		cairo_set_line_width(cr, 3);
		cairo_stroke(cr);
		cairo_restore(cr);
		{
			struct hitbox hitbox;
			hitbox.area        = n->rect;
			hitbox.trigger     = 1;
			struct ev_entry *e = calloc(1, sizeof(struct ev_entry));
			e->type            = 2;
			e->n               = n;
			hitbox.userdata    = e;
			arrput(core.boxes, hitbox);
		}
		// Draw the close button
		if (n->parent != NULL) {
			struct hitbox hitbox;
			hitbox.area.min    = point_add(n->rect.min, (struct point){.x = 0, .y = -30});
			hitbox.area.max    = point_add(hitbox.area.min, (struct point){.x = 20, .y = 20});
			hitbox.area        = rectangle_add_point(hitbox.area, (struct point){.x = 2, .y = 5});
			hitbox.trigger     = 1;
			struct ev_entry *e = calloc(1, sizeof(struct ev_entry));
			e->n               = n;
			e->type            = 3;
			hitbox.userdata    = e;
			arrput(core.boxes, hitbox);
			// draw button
			struct rectangle rect = hitbox.area;
			rect.min              = point_add(rect.min, render_offset);
			rect.max              = point_add(rect.max, render_offset);
			draw_svg_colored(cr, core.icons.close, &rect, 1, 1, 1, 1);
			// struct point btnSize = rectangle_size(&hitbox.area);
			//  path_rounded_rect_ab(cr, rect.min, rect.max, 5);
			//  cairo_set_source_rgb(cr, 1, 1, 1);
			//  cairo_set_line_width(cr, 3);
			//  cairo_stroke(cr);
		}
	}
	// Draw each item
	int len = arrlen(n->items);
	for (int i = 0; i < len; i++) {
		struct node_item item = n->items[i];
		// Add our parent offset to get the global rect
		struct rectangle rect = rectangle_add_point(item.rect, n->rect.min);
		bool in_view          = rectangle_intersects(&camera, &rect);
		bool selected         = core.selection.n == n && i == core.selection.i;
		if (in_view) {
			cairo_save(cr);
			if (selected) {
				cairo_set_source_rgb(cr, 1, 0, 0);
			} else if (item.next != NULL) {
				cairo_set_source_rgb(cr, 0, 1, 0);
			} else {
				cairo_set_source_rgb(cr, 1, 1, 1);
			}
			cairo_move_to(cr, rect.min.x + render_offset.x, rect.min.y + render_offset.y);
			draw_text2(cr, core.fonts.normal, item.info.d_name);
			{
				// TODO make hitbox generation a method or something
				struct hitbox hitbox;
				hitbox.area        = rect;
				hitbox.trigger     = 1;
				struct ev_entry *e = calloc(1, sizeof(struct ev_entry));
				e->n               = n;
				e->i               = i;
				e->type            = 0;
				hitbox.userdata    = e;
				arrput(core.boxes, hitbox);
			}
			if (selected && item.info.d_type != DT_DIR) { // open button
				struct hitbox hitbox;
				hitbox.area.min.x  = rect.max.x + 10;
				hitbox.area.min.y  = rect.min.y + 4;
				hitbox.area.max.x  = hitbox.area.min.x + 20;
				hitbox.area.max.y  = hitbox.area.min.y + 20;
				hitbox.trigger     = 1;
				struct ev_entry *e = calloc(1, sizeof(struct ev_entry));
				e->n               = n;
				e->i               = i;
				e->type            = 1;
				hitbox.userdata    = e;
				arrput(core.boxes, hitbox);
				// draw button
				struct rectangle svgrect = hitbox.area;
				svgrect.min              = point_add(svgrect.min, render_offset);
				svgrect.max              = point_add(svgrect.max, render_offset);
				draw_svg_colored(cr, core.icons.open, &svgrect, 1, 1, 1, 1);
			}
			cairo_restore(cr);
		}
		if (item.next != NULL) {
			if (item.next->busy) {
				cairo_save(cr);
				struct rectangle rrect = rectangle_from_abwh(-8, -8, 16, 16);
				struct rectangle nrect = rectangle_from_abwh(rect.max.x + 10, rect.min.y, 16, 16);
				nrect                  = rectangle_add_point(rect, render_offset);
				cairo_translate(cr, nrect.max.x + 16, nrect.min.y + 16);
				cairo_rotate(cr, icon_rotation);
				draw_svg_colored(cr, core.icons.busy, &rrect, 1, 1, 1, 1);
				cairo_restore(cr);
			} else {
				draw_entries(cr, item.next, camera);
				cairo_save(cr);
				cairo_set_source_rgb(cr, 1, 1, 1);
				cairo_set_line_width(cr, 3);
				cairo_move_to(cr,
					n->rect.max.x + render_offset.x,
					rect.min.y + ((rect.max.y - rect.min.y) / 2) + render_offset.y);
				cairo_line_to(cr,
					item.next->rect.min.x + render_offset.x,
					item.next->rect.min.y + render_offset.y + 5);
				cairo_stroke(cr);
				cairo_restore(cr);
			}
		}
	}
}

void draw_preview(cairo_t *cr, const char *filename, struct point window_size)
{
	int size = 400;
	double x, y, w, h = 0;
	double scale = 1;
	cairo_save(cr);
	if (core.preview_surface != NULL) {
		w     = cairo_image_surface_get_width(core.preview_surface);
		h     = cairo_image_surface_get_height(core.preview_surface);
		scale = (float)size / (float)w;
		x     = window_size.x - 10 - (w * scale);
		y     = window_size.y - 10 - (h * scale);
		cairo_translate(cr, x, y);
		cairo_scale(cr, scale, scale);
		cairo_set_source_surface(cr, core.preview_surface, 0, 0);
		cairo_paint(cr);
		cairo_restore(cr);
	} else if (core.preview_svg != NULL) {
		x                  = window_size.x - 10 - size;
		y                  = window_size.y - 10 - size;
		w                  = size;
		h                  = size;
		int padding        = 40;
		RsvgRectangle rect = {
			.x      = x + padding,
			.y      = y + padding,
			.width  = size - padding * 2,
			.height = size - padding * 2,
		};
		cairo_set_source_rgba(cr, 1, 1, 1, 1);
		rsvg_handle_render_document(core.preview_svg, cr, &rect, NULL);
	}
	cairo_save(cr);
	cairo_rectangle(cr, x + .5, y + .5, w * scale, h * scale);
	cairo_set_line_width(cr, 1);
	cairo_set_source_rgba(cr, 1, 1, 1, 1);
	cairo_stroke(cr);
	cairo_restore(cr);
}

void draw_selection(cairo_t *cr, struct point surf_size)
{
	char *text = NULL;
	if (core.selected_file == NULL) {
		text = "No file selected";
	} else {
		text = core.selected_file;
	}
	const float padding = 80;
	struct point size   = text_size(cr, core.fonts.normal, text);
	struct point origin = {
		.x = padding,
		.y = 15,
	};
	struct point extends = {
		.x = surf_size.x - padding,
		.y = origin.y + size.y + 10,
	};
	cairo_save(cr);
	cairo_set_source_rgba(cr, 1, 1, 1, 0.1);
	cairo_rectangle(cr, 0, 0, surf_size.x, extends.y + 15);
	cairo_fill(cr);
	cairo_restore(cr);
	cairo_save(cr);
	path_rounded_rect_ab(cr, origin, extends, 6);
	cairo_set_source_rgba(cr, 1, 1, 1, 0.8);
	cairo_fill_preserve(cr);
	cairo_set_line_width(cr, 1);
	cairo_stroke(cr);
	cairo_move_to(cr, origin.x + 15, origin.y + ((extends.y - origin.y) / 2) - (int)(size.y / 2));
	cairo_set_source_rgba(cr, 0, 0, 0, 1);
	draw_text2(cr, core.fonts.normal, text);
	cairo_restore(cr);
}

void focus_to_rect(const struct rectangle *rect, const struct rectangle *camera)
{
	struct point cam_size   = rectangle_size(camera);
	struct point frame_size = rectangle_size(rect);
	frame_size.x *= 0.5;
	frame_size.y *= 0.5;
	cam_size.x *= 0.5;
	cam_size.y *= 0.5;
	if (frame_size.y > cam_size.y) {
		frame_size.y = cam_size.y - 100;
	}
	core.camera_target  = point_add(point_sub(rect->min, cam_size), frame_size);
	core.camera_refocus = true;
}
