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

/* For `fileno` and `struct stat` with a strict C11 compiler. */
#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

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
/* Prints the call-stack notes and exits with status 101. `sep` separates
 * the excerpt from the notes; NULL if it was already printed. */
_Noreturn static void finish_trap(const char *sep, const char *note) {
    if (depth > 1) {
        if (sep != NULL) {
            fputs(sep, stderr);
        }
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

/* ---- maps ---------------------------------------------------------------- */

/*
 * A map keeps its entries in insertion order in one array, and finds keys
 * through a hash index (open addressing, linear probing) of entry positions.
 * Like lists, maps are values with copy-on-write and NULL is the empty map.
 *
 * Each entry is an 8-byte key slot followed by the value, padded to 8
 * bytes. Keys are `i64` (kind 0), `bool` (kind 1, one byte) or strings
 * (kind 2, a `BrimStr *`, retained by the map). Values are handled like list
 * elements, with generated retain/release functions.
 */
enum { KEY_I64 = 0, KEY_BOOL = 1, KEY_STR = 2 };

typedef struct BrimMap {
    int64_t rc;
    int64_t len;
    int64_t cap;
    int64_t key_kind;
    int64_t value_size;
    int64_t entry_size;
    BrimElemFn value_release;
    /* Entry position + 1 per slot; 0 is an empty slot. */
    int64_t *index;
    int64_t index_cap;
    char *entries;
} BrimMap;

static char *entry_at(const BrimMap *m, int64_t i) {
    return m->entries + i * m->entry_size;
}

static uint64_t key_hash(int64_t kind, const void *key) {
    uint64_t h = 1469598103934665603ULL;
    if (kind == KEY_STR) {
        const BrimStr *s = *(BrimStr *const *)key;
        for (int64_t i = 0; i < s->len; i++) {
            h = (h ^ (uint8_t)s->data[i]) * 1099511628211ULL;
        }
        return h;
    }
    uint64_t v = kind == KEY_BOOL ? (uint64_t)(*(const uint8_t *)key != 0)
                                  : (uint64_t)*(const int64_t *)key;
    v ^= v >> 33;
    v *= 0xff51afd7ed558ccdULL;
    v ^= v >> 33;
    return v;
}

static int key_eq(int64_t kind, const void *a, const void *b) {
    switch (kind) {
    case KEY_STR:
        return brim_rt_str_eq(*(BrimStr *const *)a, *(BrimStr *const *)b);
    case KEY_BOOL:
        return (*(const uint8_t *)a != 0) == (*(const uint8_t *)b != 0);
    default:
        return *(const int64_t *)a == *(const int64_t *)b;
    }
}

static void key_copy(int64_t kind, void *to, const void *from) {
    memset(to, 0, 8);
    memcpy(to, from, kind == KEY_BOOL ? 1 : 8);
    if (kind == KEY_STR) {
        brim_rt_retain(*(BrimStr **)to);
    }
}

static void rebuild_index(BrimMap *m) {
    int64_t cap = 8;
    while (cap < (m->len + 1) * 2) {
        cap *= 2;
    }
    free(m->index);
    m->index = calloc((size_t)cap, sizeof(int64_t));
    if (m->index == NULL) {
        fatal("out of memory");
    }
    m->index_cap = cap;
    for (int64_t i = 0; i < m->len; i++) {
        uint64_t slot = key_hash(m->key_kind, entry_at(m, i)) & (uint64_t)(cap - 1);
        while (m->index[slot] != 0) {
            slot = (slot + 1) & (uint64_t)(cap - 1);
        }
        m->index[slot] = i + 1;
    }
}

static BrimMap *map_alloc(int64_t key_kind, int64_t value_size, BrimElemFn release,
                          int64_t cap) {
    if (cap < 4) {
        cap = 4;
    }
    BrimMap *m = malloc(sizeof(BrimMap));
    if (m == NULL) {
        fatal("out of memory");
    }
    m->rc = 1;
    m->len = 0;
    m->cap = cap;
    m->key_kind = key_kind;
    m->value_size = value_size;
    m->entry_size = 8 + (value_size + 7) / 8 * 8;
    m->value_release = release;
    m->index = NULL;
    m->entries = malloc((size_t)(cap * m->entry_size));
    if (m->entries == NULL) {
        fatal("out of memory");
    }
    rebuild_index(m);
    if (checking) {
        live_strings++;
    }
    return m;
}

void brim_rt_map_retain(BrimMap *m) {
    if (m != NULL) {
        m->rc++;
    }
}

void brim_rt_map_release(BrimMap *m) {
    if (m == NULL || --m->rc > 0) {
        return;
    }
    for (int64_t i = 0; i < m->len; i++) {
        if (m->key_kind == KEY_STR) {
            brim_rt_release(*(BrimStr **)entry_at(m, i));
        }
        if (m->value_release != NULL) {
            m->value_release(entry_at(m, i) + 8);
        }
    }
    if (checking) {
        live_strings--;
    }
    free(m->index);
    free(m->entries);
    free(m);
}

int64_t brim_rt_map_len(const BrimMap *m) {
    return m == NULL ? 0 : m->len;
}

/* The position of `key`, or -1. */
int64_t brim_rt_map_find(const BrimMap *m, const void *key) {
    if (m == NULL) {
        return -1;
    }
    uint64_t mask = (uint64_t)(m->index_cap - 1);
    uint64_t slot = key_hash(m->key_kind, key) & mask;
    while (m->index[slot] != 0) {
        int64_t i = m->index[slot] - 1;
        if (key_eq(m->key_kind, entry_at(m, i), key)) {
            return i;
        }
        slot = (slot + 1) & mask;
    }
    return -1;
}

/* The position of `key`, which must be in the map. */
int64_t brim_rt_map_index(const BrimMap *m, const void *key, const char *head, const char *sep,
                          const char *note) {
    int64_t i = brim_rt_map_find(m, key);
    if (i < 0) {
        brim_rt_trap(head, sep, note);
    }
    return i;
}

void *brim_rt_map_key_at(const BrimMap *m, int64_t i) {
    return entry_at(m, i);
}

void *brim_rt_map_value_at(const BrimMap *m, int64_t i) {
    return entry_at(m, i) + 8;
}

/* Takes ownership of `m` and returns a map nothing else refers to. */
static BrimMap *map_unique(BrimMap *m, int64_t key_kind, int64_t value_size, BrimElemFn retain,
                           BrimElemFn release) {
    if (m == NULL) {
        return map_alloc(key_kind, value_size, release, 4);
    }
    if (m->rc == 1) {
        return m;
    }
    BrimMap *copy = map_alloc(key_kind, value_size, release, m->len + 1);
    memcpy(copy->entries, m->entries, (size_t)(m->len * m->entry_size));
    copy->len = m->len;
    for (int64_t i = 0; i < copy->len; i++) {
        if (key_kind == KEY_STR) {
            brim_rt_retain(*(BrimStr **)entry_at(copy, i));
        }
        if (retain != NULL) {
            retain(entry_at(copy, i) + 8);
        }
    }
    rebuild_index(copy);
    m->rc--;
    return copy;
}

BrimMap *brim_rt_map_insert(BrimMap *m, const void *key, const void *value, int64_t key_kind,
                            int64_t value_size, BrimElemFn retain, BrimElemFn release) {
    m = map_unique(m, key_kind, value_size, retain, release);
    if (retain != NULL) {
        retain((void *)value);
    }
    int64_t i = brim_rt_map_find(m, key);
    if (i >= 0) {
        if (release != NULL) {
            release(entry_at(m, i) + 8);
        }
        memcpy(entry_at(m, i) + 8, value, (size_t)value_size);
        return m;
    }
    if (m->len == m->cap) {
        m->cap *= 2;
        char *bigger = realloc(m->entries, (size_t)(m->cap * m->entry_size));
        if (bigger == NULL) {
            fatal("out of memory");
        }
        m->entries = bigger;
    }
    char *entry = entry_at(m, m->len);
    key_copy(key_kind, entry, key);
    memcpy(entry + 8, value, (size_t)value_size);
    m->len++;
    if (m->len * 2 > m->index_cap) {
        rebuild_index(m);
    } else {
        uint64_t mask = (uint64_t)(m->index_cap - 1);
        uint64_t slot = key_hash(key_kind, entry) & mask;
        while (m->index[slot] != 0) {
            slot = (slot + 1) & mask;
        }
        m->index[slot] = m->len;
    }
    return m;
}

BrimMap *brim_rt_map_set_value_at(BrimMap *m, int64_t i, const void *value, int64_t key_kind,
                                  int64_t value_size, BrimElemFn retain, BrimElemFn release) {
    m = map_unique(m, key_kind, value_size, retain, release);
    if (retain != NULL) {
        retain((void *)value);
    }
    if (release != NULL) {
        release(entry_at(m, i) + 8);
    }
    memcpy(entry_at(m, i) + 8, value, (size_t)value_size);
    return m;
}

BrimMap *brim_rt_map_remove_at(BrimMap *m, int64_t i, int64_t key_kind, int64_t value_size,
                               BrimElemFn retain, BrimElemFn release) {
    m = map_unique(m, key_kind, value_size, retain, release);
    if (key_kind == KEY_STR) {
        brim_rt_release(*(BrimStr **)entry_at(m, i));
    }
    if (release != NULL) {
        release(entry_at(m, i) + 8);
    }
    memmove(entry_at(m, i), entry_at(m, i + 1), (size_t)((m->len - i - 1) * m->entry_size));
    m->len--;
    rebuild_index(m);
    return m;
}

/* ---- assertions ----------------------------------------------------------- */

void brim_rt_assert(uint8_t ok, const char *head, const char *sep, const char *note) {
    if (!ok) {
        brim_rt_trap(head, sep, note);
    }
}

/* Reports `assert_eq` with both values, which `print_value` writes. */
_Noreturn static void assert_eq_failed(const char *head, const char *sep, const char *note,
                                       void (*print_value)(const void *), const void *left,
                                       const void *right) {
    fflush(stdout);
    fputs(head, stderr);
    fputs(sep, stderr);
    fprintf(stderr, "%sleft:  ", note);
    print_value(left);
    fprintf(stderr, "\n%sright: ", note);
    print_value(right);
    fputc('\n', stderr);
    finish_trap(NULL, note);
}

static void print_i64(const void *v) {
    fprintf(stderr, "%" PRId64, *(const int64_t *)v);
}

static void print_bool(const void *v) {
    fputs(*(const uint8_t *)v ? "true" : "false", stderr);
}

static void print_str(const void *v) {
    const BrimStr *s = *(BrimStr *const *)v;
    fputc('"', stderr);
    fwrite(s->data, 1, (size_t)s->len, stderr);
    fputc('"', stderr);
}

void brim_rt_assert_eq_i64(int64_t a, int64_t b, const char *head, const char *sep,
                           const char *note) {
    if (a != b) {
        assert_eq_failed(head, sep, note, print_i64, &a, &b);
    }
}

void brim_rt_assert_eq_bool(uint8_t a, uint8_t b, const char *head, const char *sep,
                            const char *note) {
    if ((a != 0) != (b != 0)) {
        assert_eq_failed(head, sep, note, print_bool, &a, &b);
    }
}

void brim_rt_assert_eq_str(const BrimStr *a, const BrimStr *b, const char *head,
                           const char *sep, const char *note) {
    if (!brim_rt_str_eq(a, b)) {
        assert_eq_failed(head, sep, note, print_str, &a, &b);
    }
}

/* ---- the standard library ------------------------------------------------- */

/*
 * These functions must behave exactly like their counterparts in the
 * `brim-runtime` crate, which the interpreter uses, including the text of
 * error messages.
 */

static BrimStr EMPTY_STR = {-1, 0};

static BrimStr *str_from(const char *data, int64_t len) {
    if (len == 0) {
        return &EMPTY_STR;
    }
    BrimStr *s = str_alloc(len);
    memcpy(s->data, data, (size_t)len);
    return s;
}

static BrimStr *str_from_c(const char *text) {
    return str_from(text, (int64_t)strlen(text));
}

/* A NUL-terminated copy of a string, for C library calls. */
static char *c_string(const BrimStr *s) {
    char *c = malloc((size_t)s->len + 1);
    if (c == NULL) {
        fatal("out of memory");
    }
    memcpy(c, s->data, (size_t)s->len);
    c[s->len] = 0;
    return c;
}

void brim_rt_eprint_str(const BrimStr *s) {
    fflush(stdout);
    fwrite(s->data, 1, (size_t)s->len, stderr);
    fputc('\n', stderr);
}

int64_t brim_rt_str_compare(const BrimStr *a, const BrimStr *b) {
    int64_t n = a->len < b->len ? a->len : b->len;
    int c = memcmp(a->data, b->data, (size_t)n);
    if (c != 0) {
        return c < 0 ? -1 : 1;
    }
    return a->len < b->len ? -1 : (a->len > b->len ? 1 : 0);
}

int64_t brim_rt_str_len(const BrimStr *s) {
    return s->len;
}

/* The first position of `pattern` in `s` at or after `from`, or -1. */
static int64_t find_from(const BrimStr *s, const BrimStr *pattern, int64_t from) {
    for (int64_t i = from; i + pattern->len <= s->len; i++) {
        if (memcmp(s->data + i, pattern->data, (size_t)pattern->len) == 0) {
            return i;
        }
    }
    return -1;
}

int64_t brim_rt_str_find(const BrimStr *s, const BrimStr *pattern) {
    return find_from(s, pattern, 0);
}

uint8_t brim_rt_str_contains(const BrimStr *s, const BrimStr *pattern) {
    return find_from(s, pattern, 0) >= 0;
}

uint8_t brim_rt_str_starts_with(const BrimStr *s, const BrimStr *prefix) {
    return prefix->len <= s->len && memcmp(s->data, prefix->data, (size_t)prefix->len) == 0;
}

uint8_t brim_rt_str_ends_with(const BrimStr *s, const BrimStr *suffix) {
    return suffix->len <= s->len &&
           memcmp(s->data + s->len - suffix->len, suffix->data, (size_t)suffix->len) == 0;
}

/* Whether byte `i` starts a character (or is the end of the string). */
static int char_boundary(const BrimStr *s, int64_t i) {
    return i == s->len || ((uint8_t)s->data[i] & 0xC0) != 0x80;
}

BrimStr *brim_rt_str_slice(const BrimStr *s, int64_t start, int64_t end, const char *head,
                           const char *sep, const char *note) {
    if (start < 0 || end < start || end > s->len || !char_boundary(s, start) ||
        !char_boundary(s, end)) {
        brim_rt_trap(head, sep, note);
    }
    return str_from(s->data + start, end - start);
}

static int is_space(char c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\v' || c == '\f';
}

BrimStr *brim_rt_str_trim_start(const BrimStr *s) {
    int64_t i = 0;
    while (i < s->len && is_space(s->data[i])) {
        i++;
    }
    return str_from(s->data + i, s->len - i);
}

BrimStr *brim_rt_str_trim_end(const BrimStr *s) {
    int64_t n = s->len;
    while (n > 0 && is_space(s->data[n - 1])) {
        n--;
    }
    return str_from(s->data, n);
}

BrimStr *brim_rt_str_trim(const BrimStr *s) {
    int64_t i = 0;
    int64_t n = s->len;
    while (i < n && is_space(s->data[i])) {
        i++;
    }
    while (n > i && is_space(s->data[n - 1])) {
        n--;
    }
    return str_from(s->data + i, n - i);
}

static BrimStr *map_ascii(const BrimStr *s, int upper) {
    BrimStr *out = str_alloc(s->len);
    for (int64_t i = 0; i < s->len; i++) {
        char c = s->data[i];
        if (upper && c >= 'a' && c <= 'z') {
            c = (char)(c - 'a' + 'A');
        } else if (!upper && c >= 'A' && c <= 'Z') {
            c = (char)(c - 'A' + 'a');
        }
        out->data[i] = c;
    }
    return out;
}

BrimStr *brim_rt_str_to_upper(const BrimStr *s) {
    return map_ascii(s, 1);
}

BrimStr *brim_rt_str_to_lower(const BrimStr *s) {
    return map_ascii(s, 0);
}

/* A growable byte buffer for building strings. */
typedef struct Buffer {
    char *data;
    int64_t len;
    int64_t cap;
} Buffer;

static void buffer_add(Buffer *b, const char *data, int64_t len) {
    if (b->len + len > b->cap) {
        int64_t cap = b->cap < 16 ? 16 : b->cap;
        while (cap < b->len + len) {
            cap *= 2;
        }
        char *bigger = realloc(b->data, (size_t)cap);
        if (bigger == NULL) {
            fatal("out of memory");
        }
        b->data = bigger;
        b->cap = cap;
    }
    memcpy(b->data + b->len, data, (size_t)len);
    b->len += len;
}

static BrimStr *buffer_finish(Buffer *b) {
    BrimStr *s = str_from(b->data, b->len);
    free(b->data);
    return s;
}

/* The length of the UTF-8 character starting at byte `i`. */
static int64_t char_len(const BrimStr *s, int64_t i) {
    int64_t n = 1;
    while (i + n < s->len && ((uint8_t)s->data[i + n] & 0xC0) == 0x80) {
        n++;
    }
    return n;
}

BrimStr *brim_rt_str_replace(const BrimStr *s, const BrimStr *from, const BrimStr *to) {
    Buffer b = {0};
    if (from->len == 0) {
        /* Before every character, and at the end. */
        for (int64_t i = 0; i < s->len;) {
            int64_t n = char_len(s, i);
            buffer_add(&b, to->data, to->len);
            buffer_add(&b, s->data + i, n);
            i += n;
        }
        buffer_add(&b, to->data, to->len);
        return buffer_finish(&b);
    }
    int64_t i = 0;
    for (;;) {
        int64_t at = find_from(s, from, i);
        if (at < 0) {
            break;
        }
        buffer_add(&b, s->data + i, at - i);
        buffer_add(&b, to->data, to->len);
        i = at + from->len;
    }
    buffer_add(&b, s->data + i, s->len - i);
    return buffer_finish(&b);
}

BrimStr *brim_rt_str_repeat(const BrimStr *s, int64_t n) {
    Buffer b = {0};
    for (int64_t i = 0; i < n; i++) {
        buffer_add(&b, s->data, s->len);
    }
    return buffer_finish(&b);
}

static void str_elem_retain(void *p) {
    brim_rt_retain(*(BrimStr **)p);
}

static void str_elem_release(void *p) {
    brim_rt_release(*(BrimStr **)p);
}

/* Appends a new string (taking over the caller's reference). */
static BrimList *push_str(BrimList *l, BrimStr *s) {
    l = brim_rt_list_push(l, &s, (int64_t)sizeof s, str_elem_retain, str_elem_release);
    brim_rt_release(s);
    return l;
}

BrimList *brim_rt_str_chars(const BrimStr *s) {
    BrimList *l = NULL;
    for (int64_t i = 0; i < s->len;) {
        int64_t n = char_len(s, i);
        l = push_str(l, str_from(s->data + i, n));
        i += n;
    }
    return l;
}

BrimList *brim_rt_str_split(const BrimStr *s, const BrimStr *sep) {
    if (sep->len == 0) {
        return brim_rt_str_chars(s);
    }
    BrimList *l = NULL;
    int64_t i = 0;
    for (;;) {
        int64_t at = find_from(s, sep, i);
        if (at < 0) {
            break;
        }
        l = push_str(l, str_from(s->data + i, at - i));
        i = at + sep->len;
    }
    return push_str(l, str_from(s->data + i, s->len - i));
}

BrimList *brim_rt_str_lines(const BrimStr *s) {
    BrimList *l = NULL;
    int64_t start = 0;
    for (int64_t i = 0; i < s->len; i++) {
        if (s->data[i] == '\n') {
            int64_t end = i;
            if (end > start && s->data[end - 1] == '\r') {
                end--;
            }
            l = push_str(l, str_from(s->data + start, end - start));
            start = i + 1;
        }
    }
    if (start < s->len) {
        l = push_str(l, str_from(s->data + start, s->len - start));
    }
    return l;
}

/* Parses an optionally signed decimal `i64`; returns whether `s` is one. */
static int parse_int(const BrimStr *s, int64_t *out) {
    int64_t i = 0;
    int negative = 0;
    if (i < s->len && (s->data[i] == '+' || s->data[i] == '-')) {
        negative = s->data[i] == '-';
        i++;
    }
    if (i == s->len) {
        return 0;
    }
    /* Accumulate as a negative number, which reaches i64::MIN. */
    int64_t value = 0;
    for (; i < s->len; i++) {
        char c = s->data[i];
        if (c < '0' || c > '9') {
            return 0;
        }
        int64_t digit = c - '0';
        if (value < (INT64_MIN + digit) / 10) {
            return 0;
        }
        value = value * 10 - digit;
    }
    if (!negative) {
        if (value == INT64_MIN) {
            return 0;
        }
        value = -value;
    }
    *out = value;
    return 1;
}

uint8_t brim_rt_str_parse_int_ok(const BrimStr *s) {
    int64_t value;
    return (uint8_t)parse_int(s, &value);
}

int64_t brim_rt_str_parse_int(const BrimStr *s) {
    int64_t value = 0;
    parse_int(s, &value);
    return value;
}

/* ---- input, files and the environment ------------------------------------ */

static BrimStr *last_error;
static int program_argc;
static char **program_argv;

static void set_error(BrimStr *message) {
    brim_rt_release(last_error);
    last_error = message;
}

BrimStr *brim_rt_last_error(void) {
    if (last_error == NULL) {
        return &EMPTY_STR;
    }
    brim_rt_retain(last_error);
    return last_error;
}

static int valid_utf8(const char *data, int64_t len) {
    int64_t i = 0;
    while (i < len) {
        uint8_t c = (uint8_t)data[i];
        int64_t n;
        uint32_t min;
        uint32_t cp;
        if (c < 0x80) {
            i++;
            continue;
        } else if ((c & 0xE0) == 0xC0) {
            n = 2;
            min = 0x80;
            cp = c & 0x1F;
        } else if ((c & 0xF0) == 0xE0) {
            n = 3;
            min = 0x800;
            cp = c & 0x0F;
        } else if ((c & 0xF8) == 0xF0) {
            n = 4;
            min = 0x10000;
            cp = c & 0x07;
        } else {
            return 0;
        }
        if (i + n > len) {
            return 0;
        }
        for (int64_t k = 1; k < n; k++) {
            uint8_t cont = (uint8_t)data[i + k];
            if ((cont & 0xC0) != 0x80) {
                return 0;
            }
            cp = (cp << 6) | (cont & 0x3F);
        }
        if (cp < min || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)) {
            return 0;
        }
        i += n;
    }
    return 1;
}

BrimStr *brim_rt_read_line(void) {
    fflush(stdout);
    set_error(NULL);
    Buffer b = {0};
    int c;
    int any = 0;
    while ((c = fgetc(stdin)) != EOF) {
        any = 1;
        if (c == '\n') {
            if (b.len > 0 && b.data[b.len - 1] == '\r') {
                b.len--;
            }
            break;
        }
        char ch = (char)c;
        buffer_add(&b, &ch, 1);
    }
    if (!any) {
        free(b.data);
        set_error(str_from_c("end of input"));
        return &EMPTY_STR;
    }
    if (!valid_utf8(b.data, b.len)) {
        free(b.data);
        set_error(str_from_c("input is not valid UTF-8"));
        return &EMPTY_STR;
    }
    return buffer_finish(&b);
}

static void file_error(const char *verb, const BrimStr *path, const char *reason) {
    Buffer b = {0};
    buffer_add(&b, "cannot ", 7);
    buffer_add(&b, verb, (int64_t)strlen(verb));
    buffer_add(&b, " file \"", 7);
    buffer_add(&b, path->data, path->len);
    buffer_add(&b, "\": ", 3);
    buffer_add(&b, reason, (int64_t)strlen(reason));
    set_error(buffer_finish(&b));
}

static const char *errno_reason(int error) {
    switch (error) {
    case ENOENT:
        return "file not found";
    case EACCES:
        return "permission denied";
    case EISDIR:
        return "is a directory";
    default:
        return "input/output error";
    }
}

BrimStr *brim_rt_read_file(const BrimStr *path) {
    set_error(NULL);
    char *name = c_string(path);
    FILE *f = fopen(name, "rb");
    struct stat info;
    if (f != NULL && fstat(fileno(f), &info) == 0 && S_ISDIR(info.st_mode)) {
        fclose(f);
        f = NULL;
        errno = EISDIR;
    }
    free(name);
    if (f == NULL) {
        file_error("read", path, errno_reason(errno));
        return &EMPTY_STR;
    }
    Buffer b = {0};
    char chunk[65536];
    size_t n;
    while ((n = fread(chunk, 1, sizeof chunk, f)) > 0) {
        buffer_add(&b, chunk, (int64_t)n);
    }
    int failed = ferror(f);
    fclose(f);
    if (failed) {
        free(b.data);
        file_error("read", path, "input/output error");
        return &EMPTY_STR;
    }
    if (!valid_utf8(b.data, b.len)) {
        free(b.data);
        file_error("read", path, "file is not valid UTF-8");
        return &EMPTY_STR;
    }
    return buffer_finish(&b);
}

int64_t brim_rt_write_file(const BrimStr *path, const BrimStr *text) {
    set_error(NULL);
    char *name = c_string(path);
    FILE *f = fopen(name, "wb");
    free(name);
    if (f == NULL) {
        file_error("write", path, errno_reason(errno));
        return 0;
    }
    size_t written = fwrite(text->data, 1, (size_t)text->len, f);
    int failed = fclose(f) != 0 || written != (size_t)text->len;
    if (failed) {
        file_error("write", path, "input/output error");
        return 0;
    }
    return text->len;
}

BrimList *brim_rt_args(void) {
    BrimList *l = NULL;
    for (int i = 1; i < program_argc; i++) {
        l = push_str(l, str_from_c(program_argv[i]));
    }
    return l;
}

uint8_t brim_rt_env_has(const BrimStr *name) {
    char *c = c_string(name);
    uint8_t set = getenv(c) != NULL;
    free(c);
    return set;
}

BrimStr *brim_rt_env_get(const BrimStr *name) {
    char *c = c_string(name);
    const char *value = getenv(c);
    free(c);
    return value == NULL ? &EMPTY_STR : str_from_c(value);
}

_Noreturn void brim_rt_exit(int64_t code) {
    fflush(stdout);
    exit((int)(code & 0xff));
}

/* ---- sockets (std.net) ---------------------------------------------------- */
/* The same functions and error reasons as brim-runtime's net.rs, which the
 * interpreter uses. Handles start at 1 and are never reused. */

typedef struct Socket {
    int fd;          /* -1 once closed */
    int listening;
    char *buf;       /* bytes read but not yet returned */
    int64_t len;
    int64_t cap;
} Socket;

static Socket *sockets;
static int64_t socket_count;

static const char *net_reason(int error) {
    switch (error) {
    case ECONNREFUSED: return "connection refused";
    case EADDRINUSE: return "address in use";
    case EADDRNOTAVAIL: return "address not available";
    case EACCES:
    case EPERM: return "permission denied";
    case ETIMEDOUT:
    case EAGAIN:
#if EWOULDBLOCK != EAGAIN
    case EWOULDBLOCK:
#endif
        return "timed out";
    case ECONNRESET:
    case ECONNABORTED:
    case EPIPE: return "connection reset";
    case ENOTCONN: return "not connected";
    default: return "network error";
    }
}

static void net_fail(const char *reason) {
    set_error(str_from_c(reason));
}

/* Prepares a network call: flushes output, clears the last error, and
 * makes writes to a closed connection an error instead of a signal. */
static void net_start(void) {
    static int ready;
    if (!ready) {
        signal(SIGPIPE, SIG_IGN);
        ready = 1;
    }
    fflush(stdout);
    set_error(NULL);
}

static int64_t socket_add(int fd, int listening) {
    Socket *grown = realloc(sockets, sizeof(Socket) * (size_t)(socket_count + 1));
    if (grown == NULL) {
        fatal("out of memory");
    }
    sockets = grown;
    sockets[socket_count] = (Socket){fd, listening, NULL, 0, 0};
    socket_count++;
    return socket_count;
}

static Socket *socket_get(int64_t handle, int listening) {
    if (handle < 1 || handle > socket_count) {
        return NULL;
    }
    Socket *s = &sockets[handle - 1];
    if (s->fd < 0 || s->listening != listening) {
        return NULL;
    }
    return s;
}

static int port_ok(int64_t port) {
    return port >= 0 && port <= 65535;
}

static struct addrinfo *resolve(const BrimStr *host, int64_t port, int passive) {
    char *name = c_string(host);
    char service[16];
    snprintf(service, sizeof service, "%" PRId64, port);
    struct addrinfo hints;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = passive ? AI_PASSIVE : 0;
    struct addrinfo *found = NULL;
    int failed = getaddrinfo(name, service, &hints, &found);
    free(name);
    return failed == 0 ? found : NULL;
}

static void no_delay(int fd) {
#ifdef TCP_NODELAY
    int on = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &on, sizeof on);
#else
    (void)fd;
#endif
}

