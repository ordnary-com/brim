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

#ifdef _WIN32
/* Windows: Winsock and Win32 in place of the POSIX calls; see the
 * sections marked `_WIN32` below. */
#define WIN32_LEAN_AND_MEAN
#define _CRT_SECURE_NO_WARNINGS
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0600 /* Vista: WSAPoll */
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <shellapi.h>
#include <bcrypt.h>
#include <direct.h>
#include <io.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#else
/* For `fileno` and `struct stat` with a strict C11 compiler. */
#define _POSIX_C_SOURCE 200809L

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#endif

/* ---- platform differences ------------------------------------------------ */

#ifdef _WIN32
#ifndef __MINGW32__
typedef intptr_t ssize_t;
#endif
#ifndef S_ISDIR
#define S_ISDIR(m) (((m) & _S_IFMT) == _S_IFDIR)
#endif
/* 64-bit sizes and times. */
#define stat _stat64
#define fstat _fstat64
#define lstat _stat64
#define fileno _fileno
#define rmdir _rmdir
#define unlink _unlink
#define SHUT_RDWR SD_BOTH
#define poll WSAPoll
typedef ULONG brim_nfds_t;
#define sock_close closesocket
#define make_dir(name) _mkdir(name)

/* The POSIX error number for the last Winsock error. */
static int sock_errno(void) {
    switch (WSAGetLastError()) {
    case WSAECONNREFUSED: return ECONNREFUSED;
    case WSAEADDRINUSE: return EADDRINUSE;
    case WSAEADDRNOTAVAIL: return EADDRNOTAVAIL;
    case WSAEACCES: return EACCES;
    case WSAETIMEDOUT: return ETIMEDOUT;
    case WSAEWOULDBLOCK: return EWOULDBLOCK;
    case WSAECONNRESET: return ECONNRESET;
    case WSAECONNABORTED: return ECONNABORTED;
    case WSAENOTCONN: return ENOTCONN;
    case WSAEINTR: return EINTR;
    default: return EIO;
    }
}

/* The POSIX error number for the last Win32 error. */
static int win_errno(void) {
    switch (GetLastError()) {
    case ERROR_FILE_NOT_FOUND:
    case ERROR_PATH_NOT_FOUND:
    case ERROR_INVALID_NAME: return ENOENT;
    case ERROR_ACCESS_DENIED:
    case ERROR_SHARING_VIOLATION: return EACCES;
    case ERROR_DIRECTORY: return ENOTDIR;
    case ERROR_DIR_NOT_EMPTY: return ENOTEMPTY;
    case ERROR_ALREADY_EXISTS:
    case ERROR_FILE_EXISTS: return EEXIST;
    default: return EIO;
    }
}
#else
typedef nfds_t brim_nfds_t;
#define sock_close close
#define make_dir(name) mkdir(name, 0777)
static int sock_errno(void) {
    return errno;
}
#endif

/* Directory listing: the names in a directory, one at a time (with `.` and
 * `..`). `dir_open` returns NULL and sets `errno` on failure. */
typedef struct DirIter DirIter;
#ifdef _WIN32
struct DirIter {
    HANDLE find;
    WIN32_FIND_DATAA data;
    int first;
};

static DirIter *dir_open(const char *name) {
    size_t n = strlen(name);
    char *pattern = malloc(n + 3);
    if (pattern == NULL) {
        return NULL;
    }
    memcpy(pattern, name, n);
    memcpy(pattern + n, "\\*", 3);
    DirIter *it = malloc(sizeof *it);
    if (it == NULL) {
        free(pattern);
        return NULL;
    }
    DWORD attributes = GetFileAttributesA(name);
    it->find = INVALID_HANDLE_VALUE;
    if (attributes != INVALID_FILE_ATTRIBUTES && !(attributes & FILE_ATTRIBUTE_DIRECTORY)) {
        SetLastError(ERROR_DIRECTORY);
    } else {
        it->find = FindFirstFileA(pattern, &it->data);
    }
    free(pattern);
    if (it->find == INVALID_HANDLE_VALUE) {
        errno = win_errno();
        free(it);
        return NULL;
    }
    it->first = 1;
    return it;
}

static const char *dir_next(DirIter *it) {
    if (it->first) {
        it->first = 0;
        return it->data.cFileName;
    }
    return FindNextFileA(it->find, &it->data) ? it->data.cFileName : NULL;
}

static void dir_close(DirIter *it) {
    FindClose(it->find);
    free(it);
}
#else
struct DirIter {
    DIR *dir;
};

static DirIter *dir_open(const char *name) {
    DIR *dir = opendir(name);
    if (dir == NULL) {
        return NULL;
    }
    DirIter *it = malloc(sizeof *it);
    if (it == NULL) {
        closedir(dir);
        errno = ENOMEM;
        return NULL;
    }
    it->dir = dir;
    return it;
}

static const char *dir_next(DirIter *it) {
    struct dirent *entry = readdir(it->dir);
    return entry == NULL ? NULL : entry->d_name;
}

static void dir_close(DirIter *it) {
    closedir(it->dir);
    free(it);
}
#endif

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

void brim_rt_print_u64(int64_t value) {
    printf("%" PRIu64 "\n", (uint64_t)value);
}

BrimStr *brim_rt_u64_to_str(int64_t value);

