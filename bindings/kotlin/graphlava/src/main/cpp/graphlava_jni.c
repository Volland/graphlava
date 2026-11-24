/* JNI shim between org.graphlava.Native and the C FFI (graphlava.h),
 * compiled into the same shared library as libgraphlava_ffi.a
 * ([[16-api#Mobile bindings]], design D1). Every function is a thin
 * forward: handles cross as jlong, statuses as jint, and the message of a
 * failing call is read on the same thread with Native.lastError(). Listing
 * functions return a Java array, or NULL with the status in
 * Native.lastStatus().
 *
 * @tg: implements:: [[openspec:mobile-bindings#Kotlin API]]
 */
#include <jni.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "graphlava.h"

#define FN(name) JNICALL Java_org_graphlava_Native_##name
#define H(T, x) ((T *)(uintptr_t)(x))
#define J(p) ((jlong)(uintptr_t)(p))

static _Thread_local jint last_status = 0;

static jint st(enum graphlava_status s) {
    last_status = (jint)s;
    return (jint)s;
}

/* A Java string as UTF-8 for the duration of a call; NULL stays NULL. */
static const char *utf(JNIEnv *env, jstring s) {
    return s ? (*env)->GetStringUTFChars(env, s, NULL) : NULL;
}
static void unutf(JNIEnv *env, jstring s, const char *c) {
    if (s && c) (*env)->ReleaseStringUTFChars(env, s, c);
}

static void put_long(JNIEnv *env, jlongArray out, jsize i, jlong v) {
    if (out) (*env)->SetLongArrayRegion(env, out, i, 1, &v);
}

static jlongArray longs(JNIEnv *env, const uint64_t *v, size_t n) {
    jlongArray a = (*env)->NewLongArray(env, (jsize)n);
    if (a && n) (*env)->SetLongArrayRegion(env, a, 0, (jsize)n, (const jlong *)v);
    return a;
}

static jstring jstr(JNIEnv *env, const char *s) { return s ? (*env)->NewStringUTF(env, s) : NULL; }

JNIEXPORT jint FN(lastStatus)(JNIEnv *env, jclass c) {
    (void)env; (void)c;
    return last_status;
}

JNIEXPORT jstring FN(lastError)(JNIEnv *env, jclass c) {
    (void)c;
    char *m = NULL;
    graphlava_last_error(&m);
    jstring s = jstr(env, m);
    graphlava_string_free(m);
    return s;
}

JNIEXPORT jstring FN(version)(JNIEnv *env, jclass c) {
    (void)c;
    return jstr(env, graphlava_version());
}

JNIEXPORT jint FN(abiVersion)(JNIEnv *env, jclass c) {
    (void)env; (void)c;
    uint32_t v = 0;
    graphlava_abi_version(&v);
    return (jint)v;
}

JNIEXPORT jint FN(headerAbiVersion)(JNIEnv *env, jclass c) {
    (void)env; (void)c;
    return GRAPHLAVA_ABI_VERSION;
}

/* ---- lifecycle ------------------------------------------------------------- */

JNIEXPORT jint FN(open)(JNIEnv *env, jclass c, jstring path, jint flags, jlongArray out) {
    (void)c;
    const char *p = utf(env, path);
    graphlava_db *db = NULL;
    jint s = st(graphlava_open_ex(GRAPHLAVA_ABI_VERSION, p, (uint32_t)flags, &db));
    unutf(env, path, p);
    put_long(env, out, 0, J(db));
    return s;
}

JNIEXPORT jint FN(close)(JNIEnv *env, jclass c, jlong db) {
    (void)env; (void)c;
    return st(graphlava_close(H(graphlava_db, db)));
}

JNIEXPORT jint FN(suspendDb)(JNIEnv *env, jclass c, jlong db) {
    (void)env; (void)c;
    return st(graphlava_suspend(H(graphlava_db, db)));
}