int64_t brim_rt_net_listen(const BrimStr *host, int64_t port) {
    net_start();
    if (!port_ok(port)) {
        net_fail("invalid port");
        return 0;
    }
    struct addrinfo *found = resolve(host, port, 1);
    if (found == NULL) {
        net_fail("host not found");
        return 0;
    }
    int error = EADDRNOTAVAIL;
    for (struct addrinfo *a = found; a != NULL; a = a->ai_next) {
        int fd = socket(a->ai_family, a->ai_socktype, a->ai_protocol);
        if (fd < 0) {
            error = errno;
            continue;
        }
        int on = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on);
        if (bind(fd, a->ai_addr, a->ai_addrlen) == 0 && listen(fd, 128) == 0) {
            freeaddrinfo(found);
            return socket_add(fd, 1);
        }
        error = errno;
        close(fd);
    }
    freeaddrinfo(found);
    net_fail(net_reason(error));
    return 0;
}

int64_t brim_rt_net_accept(int64_t listener) {
    net_start();
    Socket *l = socket_get(listener, 1);
    if (l == NULL) {
        net_fail("invalid socket");
        return 0;
    }
    int fd;
    do {
        fd = accept(l->fd, NULL, NULL);
    } while (fd < 0 && errno == EINTR);
    if (fd < 0) {
        net_fail(net_reason(errno));
        return 0;
    }
    no_delay(fd);
    return socket_add(fd, 0);
}

