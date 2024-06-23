#include <ctype.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <unistd.h>
#include <wordexp.h>

#include "stb_ds.h"
#include "utils.h"

char *string_concat(const char *a, const char *b) {
	char *x = malloc(strlen(a) + strlen(b) + 1);
	x[0] = 0;
	strcpy(x, a);
	strcat(x, b);
	return x;
}

char *string_path_join(const char *a, const char *b) {
	char *npath = malloc(strlen(a) + strlen(b) + 2);
	npath[0] = 0;
	strcpy(npath, a);
	strcat(npath, "/");
	strcat(npath, b);
	return npath;
}

bool is_file_ext(const char *filepath, const char *ext) {
	char *b = strrchr(filepath, '.');
	if (strcasecmp(ext, b) == 0) {
		return true;
	}
	return false;
}

int open_file(const char *filepath, const struct file_handler *handlers) {
	char *ext = strrchr(filepath, '.');
	if (ext == NULL) {
		return -1;
	}
	ext++; // move past the period
	struct file_handler h;
	bool found = false;
	for (int i = 0; i < arrlen(handlers); i++) {
		for (int n = 0; n < arrlen(handlers[i].exts); n++) {
			if (strcmp(handlers[i].exts[n], ext) == 0) {
				h = handlers[i];
				found = true;
				break;
			}
		}
	}
	if (!found) {
		return -1;
	}
	// printf("file: '%s' got handler '%s'\n", filepath, h.command);
	char *cmd = calloc(1, strlen(h.command) + strlen(filepath) + 1);
	strcpy(cmd, h.command);
	char *pos = strstr(cmd, "{FILE}");
	strcpy(pos, filepath);
	char *epos = strstr(h.command, "{FILE}") + 6;
	pos = cmd + strlen(cmd);
	strcpy(pos, epos);
	// printf("cmd>>> %s\n", cmd);
	int status = run_cmd(cmd);
	free(cmd);
	return status;
}

int run_cmd(const char *str) {
	wordexp_t w;
	switch (wordexp(str, &w, WRDE_NOCMD)) {
	case 0:
		break;
	case WRDE_NOSPACE:
	case WRDE_CMDSUB:
	case WRDE_BADCHAR:
	default:
		return -1;
	}
	if (w.we_wordc < 1) {
		return -1;
	}
	const char *bin = w.we_wordv[0];
	if (!bin || !*bin) {
		return -1;
	}
	int code = fork();
	if (code < 0) {
		return code;
	} else if (code > 0) {
		return 0;
	}
	if (strchr(bin, '/'))
		execv(bin, w.we_wordv);
	else
		execvp(bin, w.we_wordv);
	return 0;
}

struct file_handler *read_handlers(const char *filename) {
	printf("reading: %s\n", filename);
	// ext, ...: command
	FILE *fp;
	ssize_t read;
	fp = fopen(filename, "r");
	if (fp == NULL)
		return NULL;
	bool tick, tock = false;
	char c;
	char buf[4096];
	buf[0] = '\0';
	struct file_handler *xs = NULL;
	struct file_handler h;
	h.command = NULL;
	h.exts = NULL;
	while ((c = fgetc(fp)) && c != EOF) {
		if (c == '\n') {
			printf("buf: '%s'\n", buf);
			h.command = malloc(strlen(buf) + 1);
			strcpy(h.command, buf);
			arrput(xs, h);
			h.command = NULL;
			h.exts = NULL;
			memset(buf, 0, sizeof(buf));
			tick = false;
			tock = false;
		} else if (!tick) {
			switch (c) {
			case ' ':
				break;
			case ',':
			case ':': {
				printf("buf: '%s'\n", buf);
				char *ext = calloc(1, strlen(buf) + 1);
				strcpy(ext, buf);
				arrput(h.exts, ext);
				memset(buf, 0, sizeof(buf));
			} break;
			default: {
				strncat(buf, &c, 1);
			} break;
			}
			if (c == ':') {
				tick = true;
			}
		} else {
			if (!isspace(c)) {
				tock = true;
			}
			if (tock) {
				strncat(buf, &c, 1);
			}
		}
	}
	fclose(fp);
	return xs;
}
