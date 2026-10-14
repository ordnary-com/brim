/*
 * The Brim native runtime.
 *
 * Compiled together with the object file the LLVM backend produces and linked
 * into every native executable. It provides:
 *
 * - the `brim_rt_*` functions listed in `brim_runtime::RuntimeFn` (printing,
 *   string operations), with the same behaviour as the reference interpreter;
 * - the backend-internal entry points `brim_rt_enter`, `brim_rt_leave` and
 *   `brim_rt_trap`, which track the Brim call stack and report run-time
 *   errors exactly as `brim run` does, and `brim_rt_retain` /
 *   `brim_rt_release`, which manage string memory;
 * - the process entry point `main`, which calls the generated `brim_entry`.
 *
 * Strings are `BrimStr` objects. Their layout must match the constants the
 * backend emits for string literals: a reference count (-1 for static
 * objects), a byte length, then the UTF-8 bytes (not NUL-terminated).
 *
 * Memory management: heap strings are reference counted and freed when the
 * count drops to zero. Static strings (rc -1) are never counted. Brim has no
 * threads yet, so the counts are not atomic.
 *
 * With the environment variable BRIM_RT_CHECK=1, memory is checked instead
 * of freed: a retain or release of a freed string aborts the program, and
 * strings and lists still allocated when `main` returns are reported as a
 * leak. The
 * compiler's tests run every native program this way.
 */

#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct BrimStr {
    int64_t rc;
    int64_t len;
    char data[];
} BrimStr;

/* Defined by the generated code. */
extern const int64_t brim_max_call_depth;
extern void brim_entry(void);

static const char **frames;
static int64_t depth;

/* Memory checking (BRIM_RT_CHECK=1). */
static int checking;
static int64_t live_strings;
#define FREED INT64_MIN

static void fatal(const char *message) {
    fflush(stdout);
    fprintf(stderr, "error: %s\n", message);
    exit(101);
}

/* ---- run-time errors ---------------------------------------------------- */

static void print_call(int64_t i, int first) {
    if (!first) {
        fputs(" -> ", stderr);
    }
    fputs(frames[i], stderr);
}

/*
 * Reports a run-time error and exits with status 101.
 *
 * `head` is the rendered diagnostic without notes, `sep` the line printed
 * between it and the notes, and `note` the prefix of a note line. The backend
 * renders all three at compile time; only the call stack is added here, in
 * the same format the `brim` CLI uses for interpreter errors.
 */
/* Prints the call-stack notes and exits with status 101. */
_Noreturn static void finish_trap(const char *sep, const char *note) {
    if (depth > 1) {
        fputs(sep, stderr);
        if (depth > 8) {
            fprintf(stderr, "%s%" PRId64 " calls omitted from the call stack\n", note, depth - 6);
            fprintf(stderr, "%scall stack: ", note);
            for (int64_t i = 0; i < 3; i++) {
                print_call(i, i == 0);
            }
            fputs(" -> ...", stderr);
            for (int64_t i = depth - 3; i < depth; i++) {
                print_call(i, 0);
            }
        } else {
            fprintf(stderr, "%scall stack: ", note);
            for (int64_t i = 0; i < depth; i++) {
                print_call(i, i == 0);
            }
        }
        fputc('\n', stderr);
    }
    fflush(stderr);
    exit(101);
}

_Noreturn void brim_rt_trap(const char *head, const char *sep, const char *note) {
    fflush(stdout);
    fputs(head, stderr);
    finish_trap(sep, note);
}

/*
 * A list index outside the list. The message has the index and length in
 * the middle of its first line, so the rendered diagnostic comes in two
 * parts: `prefix` before them and `suffix` after.
 */
typedef struct BrimTrapIndex {
    const char *prefix;
    const char *suffix;
    const char *sep;
    const char *note;
} BrimTrapIndex;