int64_t brim_rt_net_connect(const BrimStr *host, int64_t port) {
    net_start();
    if (!port_ok(port)) {
        net_fail("invalid port");
        return 0;
    }
    struct addrinfo *found = resolve(host, port, 0);
    if (found == NULL) {
        net_fail("host not found");
        return 0;
    }
    int error = ECONNREFUSED;
    for (struct addrinfo *a = found; a != NULL; a = a->ai_next) {
        int fd = socket(a->ai_family, a->ai_socktype, a->ai_protocol);
        if (fd < 0) {
            error = errno;
            continue;
        }
        if (connect(fd, a->ai_addr, a->ai_addrlen) == 0) {
            freeaddrinfo(found);
            no_delay(fd);
            return socket_add(fd, 0);
        }
        error = errno;
        close(fd);
    }
    freeaddrinfo(found);
    net_fail(net_reason(error));
    return 0;
}

/* Reads more bytes into the buffer: the count, 0 at the end, -1 on error
 * (with the last error set). */
static int64_t socket_fill(Socket *s) {
    if (s->cap - s->len < 8192) {
        int64_t cap = s->cap * 2 + 8192;
        char *grown = realloc(s->buf, (size_t)cap);
        if (grown == NULL) {
            fatal("out of memory");
        }
        s->buf = grown;
        s->cap = cap;
    }
    ssize_t n;
    do {
        n = recv(s->fd, s->buf + s->len, 8192, 0);
    } while (n < 0 && errno == EINTR);
    if (n < 0) {
        net_fail(net_reason(errno));
        return -1;
    }
    s->len += n;
    return n;
}

