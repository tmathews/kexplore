#include "keyhold.h"

static long millis() {
	struct timespec _t;
	clock_gettime(CLOCK_MONOTONIC, &_t);
	return _t.tv_sec*1000 + lround(_t.tv_nsec/1e6);
}

struct keyhold *keyhold_new(xkb_keysym_t key) {
	struct keyhold *k = malloc(sizeof(struct keyhold));
	k->key = key;
	k->pressed_time = millis();
	k->counter = 0;
	k->next = NULL;
	return k;
}

struct keyhold *keyhold_add(struct keyhold *root, xkb_keysym_t key) {
	struct keyhold *tmp, *n;
	n = keyhold_new(key);
	if (root == NULL)
		return n;
	tmp = root;
	// Find the last node in linked list.
	while (tmp != NULL) {
		// Don't add the key because it already exists.
		if (tmp->key == key) {
			free(n);
			return root;
		}
		if (tmp->next == NULL)
			break;
		tmp = tmp->next;
	}
	tmp->next = n;
	return root;
}

struct keyhold *keyhold_remove(struct keyhold *root, xkb_keysym_t key) {
	struct keyhold *tmp, *parent;
	tmp = root;
	parent = NULL;
	while (tmp != NULL) {
		if (tmp->key == key) {
			if (parent != NULL) {
				parent->next = tmp->next;
				root = parent;
			} else {
				root = tmp->next;
			}
			free(tmp);
			break;
		}
		parent = tmp;
		tmp = tmp->next;
	}
	return root;
}

struct keyhold *keyhold_check(struct keyhold *n, xkb_keysym_t key) {
	while (n != NULL) {
		if (n->key == key)
			break;
		n = n->next;
	}
	return n;
}

void keyhold_process(struct keyhold *n, int32_t delay, int32_t rate, void (*fn)(xkb_keysym_t)) {
	static long last = 0;
	long milli = millis();
	long delta = milli - last;
	if (last == 0) last = milli;
	while (n != NULL) {
		long tsince = milli - n->pressed_time;
		if (tsince > delay) {
			n->counter += delta;
			if (n->counter >= rate/1000) {
				n->counter = 0;
				fn(n->key);
			}
		}
		n = n->next;
	}
	last = milli;
}