_Noreturn static void trap_index(const BrimTrapIndex *t, int64_t index, int64_t len) {
    fflush(stdout);
    fputs(t->prefix, stderr);
    fprintf(stderr, "the index is %" PRId64 " but the length is %" PRId64, index, len);
    fputs(t->suffix, stderr);
    finish_trap(t->sep, t->note);
}

/* Called before every Brim call; traps (stack overflow) at the call site. */
void brim_rt_enter(const char *name, const char *head, const char *sep, const char *note) {
    if (depth >= brim_max_call_depth) {
        brim_rt_trap(head, sep, note);
    }
    frames[depth++] = name;
}

/* Called after every Brim call returns. */
void brim_rt_leave(void) {
    depth--;
}

/* ---- printing ------------------------------------------------------------ */

void brim_rt_print_i64(int64_t value) {
    printf("%" PRId64 "\n", value);
}

void brim_rt_print_bool(uint8_t value) {
    fputs(value ? "true\n" : "false\n", stdout);
}

void brim_rt_print_str(const BrimStr *s) {
    fwrite(s->data, 1, (size_t)s->len, stdout);
    fputc('\n', stdout);
}

/* ---- memory -------------------------------------------------------------- */

static BrimStr *str_alloc(int64_t len) {
    BrimStr *s = malloc(sizeof(BrimStr) + (size_t)len);
    if (s == NULL) {
        fatal("out of memory");
    }
    s->rc = 1;
    s->len = len;
    live_strings++;
    return s;
}

static void check_live(const BrimStr *s, const char *operation) {
    if (s->rc == FREED) {
        fflush(stdout);
        fprintf(stderr, "error: brim runtime check: %s of a freed string\n", operation);
        abort();
    }
}

/* Takes a reference. Null (an unset slot) and static strings are ignored. */
void brim_rt_retain(BrimStr *s) {
    if (s == NULL) {
        return;
    }
    if (checking) {
        check_live(s, "retain");
    }
    if (s->rc >= 0) {
        s->rc++;
    }
}

/* Drops a reference, freeing the string when it was the last one. */
void brim_rt_release(BrimStr *s) {
    if (s == NULL) {
        return;
    }
    if (checking) {
        check_live(s, "release");
    }
    if (s->rc < 0 || --s->rc > 0) {
        return;
    }
    live_strings--;
    if (checking) {
        s->rc = FREED; /* keep the memory to detect later use */
    } else {
        free(s);
    }
}

/* ---- strings ------------------------------------------------------------- */

BrimStr *brim_rt_str_concat(const BrimStr *a, const BrimStr *b) {
    BrimStr *s = str_alloc(a->len + b->len);
    memcpy(s->data, a->data, (size_t)a->len);
    memcpy(s->data + a->len, b->data, (size_t)b->len);
    return s;
}

uint8_t brim_rt_str_eq(const BrimStr *a, const BrimStr *b) {
    return a->len == b->len && memcmp(a->data, b->data, (size_t)a->len) == 0;
}

BrimStr *brim_rt_i64_to_str(int64_t value) {
    char buf[32];
    int len = snprintf(buf, sizeof buf, "%" PRId64, value);
    BrimStr *s = str_alloc(len);
    memcpy(s->data, buf, (size_t)len);
    return s;
}

BrimStr *brim_rt_bool_to_str(uint8_t value) {
    const char *text = value ? "true" : "false";
    int64_t len = (int64_t)strlen(text);
    BrimStr *s = str_alloc(len);
    memcpy(s->data, text, (size_t)len);
    return s;
}

/* ---- lists --------------------------------------------------------------- */

/*
 * A list is a reference-counted header followed by its elements, stored
 * inline. NULL is the empty list. Lists are values with copy-on-write: the
 * functions that change a list take ownership of it, and change it in place
 * when they hold the only reference (otherwise they work on a copy).
 *
 * Elements are copied with memcpy. For element types that contain strings
 * or lists, the generated code passes `retain` and `release` functions that
 * take a pointer to an element; for other types they are NULL.
 */
typedef void (*BrimElemFn)(void *elem);

