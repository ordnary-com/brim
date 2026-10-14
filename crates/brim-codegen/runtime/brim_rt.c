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
 *   errors exactly as `brim run` does;
 * - the process entry point `main`, which calls the generated `brim_entry`.
 *
 * Strings are `BrimStr` objects. Their layout must match the constants the
 * backend emits for string literals: a reference count (-1 for static
 * objects), a byte length, then the UTF-8 bytes (not NUL-terminated).
 *
 * Memory management: heap strings are never freed yet. Reference counting is
 * planned (docs/architecture.md, "Memory management"); the `rc` field already
 * reserves room for it.
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
_Noreturn void brim_rt_trap(const char *head, const char *sep, const char *note) {
    fflush(stdout);
    fputs(head, stderr);
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

/* ---- strings ------------------------------------------------------------- */

static BrimStr *str_alloc(int64_t len) {
    BrimStr *s = malloc(sizeof(BrimStr) + (size_t)len);
    if (s == NULL) {
        fatal("out of memory");
    }
    s->rc = 1;
    s->len = len;
    return s;
}

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

/* ---- process entry ------------------------------------------------------- */

int main(void) {
    frames = malloc(sizeof(const char *) * (size_t)brim_max_call_depth);
    if (frames == NULL) {
        fatal("out of memory");
    }
    brim_entry();
    if (fflush(stdout) != 0 || ferror(stdout)) {
        fprintf(stderr, "error[R0004]: failed to write output: %s\n", strerror(errno));
        return 101;
    }
    return 0;
}
