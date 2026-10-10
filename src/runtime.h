/* sloplang runtime v0. Embedded verbatim at the top of every generated C file. */
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <setjmp.h>
#include <time.h>
#include <unistd.h>

typedef uint8_t sl_unit;
typedef struct { void *p; int64_t n; } sl_buf;

static jmp_buf *sl_test_jb;
static pthread_t sl_test_thread;
static char sl_trap_msg[1024];

__attribute__((noreturn, cold, noinline))
static void sl_trap(const char *code, const char *msg, int line, int col) {
    char b[1024];
    snprintf(b, sizeof b, "\"code\":\"%s\",\"msg\":\"%s\",\"file\":\"%s\",\"line\":%d,\"col\":%d", code, msg, SL_FILE, line, col);
    if (sl_test_jb && pthread_equal(pthread_self(), sl_test_thread)) {
        memcpy(sl_trap_msg, b, sizeof b);
        longjmp(*sl_test_jb, 1);
    }
    fflush(stdout);
    fprintf(stderr, "{\"kind\":\"trap\",%s}\n", b);
    exit(101);
}

static void *sl_malloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (!p) { fprintf(stderr, "{\"kind\":\"trap\",\"code\":\"T0010\",\"msg\":\"out of memory\"}\n"); exit(101); }
    return p;
}

/* ---- checked integer arithmetic ---- */
#ifdef SL_NOCHECK
#define SL_ARITH(T, N, MIN) \
static inline T sl_add_##N(T a, T b, int l, int c) { (void)l; (void)c; return (T)((uint64_t)a + (uint64_t)b); } \
static inline T sl_sub_##N(T a, T b, int l, int c) { (void)l; (void)c; return (T)((uint64_t)a - (uint64_t)b); } \
static inline T sl_mul_##N(T a, T b, int l, int c) { (void)l; (void)c; return (T)((uint64_t)a * (uint64_t)b); } \
static inline T sl_div_##N(T a, T b, int l, int c) { (void)l; (void)c; return a / b; } \
static inline T sl_rem_##N(T a, T b, int l, int c) { (void)l; (void)c; return a % b; } \
static inline T sl_shl_##N(T a, T b, int l, int c) { (void)l; (void)c; return (T)((uint64_t)a << (b & (8 * sizeof(T) - 1))); } \
static inline T sl_shr_##N(T a, T b, int l, int c) { (void)l; (void)c; return a >> (b & (8 * sizeof(T) - 1)); }
#else
#define SL_ARITH(T, N, MIN) \
static inline T sl_add_##N(T a, T b, int l, int c) { T r; if (__builtin_add_overflow(a, b, &r)) sl_trap("T0001", "integer overflow in +", l, c); return r; } \
static inline T sl_sub_##N(T a, T b, int l, int c) { T r; if (__builtin_sub_overflow(a, b, &r)) sl_trap("T0001", "integer overflow in -", l, c); return r; } \
static inline T sl_mul_##N(T a, T b, int l, int c) { T r; if (__builtin_mul_overflow(a, b, &r)) sl_trap("T0001", "integer overflow in *", l, c); return r; } \
static inline T sl_div_##N(T a, T b, int l, int c) { if (b == 0) sl_trap("T0002", "division by zero", l, c); \
    if ((T)-1 < 0 && b == (T)-1 && a == (T)(MIN)) sl_trap("T0001", "integer overflow in /", l, c); return a / b; } \
static inline T sl_rem_##N(T a, T b, int l, int c) { if (b == 0) sl_trap("T0002", "division by zero", l, c); \
    if ((T)-1 < 0 && b == (T)-1) return 0; return a % b; } \
static inline T sl_shl_##N(T a, T b, int l, int c) { if (b < 0 || (uint64_t)b >= 8 * sizeof(T)) sl_trap("T0009", "shift out of range", l, c); return (T)((uint64_t)a << b); } \
static inline T sl_shr_##N(T a, T b, int l, int c) { if (b < 0 || (uint64_t)b >= 8 * sizeof(T)) sl_trap("T0009", "shift out of range", l, c); return a >> b; }
#endif
#define SL_WRAP(T, N) \
static inline T sl_addw_##N(T a, T b) { return (T)((uint64_t)a + (uint64_t)b); } \
static inline T sl_subw_##N(T a, T b) { return (T)((uint64_t)a - (uint64_t)b); } \
static inline T sl_mulw_##N(T a, T b) { return (T)((uint64_t)a * (uint64_t)b); }

