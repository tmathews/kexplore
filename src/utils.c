#include "utils.h"

#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <unistd.h>
#include <wordexp.h>

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

int open_file(const char *filepath) {
	char *cmd = malloc(strlen(filepath) + 50);
	cmd[0] = 0;
	if (is_file_ext(filepath, ".png") || is_file_ext(filepath, ".jpg") ||
		is_file_ext(filepath, ".gif") || is_file_ext(filepath, ".heic")) {
		strcpy(cmd, "imv '");
	} else if (is_file_ext(filepath, ".mkv")) {
		strcpy(cmd, "mpv '");
	} else {
		free(cmd);
		return 1;
	}
	strcat(cmd, filepath);
	strcat(cmd, "'");
	// printf("Opening file: %s\n", filepath);
	printf("cmd>>> %s\n", cmd);
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
	if (strchr(bin, '/'))
		execv(bin, w.we_wordv);
	else
		execvp(bin, w.we_wordv);
	return 0;
}