JNIEXPORT jint FN(resumeDb)(JNIEnv *env, jclass c, jlong db) {
    (void)env; (void)c;
    return st(graphlava_resume(H(graphlava_db, db)));
}

JNIEXPORT jint FN(flush)(JNIEnv *env, jclass c, jlong db) {
    (void)env; (void)c;
    return st(graphlava_flush(H(graphlava_db, db)));
}

JNIEXPORT jint FN(openSnapshots)(JNIEnv *env, jclass c, jlong db, jlongArray out) {
    (void)c;
    size_t n = 0;
    jint s = st(graphlava_open_snapshots(H(graphlava_db, db), &n));
    put_long(env, out, 0, (jlong)n);
    return s;
}

/* ---- writes and requests ---------------------------------------------------- */

JNIEXPORT jint FN(sparqlUpdate)(JNIEnv *env, jclass c, jlong db, jstring text, jint d) {
    (void)c;
    const char *t = utf(env, text);
    jint s = st(graphlava_sparql_update(H(graphlava_db, db), t, (uint32_t)d));
    unutf(env, text, t);
    return s;
}

JNIEXPORT jint FN(requestNew)(JNIEnv *env, jclass c, jlongArray out) {
    (void)c;
    graphlava_request *r = NULL;
    jint s = st(graphlava_request_new(&r));
    put_long(env, out, 0, J(r));
    return s;
}

JNIEXPORT jint FN(requestNewText)(JNIEnv *env, jclass c, jint lang, jstring text, jlongArray out) {
    (void)c;
    const char *t = utf(env, text);
    graphlava_request *r = NULL;
    jint s = st(graphlava_request_new_text((uint32_t)lang, t, &r));
    unutf(env, text, t);
    put_long(env, out, 0, J(r));
    return s;
}

/* Arguments arrive as parallel arrays: kinds, ids, values, datatypes, langs. */
JNIEXPORT jint FN(requestAdd)(JNIEnv *env, jclass c, jlong req, jint op, jintArray kinds, jlongArray ids,
                              jobjectArray values, jobjectArray datatypes, jobjectArray langs) {
    (void)c;
    jsize n = kinds ? (*env)->GetArrayLength(env, kinds) : 0;
    graphlava_arg *args = calloc((size_t)(n ? n : 1), sizeof *args);
    jstring *js = calloc((size_t)(n ? n : 1) * 3, sizeof *js);
    if (!args || !js) {
        free(args);
        free(js);
        return st(GRAPHLAVA_ERR_INTERNAL);
    }
    jint *k = n ? (*env)->GetIntArrayElements(env, kinds, NULL) : NULL;
    jlong *v = n ? (*env)->GetLongArrayElements(env, ids, NULL) : NULL;
    for (jsize i = 0; i < n; i++) {
        js[3 * i] = (jstring)(*env)->GetObjectArrayElement(env, values, i);
        js[3 * i + 1] = (jstring)(*env)->GetObjectArrayElement(env, datatypes, i);
        js[3 * i + 2] = (jstring)(*env)->GetObjectArrayElement(env, langs, i);
        args[i].kind = (uint32_t)k[i];
        args[i].id = (uint64_t)v[i];
        args[i].value = utf(env, js[3 * i]);
        args[i].datatype = utf(env, js[3 * i + 1]);
        args[i].lang = utf(env, js[3 * i + 2]);
    }
    jint s = st(graphlava_request_add(H(graphlava_request, req), (uint32_t)op, args, (size_t)n));
    for (jsize i = 0; i < n; i++) {
        unutf(env, js[3 * i], args[i].value);
        unutf(env, js[3 * i + 1], args[i].datatype);
        unutf(env, js[3 * i + 2], args[i].lang);
        for (int j = 0; j < 3; j++)
            if (js[3 * i + j]) (*env)->DeleteLocalRef(env, js[3 * i + j]);
    }
    if (k) (*env)->ReleaseIntArrayElements(env, kinds, k, JNI_ABORT);
    if (v) (*env)->ReleaseLongArrayElements(env, ids, v, JNI_ABORT);
    free(args);
    free(js);
    return s;
}