typedef struct BrimList {
    int64_t rc;
    int64_t len;
    int64_t cap;
    int64_t elem_size;
    BrimElemFn release;
    char data[];
} BrimList;

static BrimList *list_alloc(int64_t elem_size, BrimElemFn release, int64_t cap) {
    if (cap < 4) {
        cap = 4;
    }
    BrimList *l = malloc(sizeof(BrimList) + (size_t)(cap * elem_size));
    if (l == NULL) {
        fatal("out of memory");
    }
    l->rc = 1;
    l->len = 0;
    l->cap = cap;
    l->elem_size = elem_size;
    l->release = release;
    if (checking) {
        live_strings++;
    }
    return l;
}

static char *elem_at(BrimList *l, int64_t i) {
    return l->data + i * l->elem_size;
}

void brim_rt_list_retain(BrimList *l) {
    if (l != NULL) {
        l->rc++;
    }
}

void brim_rt_list_release(BrimList *l) {
    if (l == NULL || --l->rc > 0) {
        return;
    }
    if (l->release != NULL) {
        for (int64_t i = 0; i < l->len; i++) {
            l->release(elem_at(l, i));
        }
    }
    if (checking) {
        live_strings--;
    }
    free(l);
}

int64_t brim_rt_list_len(const BrimList *l) {
    return l == NULL ? 0 : l->len;
}

/* Takes ownership of `l` and returns a list nothing else refers to. */
static BrimList *unique(BrimList *l, int64_t elem_size, BrimElemFn retain, BrimElemFn release,
                        int64_t extra) {
    if (l == NULL) {
        return list_alloc(elem_size, release, extra);
    }
    if (l->rc == 1) {
        return l;
    }
    BrimList *copy = list_alloc(elem_size, release, l->len + extra);
    memcpy(copy->data, l->data, (size_t)(l->len * elem_size));
    copy->len = l->len;
    if (retain != NULL) {
        for (int64_t i = 0; i < copy->len; i++) {
            retain(elem_at(copy, i));
        }
    }
    l->rc--;
    return copy;
}

/* Makes room for one more element. */
static BrimList *grow(BrimList *l) {
    if (l->len < l->cap) {
        return l;
    }
    int64_t cap = l->cap * 2;
    BrimList *bigger = realloc(l, sizeof(BrimList) + (size_t)(cap * l->elem_size));
    if (bigger == NULL) {
        fatal("out of memory");
    }
    bigger->cap = cap;
    return bigger;
}

static void check_index(const BrimList *l, int64_t i, int64_t limit, const BrimTrapIndex *t) {
    if (i < 0 || i >= limit) {
        trap_index(t, i, brim_rt_list_len(l));
    }
}

/* A pointer to element `i`, which must exist. */
void *brim_rt_list_at(BrimList *l, int64_t i, const BrimTrapIndex *t) {
    check_index(l, i, brim_rt_list_len(l), t);
    return elem_at(l, i);
}

BrimList *brim_rt_list_push(BrimList *l, const void *elem, int64_t elem_size, BrimElemFn retain,
                            BrimElemFn release) {
    l = grow(unique(l, elem_size, retain, release, 1));
    memcpy(elem_at(l, l->len), elem, (size_t)elem_size);
    if (retain != NULL) {
        retain(elem_at(l, l->len));
    }
    l->len++;
    return l;
}

BrimList *brim_rt_list_set(BrimList *l, int64_t i, const void *elem, int64_t elem_size,
                           BrimElemFn retain, BrimElemFn release, const BrimTrapIndex *t) {
    check_index(l, i, brim_rt_list_len(l), t);
    l = unique(l, elem_size, retain, release, 0);
    if (retain != NULL) {
        retain((void *)elem);
    }
    if (release != NULL) {
        release(elem_at(l, i));
    }
    memcpy(elem_at(l, i), elem, (size_t)elem_size);
    return l;
}