/* The first `count` buffered bytes as a string, removed from the buffer. */
static BrimStr *socket_take(Socket *s, int64_t count, int64_t drop) {
    if (!valid_utf8(s->buf, count)) {
        memmove(s->buf, s->buf + count + drop, (size_t)(s->len - count - drop));
        s->len -= count + drop;
        net_fail("data is not valid UTF-8");
        return &EMPTY_STR;
    }
    BrimStr *text = str_from(s->buf, count);
    memmove(s->buf, s->buf + count + drop, (size_t)(s->len - count - drop));
    s->len -= count + drop;
    return text;
}

#define CLOSED_EARLY "connection closed before the end of the message"

BrimStr *brim_rt_net_read_until(int64_t handle, const BrimStr *delimiter) {
    net_start();
    Socket *s = socket_get(handle, 0);
    if (s == NULL) {
        net_fail("invalid socket");
        return &EMPTY_STR;
    }
    int64_t d = delimiter->len;
    if (d == 0) {
        net_fail("empty delimiter");
        return &EMPTY_STR;
    }
    int64_t searched = 0;
    for (;;) {
        for (int64_t i = searched; i + d <= s->len; i++) {
            if (memcmp(s->buf + i, delimiter->data, (size_t)d) == 0) {
                return socket_take(s, i, d);
            }
        }
        searched = s->len - d + 1 > 0 ? s->len - d + 1 : 0;
        int64_t n = socket_fill(s);
        if (n < 0) {
            return &EMPTY_STR;
        }
        if (n == 0) {
            net_fail(s->len == 0 ? "connection closed" : CLOSED_EARLY);
            return &EMPTY_STR;
        }
    }
}

