#pragma once

struct file_handler {
	char **exts;
	char *command;
};

// Will create a new string by joining a and b. You must free it later.
char *string_concat(const char *, const char *);
// Will create a new string by joining a and b with a /. You must free it
// later.
char *string_path_join(const char *, const char *);

int open_file(const char *filepath, const struct file_handler *);
int run_cmd(const char *str);
struct file_handler *read_handlers(const char *);