SL_ARITH(int8_t, i8, INT8_MIN) SL_ARITH(int16_t, i16, INT16_MIN) SL_ARITH(int32_t, i32, INT32_MIN) SL_ARITH(int64_t, i64, INT64_MIN)
SL_ARITH(uint8_t, u8, 0) SL_ARITH(uint16_t, u16, 0) SL_ARITH(uint32_t, u32, 0) SL_ARITH(uint64_t, u64, 0)
SL_WRAP(int8_t, i8) SL_WRAP(int16_t, i16) SL_WRAP(int32_t, i32) SL_WRAP(int64_t, i64)
SL_WRAP(uint8_t, u8) SL_WRAP(uint16_t, u16) SL_WRAP(uint32_t, u32) SL_WRAP(uint64_t, u64)

/* Range-checked integer conversion; v is widened to __int128 first. */
#ifdef SL_NOCHECK
#define SL_CAST(T, v, lo, hi, l, c) ((T)(v))
#define SL_FCAST(T, v, lo, hi, l, c) ((T)(v))
#else
#define SL_CAST(T, v, lo, hi, l, c) ({ __int128 _v = (__int128)(v); \
    if (_v < (__int128)(lo) || _v > (__int128)(hi)) sl_trap("T0011", "integer cast out of range", l, c); (T)_v; })
#define SL_FCAST(T, v, lo, hi, l, c) ({ double _v = (double)(v); \
    if (!(_v >= (double)(lo) && _v < (double)(hi) + 1.0)) sl_trap("T0011", "float to integer cast out of range", l, c); (T)_v; })
#endif

/* ---- buffers ---- */
static sl_buf sl_alloc(int64_t n, size_t esz, int l, int c) {
    if (n < 0 || (n > 0 && (uint64_t)n > SIZE_MAX / esz)) sl_trap("T0010", "invalid allocation size", l, c);
    sl_buf b; b.p = calloc((size_t)n ? (size_t)n : 1, esz); b.n = n;
    if (!b.p) sl_trap("T0010", "out of memory", l, c);
    return b;
}
#ifdef SL_NOCHECK
#define sl_idx(b, i, l, c) (i)
#else
static inline int64_t sl_idx(sl_buf b, int64_t i, int l, int c) {
    if (__builtin_expect(i < 0 || i >= b.n, 0)) sl_trap("T0003", "index out of bounds", l, c);
    return i;
}
#endif

/* ---- printing ---- */
static void sl_p_f64(double d) {
    char b[40];
    snprintf(b, sizeof b, "%.15g", d);
    if (strtod(b, NULL) != d) snprintf(b, sizeof b, "%.16g", d);
    if (strtod(b, NULL) != d) snprintf(b, sizeof b, "%.17g", d);
    if (!strpbrk(b, ".einn")) strcat(b, ".0");
    fputs(b, stdout);
}

static int64_t sl_clock(void) {
    struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000 + ts.tv_nsec;
}

/* ---- tasks and groups ---- */
typedef struct { pthread_t th; void *data; } sl_task;

static sl_task *sl_task_start(void *(*fn)(void *), void *data) {
    sl_task *t = sl_malloc(sizeof *t);
    t->data = data;
    if (pthread_create(&t->th, NULL, fn, data) != 0) { fn(data); t->th = (pthread_t)0; }
    return t;
}
static void sl_task_join(sl_task *t) { if (t->th) pthread_join(t->th, NULL); }

typedef struct { pthread_mutex_t m; sl_task **ts; int64_t n, cap; size_t rsz; } sl_group;

static sl_group *sl_group_new(size_t rsz) {
    sl_group *g = sl_malloc(sizeof *g);
    pthread_mutex_init(&g->m, NULL);
    g->ts = NULL; g->n = 0; g->cap = 0; g->rsz = rsz;
    return g;
}
static void sl_group_add(sl_group *g, sl_task *t) {
    pthread_mutex_lock(&g->m);
    if (g->n == g->cap) { g->cap = g->cap ? g->cap * 2 : 8; g->ts = realloc(g->ts, sizeof(sl_task *) * g->cap); }
    g->ts[g->n++] = t;
    pthread_mutex_unlock(&g->m);
}
/* Joins every task in spawn order; results land in a fresh buffer in the same order. */
static sl_buf sl_group_wait(sl_group *g) {
    sl_buf out = sl_alloc(g->n, g->rsz, 0, 0);
    for (int64_t i = 0; i < g->n; i++) {
        sl_task_join(g->ts[i]);
        memcpy((char *)out.p + i * g->rsz, g->ts[i]->data, g->rsz);
        free(g->ts[i]->data); free(g->ts[i]);
    }
    free(g->ts); pthread_mutex_destroy(&g->m); free(g);
    return out;
}

