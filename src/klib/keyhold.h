#pragma once

#include <math.h>
#include <stdlib.h>
#include <time.h>
#include <xkbcommon/xkbcommon.h>

struct keyhold {
	long pressed_time;
	long counter;
	xkb_keysym_t key;
	unsigned char *utf8;
	struct keyhold *next;
};

struct keyhold *keyhold_new(xkb_keysym_t);
struct keyhold *keyhold_add(struct keyhold *, xkb_keysym_t);
struct keyhold *keyhold_remove(struct keyhold *, xkb_keysym_t);
struct keyhold *keyhold_check(struct keyhold *, xkb_keysym_t);
void keyhold_process(
	struct keyhold *, int32_t, int32_t, void (*fn)(xkb_keysym_t)
);
