#pragma once

// Will create a new string by joining a and b. You must free it later.
char *string_concat(const char *, const char *);
// Will create a new string by joining a and b with a /. You must free it
// later.
char *string_path_join(const char *, const char *);