JNIEXPORT void FN(requestFree)(JNIEnv *env, jclass c, jlong req) {
    (void)env; (void)c;
    graphlava_request_free(H(graphlava_request, req));
}

JNIEXPORT jint FN(submit)(JNIEnv *env, jclass c, jlong db, jlong req, jint d, jlongArray out) {
    (void)c;
    graphlava_result *r = NULL;
    jint s = st(graphlava_submit(H(graphlava_db, db), H(graphlava_request, req), (uint32_t)d, &r));
    put_long(env, out, 0, J(r));
    return s;
}

/* All results as [kind0, value0, kind1, value1, ...], or NULL. */
JNIEXPORT jlongArray FN(resultAll)(JNIEnv *env, jclass c, jlong res) {
    (void)c;
    graphlava_result *r = H(graphlava_result, res);
    size_t n = 0;
    if (st(graphlava_result_len(r, &n)) != GRAPHLAVA_OK) return NULL;
    uint64_t *v = calloc(2 * n + 1, sizeof *v);
    if (!v) return NULL;
    for (size_t i = 0; i < n; i++) {
        graphlava_result_kind k = 0;
        if (st(graphlava_result_get(r, i, &k, &v[2 * i + 1])) != GRAPHLAVA_OK) {
            free(v);
            return NULL;
        }
        v[2 * i] = k;
    }
    jlongArray a = longs(env, v, 2 * n);
    free(v);
    return a;
}

/* Two-call buffer convention of graphlava.h, folded into one Java call. */
#define LIST(call_with_buf_cap_n, width)                                   \
    do {                                                                   \
        size_t n = 0, cap = 0;                                             \
        uint64_t *buf = NULL;                                              \
        for (;;) {                                                         \
            enum graphlava_status s_ = call_with_buf_cap_n;                \
            if (st(s_) != GRAPHLAVA_OK) {                                  \
                free(buf);                                                 \
                return NULL;                                               \
            }                                                              \
            if (n <= cap) break;                                           \
            free(buf);                                                     \
            cap = n;                                                       \
            buf = calloc(cap * (width), sizeof *buf);                      \
            if (!buf) return NULL;                                         \
        }                                                                  \
        jlongArray a_ = longs(env, buf, n * (width));                      \
        free(buf);                                                         \
        return a_;                                                         \
    } while (0)

JNIEXPORT jlongArray FN(resultDeleted)(JNIEnv *env, jclass c, jlong res, jint i) {
    (void)c;
    LIST(graphlava_result_deleted(H(graphlava_result, res), (size_t)i, buf, cap, &n), 1);
}

JNIEXPORT void FN(resultFree)(JNIEnv *env, jclass c, jlong res) {
    (void)env; (void)c;
    graphlava_result_free(H(graphlava_result, res));
}

/* ---- transactions ----------------------------------------------------------- */

JNIEXPORT jint FN(readBegin)(JNIEnv *env, jclass c, jlong db, jlongArray out) {
    (void)c;
    graphlava_read *r = NULL;
    jint s = st(graphlava_read_begin(H(graphlava_db, db), &r));
    put_long(env, out, 0, J(r));
    return s;
}

JNIEXPORT void FN(readEnd)(JNIEnv *env, jclass c, jlong r) {
    (void)env; (void)c;
    graphlava_read_end(H(graphlava_read, r));
}

JNIEXPORT jint FN(writeBegin)(JNIEnv *env, jclass c, jlong db, jint d, jlongArray out) {
    (void)c;
    graphlava_write *w = NULL;
    jint s = st(graphlava_write_begin(H(graphlava_db, db), (uint32_t)d, &w));
    put_long(env, out, 0, J(w));
    return s;
}