BrimStr *brim_rt_net_read_exact(int64_t handle, int64_t count) {
    net_start();
    Socket *s = socket_get(handle, 0);
    if (s == NULL) {
        net_fail("invalid socket");
        return &EMPTY_STR;
    }
    if (count < 0) {
        net_fail("invalid length");
        return &EMPTY_STR;
    }
    while (s->len < count) {
        int64_t n = socket_fill(s);
        if (n < 0) {
            return &EMPTY_STR;
        }
        if (n == 0) {
            net_fail(CLOSED_EARLY);
            return &EMPTY_STR;
        }
    }
    return socket_take(s, count, 0);
}

BrimStr *brim_rt_net_read_all(int64_t handle) {
    net_start();
    Socket *s = socket_get(handle, 0);
    if (s == NULL) {
        net_fail("invalid socket");
        return &EMPTY_STR;
    }
    for (;;) {
        int64_t n = socket_fill(s);
        if (n < 0) {
            return &EMPTY_STR;
        }
        if (n == 0) {
            break;
        }
    }
    return socket_take(s, s->len, 0);
}

int64_t brim_rt_net_write(int64_t handle, const BrimStr *text) {
    net_start();
    Socket *s = socket_get(handle, 0);
    if (s == NULL) {
        net_fail("invalid socket");
        return 0;
    }
    int64_t sent = 0;
    while (sent < text->len) {
        ssize_t n = send(s->fd, text->data + sent, (size_t)(text->len - sent), 0);
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            net_fail(net_reason(errno));
            return 0;
        }
        sent += n;
    }
    return text->len;
}