double brim_rt_u64_to_f64(int64_t value) {
    return (double)(uint64_t)value;
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

BrimStr *brim_rt_u64_to_str(int64_t value) {
    char buf[32];
    int len = snprintf(buf, sizeof buf, "%" PRIu64, (uint64_t)value);
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

/*
 * Lists and maps whose count reached zero. Freeing one releases its
 * elements, which can free more lists and maps: instead of recursing
 * (a long chain of boxes would overflow the stack), they wait here and the
 * outermost release frees them one at a time.
 */
typedef struct Dead {
    void *ptr;
    int is_map;
} Dead;

static Dead *dead;
static int64_t dead_len;
static int64_t dead_cap;
static int freeing;

static void free_list(BrimList *l);
struct BrimMap;
static void free_map(struct BrimMap *m);

static void bury(void *ptr, int is_map) {
    if (dead_len == dead_cap) {
        int64_t cap = dead_cap < 16 ? 16 : dead_cap * 2;
        Dead *bigger = realloc(dead, (size_t)cap * sizeof(Dead));
        if (bigger == NULL) {
            fatal("out of memory");
        }
        dead = bigger;
        dead_cap = cap;
    }
    dead[dead_len].ptr = ptr;
    dead[dead_len].is_map = is_map;
    dead_len++;
    if (freeing) {
        return;
    }
    freeing = 1;
    while (dead_len > 0) {
        Dead d = dead[--dead_len];
        if (d.is_map) {
            free_map(d.ptr);
        } else {
            free_list(d.ptr);
        }
    }
    freeing = 0;
}

static void free_list(BrimList *l) {
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

void brim_rt_list_release(BrimList *l) {
    if (l == NULL || --l->rc > 0) {
        return;
    }
    bury(l, 0);
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
    bury(m, 1);
}

static void free_map(BrimMap *m) {
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

BrimStr *brim_rt_str_quote(const BrimStr *s) {
    Buffer b = {0};
    buffer_add(&b, "\"", 1);
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        switch (c) {
        case '"': buffer_add(&b, "\\\"", 2); break;
        case '\\': buffer_add(&b, "\\\\", 2); break;
        case '\n': buffer_add(&b, "\\n", 2); break;
        case '\r': buffer_add(&b, "\\r", 2); break;
        case '\t': buffer_add(&b, "\\t", 2); break;
        case 0: buffer_add(&b, "\\0", 2); break;
        default:
            if (c < 0x20 || c == 0x7f) {
                char esc[16];
                int n = snprintf(esc, sizeof esc, "\\u{%x}", c);
                buffer_add(&b, esc, n);
            } else {
                buffer_add(&b, (const char *)&s->data[i], 1);
            }
        }
    }
    buffer_add(&b, "\"", 1);
    return buffer_finish(&b);
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

BrimList *brim_rt_str_code_points(const BrimStr *s) {
    BrimList *l = NULL;
    const uint8_t *d = (const uint8_t *)s->data;
    for (int64_t i = 0; i < s->len;) {
        int64_t n = char_len(s, i);
        int64_t cp = d[i];
        if (n == 2) {
            cp = ((d[i] & 0x1F) << 6) | (d[i + 1] & 0x3F);
        } else if (n == 3) {
            cp = ((d[i] & 0x0F) << 12) | ((d[i + 1] & 0x3F) << 6) | (d[i + 2] & 0x3F);
        } else if (n == 4) {
            cp = ((int64_t)(d[i] & 0x07) << 18) | ((d[i + 1] & 0x3F) << 12) |
                 ((d[i + 2] & 0x3F) << 6) | (d[i + 3] & 0x3F);
        }
        l = brim_rt_list_push(l, &cp, (int64_t)sizeof cp, NULL, NULL);
        i += n;
    }
    return l;
}

uint8_t brim_rt_code_point_ok(int64_t n) {
    return (uint8_t)(n >= 0 && n <= 0x10FFFF && !(n >= 0xD800 && n <= 0xDFFF));
}

BrimStr *brim_rt_code_point_to_str(int64_t n) {
    char buf[4];
    int64_t len = 0;
    if (!brim_rt_code_point_ok(n)) {
        return str_from(buf, 0);
    }
    if (n < 0x80) {
        buf[len++] = (char)n;
    } else if (n < 0x800) {
        buf[len++] = (char)(0xC0 | (n >> 6));
        buf[len++] = (char)(0x80 | (n & 0x3F));
    } else if (n < 0x10000) {
        buf[len++] = (char)(0xE0 | (n >> 12));
        buf[len++] = (char)(0x80 | ((n >> 6) & 0x3F));
        buf[len++] = (char)(0x80 | (n & 0x3F));
    } else {
        buf[len++] = (char)(0xF0 | (n >> 18));
        buf[len++] = (char)(0x80 | ((n >> 12) & 0x3F));
        buf[len++] = (char)(0x80 | ((n >> 6) & 0x3F));
        buf[len++] = (char)(0x80 | (n & 0x3F));
    }
    return str_from(buf, len);
}

BrimList *brim_rt_str_bytes(const BrimStr *s) {
    BrimList *l = NULL;
    for (int64_t i = 0; i < s->len; i++) {
        int64_t b = (uint8_t)s->data[i];
        l = brim_rt_list_push(l, &b, (int64_t)sizeof b, NULL, NULL);
    }
    return l;
}

/* The bytes of a `List[u8]`, in a buffer the caller frees. */
static char *list_bytes(const BrimList *l, int64_t *len) {
    *len = brim_rt_list_len(l);
    char *data = malloc((size_t)(*len > 0 ? *len : 1));
    if (data == NULL) {
        fatal("out of memory");
    }
    for (int64_t i = 0; i < *len; i++) {
        data[i] = (char)*(int64_t *)(l->data + i * l->elem_size);
    }
    return data;
}

/* A `List[u8]` of `len` bytes. */
static BrimList *bytes_list(const uint8_t *data, int64_t len) {
    if (len == 0) {
        return NULL;
    }
    BrimList *l = list_alloc((int64_t)sizeof(int64_t), NULL, len);
    for (int64_t i = 0; i < len; i++) {
        ((int64_t *)l->data)[i] = data[i];
    }
    l->len = len;
    return l;
}

static int valid_utf8(const char *data, int64_t len);

uint8_t brim_rt_bytes_utf8_ok(const BrimList *l) {
    int64_t len;
    char *data = list_bytes(l, &len);
    int ok = valid_utf8(data, len);
    free(data);
    return (uint8_t)ok;
}

BrimStr *brim_rt_bytes_to_str(const BrimList *l) {
    int64_t len;
    char *data = list_bytes(l, &len);
    BrimStr *s = str_from(data, len);
    free(data);
    return s;
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

/* ---- std.fs ---------------------------------------------------------------- */

/* As brim_runtime::fs_error: `cannot <verb> "<path>": <reason>`. */
static void fs_error(const char *verb, const BrimStr *path, int error) {
    const char *reason;
    switch (error) {
    case ENOENT:
        reason = "not found";
        break;
    case EACCES:
    case EPERM:
        reason = "permission denied";
        break;
    case EEXIST:
        reason = "already exists";
        break;
    case ENOTEMPTY:
        reason = "directory not empty";
        break;
    case ENOTDIR:
        reason = "not a directory";
        break;
    case EISDIR:
        reason = "is a directory";
        break;
    default:
        reason = "input/output error";
    }
    Buffer b = {0};
    buffer_add(&b, "cannot ", 7);
    buffer_add(&b, verb, (int64_t)strlen(verb));
    buffer_add(&b, " \"", 2);
    buffer_add(&b, path->data, path->len);
    buffer_add(&b, "\": ", 3);
    buffer_add(&b, reason, (int64_t)strlen(reason));
    set_error(buffer_finish(&b));
}

uint8_t brim_rt_fs_exists(const BrimStr *path) {
    char *name = c_string(path);
    struct stat info;
    int found = stat(name, &info) == 0;
    free(name);
    return (uint8_t)found;
}

uint8_t brim_rt_fs_is_dir(const BrimStr *path) {
    char *name = c_string(path);
    struct stat info;
    int dir = stat(name, &info) == 0 && S_ISDIR(info.st_mode);
    free(name);
    return (uint8_t)dir;
}

static int compare_names(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

BrimList *brim_rt_fs_list(const BrimStr *path) {
    set_error(NULL);
    char *name = c_string(path);
    DirIter *dir = dir_open(name);
    free(name);
    if (dir == NULL) {
        fs_error("list directory", path, errno);
        return NULL;
    }
    char **names = NULL;
    size_t count = 0;
    size_t cap = 0;
    const char *entry;
    while ((entry = dir_next(dir)) != NULL) {
        if (strcmp(entry, ".") == 0 || strcmp(entry, "..") == 0) {
            continue;
        }
        if (count == cap) {
            cap = cap < 16 ? 16 : cap * 2;
            char **bigger = realloc(names, cap * sizeof *names);
            if (bigger == NULL) {
                fatal("out of memory");
            }
            names = bigger;
        }
        names[count] = malloc(strlen(entry) + 1);
        if (names[count] == NULL) {
            fatal("out of memory");
        }
        strcpy(names[count], entry);
        count++;
    }
    dir_close(dir);
    /* Byte order, as Rust sorts the names. */
    qsort(names, count, sizeof *names, compare_names);
    BrimList *l = NULL;
    for (size_t i = 0; i < count; i++) {
        l = push_str(l, str_from(names[i], (int64_t)strlen(names[i])));
        free(names[i]);
    }
    free(names);
    return l;
}

/* mkdir -p: each parent in turn. */
uint8_t brim_rt_fs_create_dir(const BrimStr *path) {
    set_error(NULL);
    char *name = c_string(path);
    struct stat info;
    if (stat(name, &info) == 0 && S_ISDIR(info.st_mode)) {
        free(name);
        return 0;
    }
    size_t len = strlen(name);
    for (size_t i = 1; i <= len; i++) {
        if (name[i] == '/' || name[i] == '\0') {
            char saved = name[i];
            name[i] = '\0';
            if (make_dir(name) != 0 && errno != EEXIST) {
                int error = errno;
                free(name);
                fs_error("create directory", path, error);
                return 0;
            }
            name[i] = saved;
        }
    }
    int ok = stat(name, &info) == 0 && S_ISDIR(info.st_mode);
    free(name);
    if (!ok) {
        fs_error("create directory", path, EEXIST);
        return 0;
    }
    return 1;
}

/* Removes `name`: a file, or a directory (with its contents when `all`). */
static int remove_path(const char *name, int all) {
    struct stat info;
    if (lstat(name, &info) != 0) {
        return -1;
    }
    if (!S_ISDIR(info.st_mode)) {
        return unlink(name);
    }
    if (all) {
        DirIter *dir = dir_open(name);
        if (dir == NULL) {
            return -1;
        }
        const char *entry;
        while ((entry = dir_next(dir)) != NULL) {
            if (strcmp(entry, ".") == 0 || strcmp(entry, "..") == 0) {
                continue;
            }
            size_t n = strlen(name) + strlen(entry) + 2;
            char *child = malloc(n);
            if (child == NULL) {
                fatal("out of memory");
            }
            snprintf(child, n, "%s/%s", name, entry);
            int failed = remove_path(child, 1);
            free(child);
            if (failed != 0) {
                int error = errno;
                dir_close(dir);
                errno = error;
                return -1;
            }
        }
        dir_close(dir);
    }
    return rmdir(name);
}

static uint8_t fs_remove(const BrimStr *path, int all) {
    set_error(NULL);
    char *name = c_string(path);
    struct stat info;
    if (lstat(name, &info) != 0 && errno == ENOENT) {
        free(name);
        return 0;
    }
    int failed = remove_path(name, all);
    int error = errno;
    free(name);
    if (failed != 0) {
        fs_error("remove", path, error);
        return 0;
    }
    return 1;
}

uint8_t brim_rt_fs_remove(const BrimStr *path) {
    return fs_remove(path, 0);
}

uint8_t brim_rt_fs_remove_all(const BrimStr *path) {
    return fs_remove(path, 1);
}

uint8_t brim_rt_fs_rename(const BrimStr *from, const BrimStr *to) {
    set_error(NULL);
    char *a = c_string(from);
    char *b = c_string(to);
    int failed = rename(a, b);
    int error = errno;
    free(a);
    free(b);
    if (failed != 0) {
        fs_error("rename", from, error);
        return 0;
    }
    return 1;
}

int64_t brim_rt_fs_size(const BrimStr *path) {
    set_error(NULL);
    char *name = c_string(path);
    struct stat info;
    int failed = stat(name, &info);
    int error = errno;
    free(name);
    if (failed != 0) {
        fs_error("read the size of", path, error);
        return 0;
    }
    return (int64_t)info.st_size;
}

int64_t brim_rt_fs_modified(const BrimStr *path) {
    set_error(NULL);
    char *name = c_string(path);
    struct stat info;
    int failed = stat(name, &info);
    int error = errno;
    free(name);
    if (failed != 0) {
        fs_error("read the time of", path, error);
        return 0;
    }
#if defined(_WIN32)
    /* In milliseconds, as Rust reads it; `stat` has whole seconds. */
    WIN32_FILE_ATTRIBUTE_DATA data;
    char *again = c_string(path);
    BOOL ok = GetFileAttributesExA(again, GetFileExInfoStandard, &data);
    free(again);
    if (!ok) {
        return (int64_t)info.st_mtime * 1000;
    }
    uint64_t ticks = ((uint64_t)data.ftLastWriteTime.dwHighDateTime << 32) |
                     data.ftLastWriteTime.dwLowDateTime;
    return (int64_t)((ticks - 116444736000000000ULL) / 10000);
#elif defined(__APPLE__)
    /* macOS in strict POSIX mode. */
    return (int64_t)info.st_mtime * 1000 + (int64_t)(info.st_mtimensec / 1000000);
#else
    return (int64_t)info.st_mtim.tv_sec * 1000 + (int64_t)(info.st_mtim.tv_nsec / 1000000);
#endif
}

int64_t brim_rt_fs_append(const BrimStr *path, const BrimStr *text) {
    set_error(NULL);
    char *name = c_string(path);
    FILE *f = fopen(name, "ab");
    int error = errno;
    free(name);
    if (f == NULL) {
        fs_error("append to", path, error);
        return 0;
    }
    size_t written = fwrite(text->data, 1, (size_t)text->len, f);
    int failed = written != (size_t)text->len;
    if (fclose(f) != 0) {
        failed = 1;
    }
    if (failed) {
        fs_error("append to", path, EIO);
        return 0;
    }
    return text->len;
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

/* ---- std.process ----------------------------------------------------------- */

#ifndef _WIN32
extern char **environ;
#endif

static int64_t process_code;
static BrimStr *process_stdout;
static BrimStr *process_stderr;

static void set_process_output(int64_t code, BrimStr *out, BrimStr *err) {
    process_code = code;
    brim_rt_release(process_stdout);
    brim_rt_release(process_stderr);
    process_stdout = out;
    process_stderr = err;
}

static int has_nul(const BrimStr *s) {
    return s->len > 0 && memchr(s->data, 0, (size_t)s->len) != NULL;
}

#ifdef _WIN32
/* Adds `arg` to a Windows command line, quoted the way programs (and
 * Rust's `Command`) split it again. */
static void add_quoted(Buffer *b, const char *arg) {
    if (*arg != '\0' && strpbrk(arg, " \t\n\v\"") == NULL) {
        buffer_add(b, arg, (int64_t)strlen(arg));
        return;
    }
    buffer_add(b, "\"", 1);
    for (const char *p = arg;; p++) {
        int64_t slashes = 0;
        while (*p == '\\') {
            p++;
            slashes++;
        }
        if (*p == '\0') {
            for (int64_t i = 0; i < slashes * 2; i++) {
                buffer_add(b, "\\", 1);
            }
            break;
        }
        if (*p == '"') {
            for (int64_t i = 0; i < slashes * 2 + 1; i++) {
                buffer_add(b, "\\", 1);
            }
        } else {
            for (int64_t i = 0; i < slashes; i++) {
                buffer_add(b, "\\", 1);
            }
        }
        buffer_add(b, p, 1);
    }
    buffer_add(b, "\"", 1);
}

typedef struct PipeReader {
    HANDLE pipe;
    Buffer *into;
} PipeReader;

static void read_pipe(HANDLE pipe, Buffer *into) {
    char chunk[65536];
    DWORD n;
    while (ReadFile(pipe, chunk, sizeof chunk, &n, NULL) && n > 0) {
        buffer_add(into, chunk, (int64_t)n);
    }
}

static DWORD WINAPI pipe_reader(LPVOID arg) {
    PipeReader *r = arg;
    read_pipe(r->pipe, r->into);
    return 0;
}

/* Runs `argv` with no input and collects its output; 0, or an errno. */
static int spawn_collect(char **argv, Buffer *out, Buffer *err, int64_t *code) {
    Buffer line = {0};
    for (int i = 0; argv[i] != NULL; i++) {
        if (i > 0) {
            buffer_add(&line, " ", 1);
        }
        add_quoted(&line, argv[i]);
    }
    buffer_add(&line, "", 1);
    line.data[line.len - 1] = '\0';
    SECURITY_ATTRIBUTES inherit = {sizeof inherit, NULL, TRUE};
    HANDLE out_r = NULL, out_w = NULL, err_r = NULL, err_w = NULL;
    if (!CreatePipe(&out_r, &out_w, &inherit, 0) || !CreatePipe(&err_r, &err_w, &inherit, 0)) {
        free(line.data);
        return EIO;
    }
    SetHandleInformation(out_r, HANDLE_FLAG_INHERIT, 0);
    SetHandleInformation(err_r, HANDLE_FLAG_INHERIT, 0);
    HANDLE nul = CreateFileA("NUL", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, &inherit,
                             OPEN_EXISTING, 0, NULL);
    STARTUPINFOA si;
    memset(&si, 0, sizeof si);
    si.cb = sizeof si;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = nul;
    si.hStdOutput = out_w;
    si.hStdError = err_w;
    PROCESS_INFORMATION pi;
    BOOL started = CreateProcessA(NULL, line.data, NULL, NULL, TRUE, CREATE_NO_WINDOW, NULL, NULL,
                                  &si, &pi);
    int error = started ? 0 : win_errno();
    free(line.data);
    CloseHandle(out_w);
    CloseHandle(err_w);
    if (nul != INVALID_HANDLE_VALUE) {
        CloseHandle(nul);
    }
    if (!started) {
        CloseHandle(out_r);
        CloseHandle(err_r);
        return error;
    }
    /* Standard error on a thread of its own, so neither pipe fills up. */
    PipeReader reader = {err_r, err};
    HANDLE thread = CreateThread(NULL, 0, pipe_reader, &reader, 0, NULL);
    read_pipe(out_r, out);
    if (thread != NULL) {
        WaitForSingleObject(thread, INFINITE);
        CloseHandle(thread);
    } else {
        read_pipe(err_r, err);
    }
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD exit_code = 0;
    GetExitCodeProcess(pi.hProcess, &exit_code);
    *code = (int64_t)exit_code;
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    CloseHandle(out_r);
    CloseHandle(err_r);
    return 0;
}
#else
/* Reads both pipes until they close, so neither can fill up and block. */
static void read_pipes(int out_fd, int err_fd, Buffer *out, Buffer *err) {
    struct pollfd fds[2] = {{out_fd, POLLIN, 0}, {err_fd, POLLIN, 0}};
    Buffer *bufs[2] = {out, err};
    int open = 2;
    char chunk[65536];
    while (open > 0) {
        if (poll(fds, 2, -1) < 0) {
            if (errno == EINTR) {
                continue;
            }
            break;
        }
        for (int i = 0; i < 2; i++) {
            if (fds[i].fd < 0 || fds[i].revents == 0) {
                continue;
            }
            ssize_t n = read(fds[i].fd, chunk, sizeof chunk);
            if (n > 0) {
                buffer_add(bufs[i], chunk, (int64_t)n);
            } else if (n == 0 || errno != EINTR) {
                close(fds[i].fd);
                fds[i].fd = -1;
                open--;
            }
        }
    }
    for (int i = 0; i < 2; i++) {
        if (fds[i].fd >= 0) {
            close(fds[i].fd);
        }
    }
}
#endif

/* Runs `argv` with no input and collects its output and exit code; 0, or
 * an errno when it cannot start. */
static int spawn_collect(char **argv, Buffer *out, Buffer *err, int64_t *code);

/* As brim_runtime::process_run. */
int64_t brim_rt_process_run(const BrimStr *cmd, BrimList *args) {
    set_error(NULL);
    set_process_output(-1, NULL, NULL);
    int64_t argc = brim_rt_list_len(args);
    int bad = has_nul(cmd);
    for (int64_t i = 0; i < argc; i++) {
        bad = bad || has_nul(*(BrimStr **)elem_at(args, i));
    }
    if (bad) {
        fs_error("run", cmd, EINVAL);
        return -1;
    }
    char **argv = malloc(sizeof(char *) * (size_t)(argc + 2));
    if (argv == NULL) {
        fatal("out of memory");
    }
    argv[0] = c_string(cmd);
    for (int64_t i = 0; i < argc; i++) {
        argv[i + 1] = c_string(*(BrimStr **)elem_at(args, i));
    }
    argv[argc + 1] = NULL;
    Buffer out = {0};
    Buffer err = {0};
    int64_t code = 0;
    int error = spawn_collect(argv, &out, &err, &code);
    for (int64_t i = 0; i <= argc; i++) {
        free(argv[i]);
    }
    free(argv);
    if (error != 0) {
        fs_error("run", cmd, error);
        return -1;
    }
    if (!valid_utf8(out.data, out.len) || !valid_utf8(err.data, err.len)) {
        free(out.data);
        free(err.data);
        Buffer b = {0};
        buffer_add(&b, "cannot run \"", 12);
        buffer_add(&b, cmd->data, cmd->len);
        buffer_add(&b, "\": output is not valid UTF-8", 28);
        set_error(buffer_finish(&b));
        return -1;
    }
    set_process_output(code, buffer_finish(&out), buffer_finish(&err));
    return code;
}

#ifndef _WIN32
static int spawn_collect(char **argv, Buffer *out_buf, Buffer *err_buf, int64_t *code_out) {
    int out_pipe[2] = {-1, -1};
    int err_pipe[2] = {-1, -1};
    int error = 0;
    pid_t pid = 0;
    if (pipe(out_pipe) != 0 || pipe(err_pipe) != 0) {
        error = errno;
    } else {
        posix_spawn_file_actions_t actions;
        posix_spawn_file_actions_init(&actions);
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0);
        posix_spawn_file_actions_adddup2(&actions, out_pipe[1], 1);
        posix_spawn_file_actions_adddup2(&actions, err_pipe[1], 2);
        for (int i = 0; i < 2; i++) {
            posix_spawn_file_actions_addclose(&actions, out_pipe[i]);
            posix_spawn_file_actions_addclose(&actions, err_pipe[i]);
        }
        error = posix_spawnp(&pid, argv[0], &actions, NULL, argv, environ);
        posix_spawn_file_actions_destroy(&actions);
    }
    if (out_pipe[1] >= 0) {
        close(out_pipe[1]);
    }
    if (err_pipe[1] >= 0) {
        close(err_pipe[1]);
    }
    if (error != 0) {
        if (out_pipe[0] >= 0) {
            close(out_pipe[0]);
        }
        if (err_pipe[0] >= 0) {
            close(err_pipe[0]);
        }
        return error;
    }
    read_pipes(out_pipe[0], err_pipe[0], out_buf, err_buf);
    int status = 0;
    while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {
    }
    *code_out = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
    return 0;
}
#endif

BrimStr *brim_rt_process_stdout(void) {
    if (process_stdout == NULL) {
        return &EMPTY_STR;
    }
    brim_rt_retain(process_stdout);
    return process_stdout;
}

BrimStr *brim_rt_process_stderr(void) {
    if (process_stderr == NULL) {
        return &EMPTY_STR;
    }
    brim_rt_retain(process_stderr);
    return process_stderr;
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
#ifdef _WIN32
        WSADATA data;
        WSAStartup(MAKEWORD(2, 2), &data);
#else
        signal(SIGPIPE, SIG_IGN);
#endif
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
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, (const char *)&on, sizeof on);
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
            error = sock_errno();
            continue;
        }
        int on = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, (const char *)&on, sizeof on);
        if (bind(fd, a->ai_addr, a->ai_addrlen) == 0 && listen(fd, 128) == 0) {
            freeaddrinfo(found);
            return socket_add(fd, 1);
        }
        error = sock_errno();
        sock_close(fd);
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
    } while (fd < 0 && sock_errno() == EINTR);
    if (fd < 0) {
        net_fail(net_reason(sock_errno()));
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
            error = sock_errno();
            continue;
        }
        if (connect(fd, a->ai_addr, a->ai_addrlen) == 0) {
            freeaddrinfo(found);
            no_delay(fd);
            return socket_add(fd, 0);
        }
        error = sock_errno();
        sock_close(fd);
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
    } while (n < 0 && sock_errno() == EINTR);
    if (n < 0) {
        net_fail(net_reason(sock_errno()));
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

BrimStr *brim_rt_net_read_until(int64_t handle, const BrimStr *delimiter, int64_t max) {
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
    if (max < 0) {
        net_fail("invalid length");
        return &EMPTY_STR;
    }
    int64_t searched = 0;
    for (;;) {
        for (int64_t i = searched; i + d <= s->len; i++) {
            if (memcmp(s->buf + i, delimiter->data, (size_t)d) == 0) {
                if (i > max) {
                    net_fail("message too large");
                    return &EMPTY_STR;
                }
                return socket_take(s, i, d);
            }
        }
        searched = s->len - d + 1 > 0 ? s->len - d + 1 : 0;
        if (searched > max) {
            net_fail("message too large");
            return &EMPTY_STR;
        }
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

BrimList *brim_rt_net_read_bytes(int64_t handle, int64_t count) {
    net_start();
    Socket *s = socket_get(handle, 0);
    if (s == NULL) {
        net_fail("invalid socket");
        return NULL;
    }
    if (count < 0) {
        net_fail("invalid length");
        return NULL;
    }
    while (s->len < count) {
        int64_t n = socket_fill(s);
        if (n < 0) {
            return NULL;
        }
        if (n == 0) {
            net_fail(CLOSED_EARLY);
            return NULL;
        }
    }
    BrimList *l = bytes_list((const uint8_t *)s->buf, count);
    memmove(s->buf, s->buf + count, (size_t)(s->len - count));
    s->len -= count;
    return l;
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
            if (sock_errno() == EINTR) {
                continue;
            }
            net_fail(net_reason(sock_errno()));
            return 0;
        }
        sent += n;
    }
    return text->len;
}

int64_t brim_rt_net_write_bytes(int64_t handle, const BrimList *bytes) {
    net_start();
    Socket *s = socket_get(handle, 0);
    if (s == NULL) {
        net_fail("invalid socket");
        return 0;
    }
    int64_t len;
    char *data = list_bytes(bytes, &len);
    int64_t sent = 0;
    while (sent < len) {
        ssize_t n = send(s->fd, data + sent, (size_t)(len - sent), 0);
        if (n < 0) {
            if (sock_errno() == EINTR) {
                continue;
            }
            free(data);
            net_fail(net_reason(sock_errno()));
            return 0;
        }
        sent += n;
    }
    free(data);
    return len;
}

/* Connections given back to a listener (HTTP keep-alive), oldest first:
 * as `idle` in brim-runtime's net.rs. */
typedef struct Kept {
    int64_t listener;
    int64_t conn;
    int64_t since;
} Kept;

static Kept *kept;
static int64_t kept_count;
#define MAX_IDLE 256

static void kept_remove(int64_t i) {
    memmove(&kept[i], &kept[i + 1], sizeof(Kept) * (size_t)(kept_count - i - 1));
    kept_count--;
}

int64_t brim_rt_time_monotonic_millis(void);

static void close_socket(int64_t handle) {
    if (handle < 1 || handle > socket_count) {
        return;
    }
    Socket *s = &sockets[handle - 1];
    if (s->fd >= 0 && s->listening) {
        for (int64_t i = 0; i < kept_count;) {
            if (kept[i].listener == handle) {
                int64_t conn = kept[i].conn;
                kept_remove(i);
                close_socket(conn);
            } else {
                i++;
            }
        }
    }
    for (int64_t i = 0; i < kept_count; i++) {
        if (kept[i].conn == handle) {
            kept_remove(i);
            break;
        }
    }
    if (s->fd >= 0) {
        if (!s->listening) {
            shutdown(s->fd, SHUT_RDWR);
        }
        sock_close(s->fd);
        s->fd = -1;
        free(s->buf);
        s->buf = NULL;
        s->len = s->cap = 0;
    }
}

void brim_rt_net_close(int64_t handle) {
    net_start();
    close_socket(handle);
}

void brim_rt_net_keep(int64_t listener, int64_t conn) {
    net_start();
    if (socket_get(listener, 1) == NULL || socket_get(conn, 0) == NULL) {
        close_socket(conn);
        return;
    }
    Kept *grown = realloc(kept, sizeof(Kept) * (size_t)(kept_count + 1));
    if (grown == NULL) {
        fatal("out of memory");
    }
    kept = grown;
    kept[kept_count++] = (Kept){listener, conn, brim_rt_time_monotonic_millis()};
    int64_t count = 0;
    int64_t oldest = -1;
    for (int64_t i = 0; i < kept_count; i++) {
        if (kept[i].listener == listener) {
            if (oldest < 0) {
                oldest = i;
            }
            count++;
        }
    }
    if (count > MAX_IDLE) {
        int64_t c = kept[oldest].conn;
        kept_remove(oldest);
        close_socket(c);
    }
}

/* 1 if a kept connection has something to read, 0 if not, -1 if the
 * other side closed it. */
static int kept_readiness(int64_t handle) {
    Socket *s = socket_get(handle, 0);
    if (s == NULL) {
        return -1;
    }
    if (s->len > 0) {
        return 1;
    }
    struct pollfd fd = {s->fd, POLLIN, 0};
    int got;
    do {
        got = poll(&fd, 1, 0);
    } while (got < 0 && sock_errno() == EINTR);
    if (got == 0) {
        return 0;
    }
    if (got < 0 || !(fd.revents & (POLLIN | POLLHUP))) {
        return -1;
    }
    /* Readable: data, or the end of the stream. */
    char byte;
    ssize_t n;
    do {
        n = recv(s->fd, &byte, 1, MSG_PEEK);
    } while (n < 0 && sock_errno() == EINTR);
    return n > 0 ? 1 : -1;
}

int64_t brim_rt_net_next_ready(int64_t listener, int64_t idle_ms) {
    net_start();
    Socket *l = socket_get(listener, 1);
    if (l == NULL) {
        net_fail("invalid socket");
        return 0;
    }
    for (;;) {
        int64_t now = brim_rt_time_monotonic_millis();
        int64_t wait = -1;
        for (int64_t i = 0; i < kept_count;) {
            if (kept[i].listener != listener) {
                i++;
                continue;
            }
            int64_t conn = kept[i].conn;
            int ready = kept_readiness(conn);
            if (ready > 0) {
                kept_remove(i);
                return conn;
            }
            int64_t left = kept[i].since + idle_ms - now;
            if (ready < 0 || left <= 0) {
                kept_remove(i);
                close_socket(conn);
                continue;
            }
            if (wait < 0 || left < wait) {
                wait = left;
            }
            i++;
        }
        if (wait < 0) {
            return brim_rt_net_accept(listener);
        }
        /* The listener and the kept connections, until one can be read. */
        int64_t n = 1;
        struct pollfd *fds = malloc(sizeof(struct pollfd) * (size_t)(kept_count + 1));
        if (fds == NULL) {
            fatal("out of memory");
        }
        fds[0] = (struct pollfd){l->fd, POLLIN, 0};
        for (int64_t i = 0; i < kept_count; i++) {
            if (kept[i].listener == listener) {
                fds[n++] = (struct pollfd){sockets[kept[i].conn - 1].fd, POLLIN, 0};
            }
        }
        int got = poll(fds, (brim_nfds_t)n, (int)wait);
        int listener_ready = got > 0 && (fds[0].revents & POLLIN);
        free(fds);
        if (got < 0 && sock_errno() != EINTR) {
            net_fail(net_reason(sock_errno()));
            return 0;
        }
        if (listener_ready) {
            /* Kept connections first, as the interpreter checks them. */
            int any = 0;
            for (int64_t i = 0; i < kept_count; i++) {
                if (kept[i].listener == listener && kept_readiness(kept[i].conn) != 0) {
                    any = 1;
                }
            }
            if (!any) {
                return brim_rt_net_accept(listener);
            }
        }
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
        net_fail(net_reason(sock_errno()));
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
#ifdef _WIN32
    DWORD tv = (DWORD)millis;
#else
    struct timeval tv;
    tv.tv_sec = (time_t)(millis / 1000);
    tv.tv_usec = (suseconds_t)((millis % 1000) * 1000);
#endif
    if (setsockopt(s->fd, SOL_SOCKET, SO_RCVTIMEO, (const char *)&tv, sizeof tv) != 0 ||
        setsockopt(s->fd, SOL_SOCKET, SO_SNDTIMEO, (const char *)&tv, sizeof tv) != 0) {
        net_fail(net_reason(sock_errno()));
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

/* ---- time (std.time) ------------------------------------------------------- */

/* ---- sorting (`sorted()` for integers and strings) ------------------------ */

static int compare_i64(const void *a, const void *b) {
    int64_t x = *(const int64_t *)a, y = *(const int64_t *)b;
    return (x > y) - (x < y);
}

static int compare_u64(const void *a, const void *b) {
    uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
    return (x > y) - (x < y);
}

static int compare_strs(const void *a, const void *b) {
    return (int)brim_rt_str_compare(*(BrimStr *const *)a, *(BrimStr *const *)b);
}

/* A copy of a list with its elements sorted by `compare`. */
static BrimList *sorted_copy(const BrimList *l, int (*compare)(const void *, const void *),
                             int strings) {
    int64_t len = brim_rt_list_len(l);
    if (len == 0) {
        return NULL;
    }
    BrimList *out = list_alloc(l->elem_size, l->release, len);
    memcpy(out->data, l->data, (size_t)(len * l->elem_size));
    out->len = len;
    if (strings) {
        for (int64_t i = 0; i < len; i++) {
            brim_rt_retain(*(BrimStr **)elem_at(out, i));
        }
    }
    qsort(out->data, (size_t)len, (size_t)l->elem_size, compare);
    return out;
}

BrimList *brim_rt_list_sort_i64(const BrimList *l) {
    return sorted_copy(l, compare_i64, 0);
}

BrimList *brim_rt_list_sort_u64(const BrimList *l) {
    return sorted_copy(l, compare_u64, 0);
}

BrimList *brim_rt_list_sort_str(const BrimList *l) {
    return sorted_copy(l, compare_strs, 1);
}

/* ---- std.crypto: as brim_runtime::crypto ---------------------------------- */

static uint32_t rotr32(uint32_t x, int n) {
    return (x >> n) | (x << (32 - n));
}

static uint32_t rotl32(uint32_t x, int n) {
    return (x << n) | (x >> (32 - n));
}

/* The message with MD-style padding: big-endian length for SHA-256,
 * little-endian for MD5. The caller frees it. */
static uint8_t *hash_pad(const uint8_t *data, int64_t len, int big_endian, int64_t *padded_len) {
    int64_t total = len + 1;
    while (total % 64 != 56) {
        total++;
    }
    total += 8;
    uint8_t *out = calloc((size_t)total, 1);
    if (out == NULL) {
        fatal("out of memory");
    }
    if (len > 0) {
        memcpy(out, data, (size_t)len);
    }
    out[len] = 0x80;
    uint64_t bits = (uint64_t)len * 8;
    for (int i = 0; i < 8; i++) {
        int shift = big_endian ? 56 - 8 * i : 8 * i;
        out[total - 8 + i] = (uint8_t)(bits >> shift);
    }
    *padded_len = total;
    return out;
}

static void sha256_raw(const uint8_t *data, int64_t len, uint8_t out[32]) {
    static const uint32_t k[64] = {
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2};
    uint32_t h[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                     0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
    int64_t total;
    uint8_t *m = hash_pad(data, len, 1, &total);
    for (int64_t block = 0; block < total; block += 64) {
        uint32_t w[64];
        for (int i = 0; i < 16; i++) {
            const uint8_t *b = m + block + 4 * i;
            w[i] = (uint32_t)b[0] << 24 | (uint32_t)b[1] << 16 | (uint32_t)b[2] << 8 | b[3];
        }
        for (int i = 16; i < 64; i++) {
            uint32_t s0 = rotr32(w[i - 15], 7) ^ rotr32(w[i - 15], 18) ^ (w[i - 15] >> 3);
            uint32_t s1 = rotr32(w[i - 2], 17) ^ rotr32(w[i - 2], 19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16] + s0 + w[i - 7] + s1;
        }
        uint32_t a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];
        for (int i = 0; i < 64; i++) {
            uint32_t s1 = rotr32(e, 6) ^ rotr32(e, 11) ^ rotr32(e, 25);
            uint32_t ch = (e & f) ^ (~e & g);
            uint32_t t1 = hh + s1 + ch + k[i] + w[i];
            uint32_t s0 = rotr32(a, 2) ^ rotr32(a, 13) ^ rotr32(a, 22);
            uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
            uint32_t t2 = s0 + maj;
            hh = g;
            g = f;
            f = e;
            e = d + t1;
            d = c;
            c = b;
            b = a;
            a = t1 + t2;
        }
        h[0] += a;
        h[1] += b;
        h[2] += c;
        h[3] += d;
        h[4] += e;
        h[5] += f;
        h[6] += g;
        h[7] += hh;
    }
    free(m);
    for (int i = 0; i < 8; i++) {
        out[4 * i] = (uint8_t)(h[i] >> 24);
        out[4 * i + 1] = (uint8_t)(h[i] >> 16);
        out[4 * i + 2] = (uint8_t)(h[i] >> 8);
        out[4 * i + 3] = (uint8_t)h[i];
    }
}

static void hmac_sha256_raw(const uint8_t *key, int64_t key_len, const uint8_t *msg,
                            int64_t msg_len, uint8_t out[32]) {
    uint8_t block[64] = {0};
    if (key_len > 64) {
        sha256_raw(key, key_len, block);
    } else if (key_len > 0) {
        memcpy(block, key, (size_t)key_len);
    }
    uint8_t *inner = malloc((size_t)(64 + msg_len));
    if (inner == NULL) {
        fatal("out of memory");
    }
    for (int i = 0; i < 64; i++) {
        inner[i] = block[i] ^ 0x36;
    }
    if (msg_len > 0) {
        memcpy(inner + 64, msg, (size_t)msg_len);
    }
    uint8_t outer[96];
    for (int i = 0; i < 64; i++) {
        outer[i] = block[i] ^ 0x5c;
    }
    sha256_raw(inner, 64 + msg_len, outer + 64);
    free(inner);
    sha256_raw(outer, 96, out);
}

static void md5_raw(const uint8_t *data, int64_t len, uint8_t out[16]) {
    static const int s[64] = {7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
                              5, 9,  14, 20, 5, 9,  14, 20, 5, 9,  14, 20, 5, 9,  14, 20,
                              4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
                              6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21};
    uint32_t k[64];
    for (int i = 0; i < 64; i++) {
        k[i] = (uint32_t)(fabs(sin((double)(i + 1))) * 4294967296.0);
    }
    uint32_t h[4] = {0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476};
    int64_t total;
    uint8_t *m = hash_pad(data, len, 0, &total);
    for (int64_t block = 0; block < total; block += 64) {
        uint32_t w[16];
        for (int i = 0; i < 16; i++) {
            const uint8_t *b = m + block + 4 * i;
            w[i] = (uint32_t)b[3] << 24 | (uint32_t)b[2] << 16 | (uint32_t)b[1] << 8 | b[0];
        }
        uint32_t a = h[0], b = h[1], c = h[2], d = h[3];
        for (int i = 0; i < 64; i++) {
            uint32_t f;
            int g;
            switch (i / 16) {
            case 0:
                f = (b & c) | (~b & d);
                g = i;
                break;
            case 1:
                f = (d & b) | (~d & c);
                g = (5 * i + 1) % 16;
                break;
            case 2:
                f = b ^ c ^ d;
                g = (3 * i + 5) % 16;
                break;
            default:
                f = c ^ (b | ~d);
                g = (7 * i) % 16;
            }
            f = f + a + k[i] + w[g];
            a = d;
            d = c;
            c = b;
            b = b + rotl32(f, s[i]);
        }
        h[0] += a;
        h[1] += b;
        h[2] += c;
        h[3] += d;
    }
    free(m);
    for (int i = 0; i < 4; i++) {
        out[4 * i] = (uint8_t)h[i];
        out[4 * i + 1] = (uint8_t)(h[i] >> 8);
        out[4 * i + 2] = (uint8_t)(h[i] >> 16);
        out[4 * i + 3] = (uint8_t)(h[i] >> 24);
    }
}

BrimList *brim_rt_sha256(const BrimList *l) {
    int64_t len;
    char *data = list_bytes(l, &len);
    uint8_t out[32];
    sha256_raw((const uint8_t *)data, len, out);
    free(data);
    return bytes_list(out, 32);
}

BrimList *brim_rt_hmac_sha256(const BrimList *key, const BrimList *msg) {
    int64_t key_len, msg_len;
    char *k = list_bytes(key, &key_len);
    char *m = list_bytes(msg, &msg_len);
    uint8_t out[32];
    hmac_sha256_raw((const uint8_t *)k, key_len, (const uint8_t *)m, msg_len, out);
    free(k);
    free(m);
    return bytes_list(out, 32);
}

BrimList *brim_rt_pbkdf2_sha256(const BrimList *password, const BrimList *salt,
                                int64_t iterations, int64_t length) {
    if (length <= 0) {
        return NULL;
    }
    int64_t pass_len, salt_len;
    char *pass = list_bytes(password, &pass_len);
    char *s = list_bytes(salt, &salt_len);
    uint8_t *out = malloc((size_t)length);
    uint8_t *first = malloc((size_t)(salt_len + 4));
    if (out == NULL || first == NULL) {
        fatal("out of memory");
    }
    if (salt_len > 0) {
        memcpy(first, s, (size_t)salt_len);
    }
    int64_t done = 0;
    for (uint32_t block = 1; done < length; block++) {
        first[salt_len] = (uint8_t)(block >> 24);
        first[salt_len + 1] = (uint8_t)(block >> 16);
        first[salt_len + 2] = (uint8_t)(block >> 8);
        first[salt_len + 3] = (uint8_t)block;
        uint8_t u[32], t[32];
        hmac_sha256_raw((const uint8_t *)pass, pass_len, first, salt_len + 4, u);
        memcpy(t, u, 32);
        for (int64_t i = 1; i < iterations; i++) {
            uint8_t next[32];
            hmac_sha256_raw((const uint8_t *)pass, pass_len, u, 32, next);
            memcpy(u, next, 32);
            for (int j = 0; j < 32; j++) {
                t[j] ^= u[j];
            }
        }
        int64_t take = length - done < 32 ? length - done : 32;
        memcpy(out + done, t, (size_t)take);
        done += take;
    }
    BrimList *l = bytes_list(out, length);
    free(out);
    free(first);
    free(pass);
    free(s);
    return l;
}

BrimList *brim_rt_md5(const BrimList *l) {
    int64_t len;
    char *data = list_bytes(l, &len);
    uint8_t out[16];
    md5_raw((const uint8_t *)data, len, out);
    free(data);
    return bytes_list(out, 16);
}

/* Bytes from the system's secure random source. */
static void os_random(void *data, size_t n) {
#ifdef _WIN32
    if (BCryptGenRandom(NULL, data, (ULONG)n, BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0) {
        fatal("cannot get random bytes");
    }
#else
    FILE *f = fopen("/dev/urandom", "rb");
    if (f == NULL || fread(data, 1, n, f) != n) {
        fatal("cannot read /dev/urandom");
    }
    fclose(f);
#endif
}

BrimList *brim_rt_random_bytes(int64_t n) {
    if (n <= 0) {
        return NULL;
    }
    uint8_t *data = malloc((size_t)n);
    if (data == NULL) {
        fatal("out of memory");
    }
    os_random(data, (size_t)n);
    BrimList *l = bytes_list(data, n);
    free(data);
    return l;
}

/* ---- std.random: xoshiro256**, as brim_runtime::random ------------------ */

static uint64_t random_state[4];
static int random_ready = 0;

static uint64_t splitmix64(uint64_t *x) {
    *x += 0x9e3779b97f4a7c15ULL;
    uint64_t z = *x;
    z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
    z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
    return z ^ (z >> 31);
}

static void random_seed_with(uint64_t seed) {
    for (int i = 0; i < 4; i++) {
        random_state[i] = splitmix64(&seed);
    }
    random_ready = 1;
}

static uint64_t rotl64(uint64_t x, int k) {
    return (x << k) | (x >> (64 - k));
}

static uint64_t random_next(void) {
    if (!random_ready) {
        uint64_t seed = 0;
        os_random(&seed, sizeof seed);
        random_seed_with(seed);
    }
    uint64_t *s = random_state;
    uint64_t result = rotl64(s[1] * 5, 7) * 9;
    uint64_t t = s[1] << 17;
    s[2] ^= s[0];
    s[3] ^= s[1];
    s[1] ^= s[2];
    s[0] ^= s[3];
    s[2] ^= t;
    s[3] = rotl64(s[3], 45);
    return result;
}

void brim_rt_random_seed(int64_t n) {
    random_seed_with((uint64_t)n);
}

int64_t brim_rt_random_range(int64_t low, int64_t high) {
    if (high <= low) {
        return low;
    }
    uint64_t span = (uint64_t)high - (uint64_t)low;
    uint64_t threshold = (0 - span) % span;
    for (;;) {
        uint64_t r = random_next();
        if (r >= threshold) {
            return (int64_t)((uint64_t)low + r % span);
        }
    }
}

double brim_rt_random_float(void) {
    return (double)(random_next() >> 11) * (1.0 / 9007199254740992.0);
}

int64_t brim_rt_time_now_millis(void) {
#ifdef _WIN32
    FILETIME ft;
    GetSystemTimeAsFileTime(&ft);
    uint64_t ticks = ((uint64_t)ft.dwHighDateTime << 32) | ft.dwLowDateTime;
    return (int64_t)((ticks - 116444736000000000ULL) / 10000);
#else
    struct timespec t;
    clock_gettime(CLOCK_REALTIME, &t);
    return (int64_t)t.tv_sec * 1000 + (int64_t)(t.tv_nsec / 1000000);
#endif
}

int64_t brim_rt_time_monotonic_millis(void) {
    static int64_t start = -1;
#ifdef _WIN32
    int64_t now = (int64_t)GetTickCount64();
#else
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    int64_t now = (int64_t)t.tv_sec * 1000 + (int64_t)(t.tv_nsec / 1000000);
#endif
    if (start < 0) {
        start = now;
    }
    return now - start;
}

void brim_rt_time_sleep_millis(int64_t millis) {
    fflush(stdout);
    if (millis <= 0) {
        return;
    }
#ifdef _WIN32
    Sleep((DWORD)millis);
#else
    struct timespec t;
    t.tv_sec = (time_t)(millis / 1000);
    t.tv_nsec = (long)((millis % 1000) * 1000000);
    while (nanosleep(&t, &t) != 0 && errno == EINTR) {
    }
#endif
}

/* ---- f64 (format, parse) --------------------------------------------------- */
/* The same text as brim-runtime's float.rs: the fewest digits that read back
 * exactly, decimals from 1e-5 up to 1e16, scientific notation otherwise. */

static void format_f64(double x, Buffer *b) {
    if (x != x) {
        buffer_add(b, "NaN", 3);
        return;
    }
    if (signbit(x)) {
        buffer_add(b, "-", 1);
        x = -x;
    }
    if (isinf(x)) {
        buffer_add(b, "inf", 3);
        return;
    }
    if (x == 0.0) {
        buffer_add(b, "0.0", 3);
        return;
    }
    /* The shortest precision whose digits read back as x. */
    char sci[40];
    for (int p = 0; p <= 17; p++) {
        snprintf(sci, sizeof sci, "%.*e", p, x);
        if (strtod(sci, NULL) == x) {
            break;
        }
    }
    char digits[24];
    int n = 0;
    const char *c = sci;
    for (; *c != 'e'; c++) {
        if (*c >= '0' && *c <= '9') {
            digits[n++] = *c;
        }
    }
    int exp = atoi(c + 1);
    if (exp >= -5 && exp < 16) {
        if (exp >= 0) {
            int int_len = exp + 1;
            if (n <= int_len) {
                buffer_add(b, digits, n);
                for (int i = n; i < int_len; i++) {
                    buffer_add(b, "0", 1);
                }
                buffer_add(b, ".0", 2);
            } else {
                buffer_add(b, digits, int_len);
                buffer_add(b, ".", 1);
                buffer_add(b, digits + int_len, n - int_len);
            }
        } else {
            buffer_add(b, "0.", 2);
            for (int i = 0; i < -exp - 1; i++) {
                buffer_add(b, "0", 1);
            }
            buffer_add(b, digits, n);
        }
    } else {
        buffer_add(b, digits, 1);
        if (n > 1) {
            buffer_add(b, ".", 1);
            buffer_add(b, digits + 1, n - 1);
        }
        char e[16];
        int len = snprintf(e, sizeof e, "e%d", exp);
        buffer_add(b, e, len);
    }
}

BrimStr *brim_rt_f64_to_str(double x) {
    Buffer b = {0};
    format_f64(x, &b);
    return buffer_finish(&b);
}

void brim_rt_print_f64(double x) {
    Buffer b = {0};
    format_f64(x, &b);
    buffer_add(&b, "\n", 1);
    fwrite(b.data, 1, (size_t)b.len, stdout);
    free(b.data);
}

static int is_float_text(const BrimStr *s) {
    int64_t i = 0, n = s->len;
    const char *d = s->data;
    int64_t start;
    if (i < n && (d[i] == '+' || d[i] == '-')) i++;
    start = i;
    while (i < n && d[i] >= '0' && d[i] <= '9') i++;
    if (i == start) return 0;
    if (i < n && d[i] == '.') {
        i++;
        start = i;
        while (i < n && d[i] >= '0' && d[i] <= '9') i++;
        if (i == start) return 0;
    }
    if (i < n && (d[i] == 'e' || d[i] == 'E')) {
        i++;
        if (i < n && (d[i] == '+' || d[i] == '-')) i++;
        start = i;
        while (i < n && d[i] >= '0' && d[i] <= '9') i++;
        if (i == start) return 0;
    }
    return i == n;
}

uint8_t brim_rt_str_parse_float_ok(const BrimStr *s) {
    return (uint8_t)is_float_text(s);
}

double brim_rt_str_parse_float(const BrimStr *s) {
    if (!is_float_text(s)) {
        return 0.0;
    }
    char *text = c_string(s);
    double x = strtod(text, NULL);
    free(text);
    return x;
}

double brim_rt_i64_to_f64(int64_t value) {
    return (double)value;
}

/* `to_int(x)`: rounds toward zero; NaN and numbers outside i64 stop the
 * program (run-time error R0010). */
int64_t brim_rt_abs_i64(int64_t x, const char *head, const char *sep, const char *note) {
    if (x == INT64_MIN) {
        brim_rt_trap(head, sep, note);
    }
    return x < 0 ? -x : x;
}

double brim_rt_abs_f64(double x) {
    return fabs(x);
}

double brim_rt_sqrt(double x) {
    return sqrt(x);
}

double brim_rt_floor(double x) {
    return floor(x);
}

double brim_rt_ceil(double x) {
    return ceil(x);
}

double brim_rt_round(double x) {
    return round(x);
}

double brim_rt_pow_f64(double base, double exp) {
    return pow(base, exp);
}

/* `pow` on integers; see `brim_runtime::ops::pow`. */
int64_t brim_rt_pow_i64(int64_t base, int64_t exp, const char *head, const char *sep,
                        const char *note) {
    if (exp < 0) {
        switch (base) {
        case 0:
            brim_rt_trap(head, sep, note);
            return 0;
        case 1:
            return 1;
        case -1:
            return exp % 2 == 0 ? 1 : -1;
        default:
            return 0;
        }
    }
    /* Squaring, as Rust's `checked_pow`: once the square overflows while
       bits of the exponent remain, the result would too. */
    int64_t result = 1;
    while (exp > 0) {
        if ((exp & 1) && __builtin_mul_overflow(result, base, &result)) {
            brim_rt_trap(head, sep, note);
        }
        exp >>= 1;
        if (exp > 0 && __builtin_mul_overflow(base, base, &base)) {
            brim_rt_trap(head, sep, note);
        }
    }
    return result;
}

int64_t brim_rt_f64_to_i64(double x, const char *head, const char *sep, const char *note) {
    if (!(x > -9223372036854775809.0 && x < 9223372036854775808.0)) {
        brim_rt_trap(head, sep, note);
    }
    return (int64_t)x;
}

/* ---- process entry ------------------------------------------------------- */

#ifdef _WIN32
/* The arguments as UTF-8 (`argv` is in the ANSI code page). */
static char **utf8_arguments(int *argc) {
    LPWSTR *wide = CommandLineToArgvW(GetCommandLineW(), argc);
    char **out = malloc(sizeof(char *) * (size_t)(*argc + 1));
    if (wide == NULL || out == NULL) {
        fatal("cannot read the command line");
    }
    for (int i = 0; i < *argc; i++) {
        int n = WideCharToMultiByte(CP_UTF8, 0, wide[i], -1, NULL, 0, NULL, NULL);
        out[i] = malloc((size_t)n);
        if (out[i] == NULL) {
            fatal("out of memory");
        }
        WideCharToMultiByte(CP_UTF8, 0, wide[i], -1, out[i], n, NULL, NULL);
    }
    out[*argc] = NULL;
    LocalFree(wide);
    return out;
}
#endif

int main(int argc, char **argv) {
#ifdef _WIN32
    /* Bytes as they are (no `\r\n`), and UTF-8 text in the console. */
    _setmode(_fileno(stdin), _O_BINARY);
    _setmode(_fileno(stdout), _O_BINARY);
    _setmode(_fileno(stderr), _O_BINARY);
    SetConsoleOutputCP(CP_UTF8);
    argv = utf8_arguments(&argc);
#endif
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
    set_process_output(0, NULL, NULL);
    if (checking && live_strings != 0) {
        fprintf(stderr, "error: brim runtime check: %" PRId64 " string(s) or list(s) leaked\n",
                live_strings);
        return 102;
    }
    return 0;
}