BrimList *brim_rt_list_truncate(BrimList *l, int64_t n, int64_t elem_size, BrimElemFn retain,
                                BrimElemFn release) {
    if (l == NULL || n >= l->len) {
        return l;
    }
    l = unique(l, elem_size, retain, release, 0);
    if (release != NULL) {
        for (int64_t i = n; i < l->len; i++) {
            release(elem_at(l, i));
        }
    }
    l->len = n;
    return l;
}

BrimList *brim_rt_list_insert(BrimList *l, int64_t i, const void *elem, int64_t elem_size,
                              BrimElemFn retain, BrimElemFn release, const BrimTrapIndex *t) {
    check_index(l, i, brim_rt_list_len(l) + 1, t);
    l = grow(unique(l, elem_size, retain, release, 1));
    memmove(elem_at(l, i + 1), elem_at(l, i), (size_t)((l->len - i) * elem_size));
    memcpy(elem_at(l, i), elem, (size_t)elem_size);
    if (retain != NULL) {
        retain(elem_at(l, i));
    }
    l->len++;
    return l;
}

BrimList *brim_rt_list_remove(BrimList *l, int64_t i, int64_t elem_size, BrimElemFn retain,
                              BrimElemFn release, const BrimTrapIndex *t) {
    check_index(l, i, brim_rt_list_len(l), t);
    l = unique(l, elem_size, retain, release, 0);
    if (release != NULL) {
        release(elem_at(l, i));
    }
    memmove(elem_at(l, i), elem_at(l, i + 1), (size_t)((l->len - i - 1) * elem_size));
    l->len--;
    return l;
}

uint8_t brim_rt_list_contains_i64(const BrimList *l, int64_t value) {
    for (int64_t i = 0; i < brim_rt_list_len(l); i++) {
        if (((const int64_t *)l->data)[i] == value) {
            return 1;
        }
    }
    return 0;
}

uint8_t brim_rt_list_contains_bool(const BrimList *l, uint8_t value) {
    for (int64_t i = 0; i < brim_rt_list_len(l); i++) {
        if ((((const uint8_t *)l->data)[i] != 0) == (value != 0)) {
            return 1;
        }
    }
    return 0;
}

uint8_t brim_rt_list_contains_str(const BrimList *l, const BrimStr *value) {
    for (int64_t i = 0; i < brim_rt_list_len(l); i++) {
        if (brim_rt_str_eq(((BrimStr *const *)l->data)[i], value)) {
            return 1;
        }
    }
    return 0;
}

BrimStr *brim_rt_list_join(const BrimList *l, const BrimStr *sep) {
    int64_t n = brim_rt_list_len(l);
    BrimStr *const *items = n > 0 ? (BrimStr *const *)l->data : NULL;
    int64_t total = 0;
    for (int64_t i = 0; i < n; i++) {
        total += items[i]->len + (i > 0 ? sep->len : 0);
    }
    BrimStr *s = str_alloc(total);
    int64_t at = 0;
    for (int64_t i = 0; i < n; i++) {
        if (i > 0) {
            memcpy(s->data + at, sep->data, (size_t)sep->len);
            at += sep->len;
        }
        memcpy(s->data + at, items[i]->data, (size_t)items[i]->len);
        at += items[i]->len;
    }
    return s;
}

/* ---- process entry ------------------------------------------------------- */

int main(void) {
    const char *check = getenv("BRIM_RT_CHECK");
    checking = check != NULL && strcmp(check, "1") == 0;
    frames = malloc(sizeof(const char *) * (size_t)brim_max_call_depth);
    if (frames == NULL) {
        fatal("out of memory");
    }
    brim_entry();
    if (fflush(stdout) != 0 || ferror(stdout)) {
        fprintf(stderr, "error[R0004]: failed to write output: %s\n", strerror(errno));
        return 101;
    }
    if (checking && live_strings != 0) {
        fprintf(stderr, "error: brim runtime check: %" PRId64 " string(s) or list(s) leaked\n",
                live_strings);
        return 102;
    }
    return 0;
}
