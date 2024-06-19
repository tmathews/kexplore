#include "utils.h"

#include <stdlib.h>
#include <string.h>
#include <unistd.h>

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