void brim_rt_net_close(int64_t handle) {
    net_start();
    if (handle < 1 || handle > socket_count) {
        return;
    }
    Socket *s = &sockets[handle - 1];
    if (s->fd >= 0) {
        if (!s->listening) {
            shutdown(s->fd, SHUT_RDWR);
        }
        close(s->fd);
        s->fd = -1;
        free(s->buf);
        s->buf = NULL;
        s->len = s->cap = 0;
    }
}

int64_t brim_rt_net_local_port(int64_t handle) {
    net_start();
    Socket *s = NULL;
    if (handle >= 1 && handle <= socket_count && sockets[handle - 1].fd >= 0) {
        s = &sockets[handle - 1];
    }
    if (s == NULL) {
        net_fail("invalid socket");
        return 0;
    }
    struct sockaddr_storage addr;
    socklen_t size = sizeof addr;
    if (getsockname(s->fd, (struct sockaddr *)&addr, &size) != 0) {
        net_fail(net_reason(errno));
        return 0;
    }
    if (addr.ss_family == AF_INET6) {
        return ntohs(((struct sockaddr_in6 *)&addr)->sin6_port);
    }
    return ntohs(((struct sockaddr_in *)&addr)->sin_port);
}

void brim_rt_net_set_timeout(int64_t handle, int64_t millis) {
    net_start();
    Socket *s = socket_get(handle, 0);
    if (s == NULL) {
        net_fail("invalid socket");
        return;
    }
    if (millis < 0) {
        net_fail("invalid timeout");
        return;
    }
    struct timeval tv;
    tv.tv_sec = (time_t)(millis / 1000);
    tv.tv_usec = (suseconds_t)((millis % 1000) * 1000);
    if (setsockopt(s->fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv) != 0 ||
        setsockopt(s->fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv) != 0) {
        net_fail(net_reason(errno));
    }
}