JNIEXPORT jint FN(writeRun)(JNIEnv *env, jclass c, jlong w, jlong req, jlongArray out) {
    (void)c;
    graphlava_result *r = NULL;
    jint s = st(graphlava_write_run(H(graphlava_write, w), H(graphlava_request, req), &r));
    put_long(env, out, 0, J(r));
    return s;
}

JNIEXPORT jint FN(commit)(JNIEnv *env, jclass c, jlong w) {
    (void)env; (void)c;
    return st(graphlava_commit(H(graphlava_write, w)));
}

JNIEXPORT jint FN(abort)(JNIEnv *env, jclass c, jlong w) {
    (void)env; (void)c;
    return st(graphlava_abort(H(graphlava_write, w)));
}

/* ---- core reads ------------------------------------------------------------- */

JNIEXPORT jint FN(lookup)(JNIEnv *env, jclass c, jlong r, jint kind, jstring value, jstring dt, jstring lang,
                          jlongArray out) {
    (void)c;
    graphlava_arg a = {(uint32_t)kind, 0, utf(env, value), utf(env, dt), utf(env, lang)};
    uint64_t id = 0;
    jint s = st(graphlava_lookup(H(graphlava_read, r), &a, &id));
    unutf(env, value, a.value);
    unutf(env, dt, a.datatype);
    unutf(env, lang, a.lang);
    put_long(env, out, 0, (jlong)id);
    return s;
}

JNIEXPORT jint FN(graph)(JNIEnv *env, jclass c, jlong r, jstring iri, jlongArray out) {
    (void)c;
    graphlava_arg a = {GRAPHLAVA_TERM_IRI, 0, utf(env, iri), NULL, NULL};
    uint64_t id = 0;
    jint s = st(graphlava_graph(H(graphlava_read, r), &a, &id));
    unutf(env, iri, a.value);
    put_long(env, out, 0, (jlong)id);
    return s;
}

static jint term_out(JNIEnv *env, enum graphlava_status s, graphlava_term *t, jobjectArray out) {
    if (s == GRAPHLAVA_OK) {
        static const char *kinds[] = {"unbound", "iri", "blank", "literal", "triple"};
        const char *k = t->kind < 5 ? kinds[t->kind] : "unbound";
        (*env)->SetObjectArrayElement(env, out, 0, jstr(env, k));
        (*env)->SetObjectArrayElement(env, out, 1, jstr(env, t->value));
        (*env)->SetObjectArrayElement(env, out, 2, jstr(env, t->lang ? NULL : t->datatype));
        (*env)->SetObjectArrayElement(env, out, 3, jstr(env, t->lang));
    }
    graphlava_term_free(t);
    return st(s);
}

/* out = [kind, value, datatype, lang] */
JNIEXPORT jint FN(decode)(JNIEnv *env, jclass c, jlong r, jlong id, jobjectArray out) {
    (void)c;
    graphlava_term t;
    return term_out(env, graphlava_decode(H(graphlava_read, r), (uint64_t)id, &t), &t, out);
}

JNIEXPORT jlongArray FN(find)(JNIEnv *env, jclass c, jlong r, jlong s, jlong p, jlong o) {
    (void)c;
    LIST(graphlava_find(H(graphlava_read, r), (uint64_t)s, (uint64_t)p, (uint64_t)o, buf, cap, &n), 4);
}

JNIEXPORT jlongArray FN(graphsOf)(JNIEnv *env, jclass c, jlong r, jlong item) {
    (void)c;
    LIST(graphlava_graphs_of(H(graphlava_read, r), (uint64_t)item, buf, cap, &n), 1);
}

JNIEXPORT jlongArray FN(scope)(JNIEnv *env, jclass c, jlong r, jlong g, jboolean direct) {
    (void)c;
    LIST(graphlava_scope(H(graphlava_read, r), (uint64_t)g, direct ? 1 : 0, buf, cap, &n), 1);
}