/* ---- parallel for: [lo, hi) split into contiguous chunks, one per worker ---- */
typedef void (*sl_pfor_fn)(void *env, int64_t lo, int64_t hi);
typedef struct { sl_pfor_fn fn; void *env; int64_t lo, hi; } sl_pfor_job;
static void *sl_pfor_run(void *p) { sl_pfor_job *j = p; j->fn(j->env, j->lo, j->hi); return NULL; }
static int64_t sl_workers(void) {
    const char *e = getenv("SLOP_THREADS");
    long n = e ? atol(e) : sysconf(_SC_NPROCESSORS_ONLN);
    return n < 1 ? 1 : n > 256 ? 256 : n;
}
static void sl_pfor(sl_pfor_fn fn, void *env, int64_t lo, int64_t hi) {
    if (hi <= lo) return;
    int64_t n = hi - lo, w = sl_workers();
    if (w > n) w = n;
    if (w == 1) { fn(env, lo, hi); return; }
    pthread_t th[256]; sl_pfor_job jobs[256];
    int64_t chunk = n / w, extra = n % w, at = lo;
    for (int64_t k = 0; k < w; k++) {
        int64_t sz = chunk + (k < extra);
        jobs[k] = (sl_pfor_job){ fn, env, at, at + sz };
        at += sz;
        if (k > 0) pthread_create(&th[k], NULL, sl_pfor_run, &jobs[k]);
    }
    sl_pfor_run(&jobs[0]);
    for (int64_t k = 1; k < w; k++) pthread_join(th[k], NULL);
}

/* ---- channels: bounded MPMC ring buffer ---- */
typedef struct {
    pthread_mutex_t m; pthread_cond_t not_empty, not_full;
    char *buf; int64_t cap, head, count; size_t esz; int closed;
} sl_chan;

static sl_chan *sl_chan_new(int64_t cap, size_t esz, int l, int c) {
    if (cap < 0) sl_trap("T0010", "negative channel capacity", l, c);
    if (cap == 0) cap = 1;
    sl_chan *ch = sl_malloc(sizeof *ch);
    pthread_mutex_init(&ch->m, NULL);
    pthread_cond_init(&ch->not_empty, NULL);
    pthread_cond_init(&ch->not_full, NULL);
    ch->buf = sl_malloc((size_t)cap * esz);
    ch->cap = cap; ch->head = 0; ch->count = 0; ch->esz = esz; ch->closed = 0;
    return ch;
}
static void sl_chan_send(sl_chan *ch, const void *v, int l, int c) {
    pthread_mutex_lock(&ch->m);
    while (ch->count == ch->cap && !ch->closed) pthread_cond_wait(&ch->not_full, &ch->m);
    if (ch->closed) { pthread_mutex_unlock(&ch->m); sl_trap("T0008", "send on closed channel", l, c); }
    memcpy(ch->buf + ((ch->head + ch->count) % ch->cap) * ch->esz, v, ch->esz);
    ch->count++;
    pthread_cond_signal(&ch->not_empty);
    pthread_mutex_unlock(&ch->m);
}
/* Returns 0 when the channel is closed and drained. */
static int sl_chan_recv(sl_chan *ch, void *out) {
    pthread_mutex_lock(&ch->m);
    while (ch->count == 0 && !ch->closed) pthread_cond_wait(&ch->not_empty, &ch->m);
    if (ch->count == 0) { pthread_mutex_unlock(&ch->m); return 0; }
    memcpy(out, ch->buf + ch->head * ch->esz, ch->esz);
    ch->head = (ch->head + 1) % ch->cap; ch->count--;
    pthread_cond_signal(&ch->not_full);
    pthread_mutex_unlock(&ch->m);
    return 1;
}
static void sl_chan_close(sl_chan *ch) {
    pthread_mutex_lock(&ch->m);
    ch->closed = 1;
    pthread_cond_broadcast(&ch->not_empty);
    pthread_cond_broadcast(&ch->not_full);
    pthread_mutex_unlock(&ch->m);
}

/* ---- property testing: deterministic xorshift seeded from the prop name ---- */
static uint64_t sl_rng_next(uint64_t *s) { uint64_t x = *s; x ^= x << 13; x ^= x >> 7; x ^= x << 17; return *s = x; }
static __int128 sl_rng_range(uint64_t *s, __int128 lo, __int128 hi) {
    unsigned __int128 span = (unsigned __int128)(hi - lo) + 1;
    unsigned __int128 r = ((unsigned __int128)sl_rng_next(s) << 64) | sl_rng_next(s);
    return lo + (__int128)(span ? r % span : r);
}