/* ---- URLs (std.http) ------------------------------------------------------ */

static int hex_value(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

BrimStr *brim_rt_url_decode(const BrimStr *text, uint8_t plus) {
    set_error(NULL);
    Buffer b = {0};
    for (int64_t i = 0; i < text->len; i++) {
        char c = text->data[i];
        if (c == '%') {
            int hi = i + 2 < text->len ? hex_value(text->data[i + 1]) : -1;
            int lo = i + 2 < text->len ? hex_value(text->data[i + 2]) : -1;
            if (hi < 0 || lo < 0) {
                free(b.data);
                set_error(str_from_c("invalid percent escape"));
                return &EMPTY_STR;
            }
            char byte = (char)(hi * 16 + lo);
            buffer_add(&b, &byte, 1);
            i += 2;
        } else if (c == '+' && plus) {
            buffer_add(&b, " ", 1);
        } else {
            buffer_add(&b, &c, 1);
        }
    }
    if (!valid_utf8(b.data, b.len)) {
        free(b.data);
        set_error(str_from_c("decoded text is not valid UTF-8"));
        return &EMPTY_STR;
    }
    return buffer_finish(&b);
}

BrimStr *brim_rt_url_encode(const BrimStr *text) {
    static const char digits[] = "0123456789ABCDEF";
    Buffer b = {0};
    for (int64_t i = 0; i < text->len; i++) {
        unsigned char c = (unsigned char)text->data[i];
        if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') ||
            c == '-' || c == '.' || c == '_' || c == '~') {
            char ch = (char)c;
            buffer_add(&b, &ch, 1);
        } else {
            char escape[3] = {'%', digits[c >> 4], digits[c & 15]};
            buffer_add(&b, escape, 3);
        }
    }
    return buffer_finish(&b);
}