/* [type, graph, role0, filler0, ...], or NULL (NOT_FOUND in lastStatus). */
JNIEXPORT jlongArray FN(hedge)(JNIEnv *env, jclass c, jlong r, jlong e) {
    (void)c;
    uint64_t ty = 0, g = 0;
    size_t n = 0, cap = 0;
    uint64_t *buf = NULL;
    for (;;) {
        if (st(graphlava_hedge(H(graphlava_read, r), (uint64_t)e, &ty, &g, buf, cap, &n)) != GRAPHLAVA_OK) {
            free(buf);
            return NULL;
        }
        if (n <= cap) break;
        free(buf);
        cap = n;
        buf = calloc(2 * cap, sizeof *buf);
        if (!buf) return NULL;
    }
    uint64_t *all = calloc(2 * n + 2, sizeof *all);
    if (!all) {
        free(buf);
        return NULL;
    }
    all[0] = ty;
    all[1] = g;
    if (n) memcpy(all + 2, buf, 2 * n * sizeof *all);
    jlongArray a = longs(env, all, 2 * n + 2);
    free(buf);
    free(all);
    return a;
}

/* ---- queries ---------------------------------------------------------------- */

JNIEXPORT jint FN(query)(JNIEnv *env, jclass c, jlong db, jint lang, jstring text, jlongArray out) {
    (void)c;
    const char *t = utf(env, text);
    graphlava_prepared *p = NULL;
    graphlava_rows *rows = NULL;
    jint s = st(graphlava_prepare(H(graphlava_db, db), (uint32_t)lang, t, &p));
    unutf(env, text, t);
    if (s == GRAPHLAVA_OK) s = st(graphlava_query(H(graphlava_db, db), p, &rows));
    graphlava_prepared_free(p);
    put_long(env, out, 0, J(rows));
    return s;
}

JNIEXPORT jobjectArray FN(rowsColumns)(JNIEnv *env, jclass c, jlong rows) {
    (void)c;
    size_t n = 0;
    if (st(graphlava_rows_columns(H(graphlava_rows, rows), &n)) != GRAPHLAVA_OK) return NULL;
    jclass sc = (*env)->FindClass(env, "java/lang/String");
    jobjectArray a = (*env)->NewObjectArray(env, (jsize)n, sc, NULL);
    for (size_t i = 0; a && i < n; i++) {
        char *name = NULL;
        if (st(graphlava_rows_column_name(H(graphlava_rows, rows), i, &name)) != GRAPHLAVA_OK) return NULL;
        jstring s = jstr(env, name);
        (*env)->SetObjectArrayElement(env, a, (jsize)i, s);
        (*env)->DeleteLocalRef(env, s);
        graphlava_string_free(name);
    }
    return a;
}

/* [nrows, ncols, ids (column-major)...], nrows 0 at the end; NULL on error. */
JNIEXPORT jlongArray FN(rowsNextBatch)(JNIEnv *env, jclass c, jlong rows) {
    (void)c;
    const uint64_t *ids = NULL;
    size_t nr = 0, nc = 0;
    if (st(graphlava_rows_next_batch(H(graphlava_rows, rows), &ids, &nr, &nc)) != GRAPHLAVA_OK) return NULL;
    jlongArray a = (*env)->NewLongArray(env, (jsize)(2 + nr * nc));
    if (!a) return NULL;
    jlong head[2] = {(jlong)nr, (jlong)nc};
    (*env)->SetLongArrayRegion(env, a, 0, 2, head);
    if (nr * nc) (*env)->SetLongArrayRegion(env, a, 2, (jsize)(nr * nc), (const jlong *)ids);
    return a;
}

JNIEXPORT jint FN(rowsDecode)(JNIEnv *env, jclass c, jlong rows, jlong id, jobjectArray out) {
    (void)c;
    graphlava_term t;
    return term_out(env, graphlava_term_decode(H(graphlava_rows, rows), (uint64_t)id, &t), &t, out);
}

JNIEXPORT void FN(rowsFree)(JNIEnv *env, jclass c, jlong rows) {
    (void)env; (void)c;
    graphlava_rows_free(H(graphlava_rows, rows));
}