/* ---- characters (std.json) ------------------------------------------------- */

BrimStr *brim_rt_char_from_code(int64_t code) {
    set_error(NULL);
    if (code < 0 || code > 0x10FFFF || (code >= 0xD800 && code <= 0xDFFF)) {
        set_error(str_from_c("not a Unicode character"));
        return &EMPTY_STR;
    }
    char out[4];
    int64_t n;
    if (code < 0x80) {
        out[0] = (char)code;
        n = 1;
    } else if (code < 0x800) {
        out[0] = (char)(0xC0 | (code >> 6));
        out[1] = (char)(0x80 | (code & 0x3F));
        n = 2;
    } else if (code < 0x10000) {
        out[0] = (char)(0xE0 | (code >> 12));
        out[1] = (char)(0x80 | ((code >> 6) & 0x3F));
        out[2] = (char)(0x80 | (code & 0x3F));
        n = 3;
    } else {
        out[0] = (char)(0xF0 | (code >> 18));
        out[1] = (char)(0x80 | ((code >> 12) & 0x3F));
        out[2] = (char)(0x80 | ((code >> 6) & 0x3F));
        out[3] = (char)(0x80 | (code & 0x3F));
        n = 4;
    }
    return str_from(out, n);
}

int64_t brim_rt_char_code(const BrimStr *s) {
    if (s->len == 0) {
        return -1;
    }
    const unsigned char *p = (const unsigned char *)s->data;
    if (p[0] < 0x80) {
        return p[0];
    }
    if (p[0] < 0xE0) {
        return ((int64_t)(p[0] & 0x1F) << 6) | (p[1] & 0x3F);
    }
    if (p[0] < 0xF0) {
        return ((int64_t)(p[0] & 0x0F) << 12) | ((int64_t)(p[1] & 0x3F) << 6) | (p[2] & 0x3F);
    }
    return ((int64_t)(p[0] & 0x07) << 18) | ((int64_t)(p[1] & 0x3F) << 12) |
           ((int64_t)(p[2] & 0x3F) << 6) | (p[3] & 0x3F);
}

/* ---- helpers for brim_rt_https.c --------------------------------------------- */

void brim_rt_internal_set_error(const char *reason) {
    set_error(reason == NULL ? NULL : str_from_c(reason));
}

BrimStr *brim_rt_internal_str(const char *data, int64_t len) {
    return str_from(data, len);
}

int brim_rt_internal_valid_utf8(const char *data, int64_t len) {
    return valid_utf8(data, len);
}

char *brim_rt_internal_c_string(const BrimStr *s) {
    return c_string(s);
}

/* ---- process entry ------------------------------------------------------- */

int main(int argc, char **argv) {
    program_argc = argc;
    program_argv = argv;
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
    set_error(NULL);
    if (checking && live_strings != 0) {
        fprintf(stderr, "error: brim runtime check: %" PRId64 " string(s) or list(s) leaked\n",
                live_strings);
        return 102;
    }
    return 0;
}
