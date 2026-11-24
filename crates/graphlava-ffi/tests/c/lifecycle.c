/* The agent-memory session through every function group of the C FFI
 * ([[16-api#C FFI]]): lifecycle with the ABI check, one-shot write requests
 * (graphlava_submit), an interactive write transaction, core reads on a read
 * snapshot, prepared queries in 1024-row batches with graphlava_term_decode,
 * suspend and resume, the extension journal, and administration. Then it
 * closes, reopens and checks that the session's statements are there.
 * Built and run under AddressSanitizer and LeakSanitizer in CI. Prints "ok".
 *
 * @tg: verifies:: [[openspec:mobile-bindings#Complete C FFI#C lifecycle under sanitizers]] {level: e2e}
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "graphlava.h"

#define EX "http://ex.org/"
#define P "PREFIX : <http://ex.org/>\n"

static int fail(const char *what, enum graphlava_status st) {
    char *msg = NULL;
    graphlava_last_error(&msg);
    fprintf(stderr, "%s: status %d: %s\n", what, (int)st, msg ? msg : "(none)");
    graphlava_string_free(msg);
    return 1;
}

#define CHECK(call)                                       \
    do {                                                  \
        enum graphlava_status st_ = (call);               \
        if (st_ != GRAPHLAVA_OK) return fail(#call, st_); \
    } while (0)

#define EXPECT(cond, what)                                      \
    do {                                                        \
        if (!(cond)) return fail(what, GRAPHLAVA_ERR_INTERNAL); \
    } while (0)

static graphlava_arg iri(const char *v) {
    graphlava_arg a = {GRAPHLAVA_TERM_IRI, 0, v, NULL, NULL};
    return a;
}
static graphlava_arg lit(const char *v, const char *dt) {
    graphlava_arg a = {GRAPHLAVA_TERM_LITERAL, 0, v, dt, NULL};
    return a;
}
static graphlava_arg id(uint64_t v) {
    graphlava_arg a = {GRAPHLAVA_ARG_ID, v, NULL, NULL, NULL};
    return a;
}

/* Counts the rows of a query and copies column `col` of the first row. */
static int query(graphlava_db *db, const char *q, size_t col, char *out, size_t cap, size_t *n) {
    graphlava_prepared *p = NULL;
    graphlava_rows *rows = NULL;
    CHECK(graphlava_prepare(db, GRAPHLAVA_LANG_SPARQL, q, &p));
    enum graphlava_status st = graphlava_query(db, p, &rows);
    graphlava_prepared_free(p);
    if (st != GRAPHLAVA_OK) return fail("graphlava_query", st);
    *n = 0;
    for (;;) {
        const uint64_t *ids = NULL;
        size_t nr = 0, nc = 0;
        st = graphlava_rows_next_batch(rows, &ids, &nr, &nc);
        if (st != GRAPHLAVA_OK) {
            graphlava_rows_free(rows);
            return fail("graphlava_rows_next_batch", st);
        }
        if (nr == 0) break;
        if (nr > GRAPHLAVA_BATCH_ROWS) return fail("batch too large", GRAPHLAVA_ERR_INTERNAL);
        if (out && *n == 0 && col < nc) {
            graphlava_term t;
            CHECK(graphlava_term_decode(rows, ids[col * nr + 0], &t));
            snprintf(out, cap, "%s", t.value ? t.value : "");
            graphlava_term_free(&t);
        }
        *n += nr;
    }
    graphlava_rows_free(rows);
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    const char *path = argv[1];
    char journal[1024], copy[1024], buf[256];
    snprintf(journal, sizeof journal, "%s.share.journal", path);
    snprintf(copy, sizeof copy, "%s.copy", path);
    remove(journal);
    remove(copy);

    uint32_t abi = 0;
    CHECK(graphlava_abi_version(&abi));
    EXPECT(abi == GRAPHLAVA_ABI_VERSION, "abi version");
    graphlava_db *db = NULL;
    CHECK(graphlava_open_ex(GRAPHLAVA_ABI_VERSION, path, GRAPHLAVA_OPEN_APP_GROUP, &db));

    /* One-shot write request: the agent remembers that Alice works at Acme,
     * with a confidence annotation on that statement. */
    graphlava_request *req = NULL;
    CHECK(graphlava_request_new(&req));
    graphlava_arg ins[] = {iri(EX "alice"), iri(EX "worksAt"), iri(EX "acme"), iri(EX "episode1")};
    CHECK(graphlava_request_add(req, GRAPHLAVA_OP_INSERT, ins, 4));
    graphlava_arg age[] = {iri(EX "alice"), iri(EX "age"),
                           lit("42", "http://www.w3.org/2001/XMLSchema#integer")};
    CHECK(graphlava_request_add(req, GRAPHLAVA_OP_INSERT, age, 3));
    /* A bad operation is refused and leaves the request unchanged. */
    EXPECT(graphlava_request_add(req, GRAPHLAVA_OP_DELETE, NULL, 0) == GRAPHLAVA_ERR_INVALID_ARGUMENT,
           "bad op refused");
    graphlava_result *res = NULL;
    CHECK(graphlava_submit(db, req, GRAPHLAVA_DURABILITY_ORDERED, &res));
    graphlava_request_free(req);
    size_t nres = 0;
    CHECK(graphlava_result_len(res, &nres));
    EXPECT(nres == 2, "two results");
    graphlava_result_kind kind = 0;
    uint64_t works = 0;
    CHECK(graphlava_result_get(res, 0, &kind, &works));
    EXPECT(kind == GRAPHLAVA_RESULT_ID && works != 0, "statement id");
    graphlava_result_free(res);

    CHECK(graphlava_request_new(&req));
    graphlava_arg ann[] = {id(works), iri(EX "confidence"),
                           lit("0.9", "http://www.w3.org/2001/XMLSchema#decimal")};
    CHECK(graphlava_request_add(req, GRAPHLAVA_OP_ANNOTATE, ann, 3));
    CHECK(graphlava_submit(db, req, GRAPHLAVA_DURABILITY_ORDERED, NULL));
    graphlava_request_free(req);

    /* Interactive write transaction: insert, then abort; then commit one. */
    graphlava_write *w = NULL;
    CHECK(graphlava_write_begin(db, GRAPHLAVA_DURABILITY_SYNC, &w));
    CHECK(graphlava_request_new_text(GRAPHLAVA_LANG_SPARQL, P "INSERT DATA { :alice :knows :bob }", &req));
    CHECK(graphlava_write_run(w, req, NULL));
    graphlava_request_free(req);
    CHECK(graphlava_abort(w));
    CHECK(graphlava_write_begin(db, GRAPHLAVA_DURABILITY_SYNC, &w));
    CHECK(graphlava_request_new_text(GRAPHLAVA_LANG_SPARQL, P "INSERT DATA { :alice :knows :carol }", &req));
    CHECK(graphlava_write_run(w, req, NULL));
    graphlava_request_free(req);
    CHECK(graphlava_commit(w));

    /* Core reads on a snapshot. */
    graphlava_read *r = NULL;
    CHECK(graphlava_read_begin(db, &r));
    uint64_t alice = 0, works_at = 0, bob = 0;
    graphlava_arg a = iri(EX "alice"), wa = iri(EX "worksAt"), b = iri(EX "bob");
    CHECK(graphlava_lookup(r, &a, &alice));
    CHECK(graphlava_lookup(r, &wa, &works_at));
    CHECK(graphlava_lookup(r, &b, &bob));
    EXPECT(alice != 0 && works_at != 0, "lookup");
    EXPECT(bob == 0, "aborted write left no term");
    uint64_t quads[4 * 8];
    size_t nq = 0;
    CHECK(graphlava_out(r, alice, works_at, quads, 8, &nq));
    EXPECT(nq == 1 && quads[3] == works, "out");
    uint64_t spo[3];
    CHECK(graphlava_stmt(r, works, spo));
    EXPECT(spo[0] == alice && spo[1] == works_at, "stmt");
    EXPECT(graphlava_stmt(r, 12345, spo) == GRAPHLAVA_ERR_NOT_FOUND, "stmt not found");
    graphlava_term t;
    CHECK(graphlava_decode(r, spo[2], &t));
    EXPECT(t.kind == GRAPHLAVA_TERM_IRI && strcmp(t.value, EX "acme") == 0, "decode");
    graphlava_term_free(&t);
    size_t total = 0;
    CHECK(graphlava_find(r, 0, 0, 0, NULL, 0, &total));
    EXPECT(total >= 4, "find counts");
    uint64_t *all = malloc(sizeof(uint64_t) * 4 * total);
    size_t again = 0;
    CHECK(graphlava_find(r, 0, 0, 0, all, total, &again));
    EXPECT(again == total, "find stable on the snapshot");
    free(all);
    uint64_t g1 = 0, graphs[4];
    size_t ng = 0;
    graphlava_arg ep = iri(EX "episode1");
    CHECK(graphlava_graph(r, &ep, &g1));
    CHECK(graphlava_graphs_of(r, works, graphs, 4, &ng));
    EXPECT(g1 != 0 && ng == 1 && graphs[0] == g1, "graphs_of");
    graphlava_read_end(r);

    /* Queries: the reifier query returns the stored confidence. */
    size_t n = 0;
    if (query(db, P "SELECT ?c WHERE { << :alice :worksAt :acme ~ ?r >> . ?r :confidence ?c }", 0, buf,
              sizeof buf, &n) || n != 1 || strcmp(buf, "0.9") != 0)
        return fail("confidence", GRAPHLAVA_ERR_INTERNAL);

    /* Suspend ends snapshots and makes calls wait; resume restores. */
    CHECK(graphlava_read_begin(db, &r));
    CHECK(graphlava_suspend(db));
    uint32_t susp = 0;
    CHECK(graphlava_is_suspended(db, &susp));
    EXPECT(susp == 1, "suspended");
    EXPECT(graphlava_lookup(r, &a, &alice) == GRAPHLAVA_ERR_CLOSED, "snapshot ended by suspend");
    graphlava_read_end(r);
    CHECK(graphlava_resume(db));
    size_t snaps = 9;
    CHECK(graphlava_open_snapshots(db, &snaps));
    EXPECT(snaps == 0, "no snapshot left");

    /* Extension journal: an extension appends, the owner applies once. */
    CHECK(graphlava_request_new(&req));
    graphlava_arg shared[] = {iri(EX "alice"), iri(EX "shared"), iri(EX "photo1")};
    CHECK(graphlava_request_add(req, GRAPHLAVA_OP_INSERT, shared, 3));
    uint64_t seq = 0;
    CHECK(graphlava_journal_append(journal, req, &seq));
    EXPECT(seq == 1, "first frame");
    graphlava_request_free(req);
    graphlava_journal_report rep;
    CHECK(graphlava_journal_apply(db, journal, "share", &rep));
    EXPECT(rep.applied == 1 && rep.dropped_bytes == 0, "journal applied");
    CHECK(graphlava_journal_apply(db, journal, "share", &rep));
    EXPECT(rep.applied == 0, "journal applied once");

    /* Administration. */
    uint64_t deleted = 0;
    CHECK(graphlava_sweep(db, 0, &deleted));
    CHECK(graphlava_export_snapshot(db, copy));
    CHECK(graphlava_flush(db));
    CHECK(graphlava_close(db));

    /* Reopen: the session's statements are there. */
    CHECK(graphlava_open(path, &db));
    if (query(db, P "SELECT ?o WHERE { :alice :worksAt ?o }", 0, buf, sizeof buf, &n) || n != 1 ||
        strcmp(buf, EX "acme") != 0)
        return fail("employer after reopen", GRAPHLAVA_ERR_INTERNAL);
    if (query(db, P "SELECT ?x WHERE { :alice :knows ?x }", 0, buf, sizeof buf, &n) || n != 1 ||
        strcmp(buf, EX "carol") != 0)
        return fail("committed write after reopen", GRAPHLAVA_ERR_INTERNAL);
    if (query(db, P "SELECT ?x WHERE { :alice :shared ?x }", 0, NULL, 0, &n) || n != 1)
        return fail("journal write after reopen", GRAPHLAVA_ERR_INTERNAL);
    CHECK(graphlava_close(db));
    CHECK(graphlava_open(copy, &db));
    if (query(db, P "SELECT ?o WHERE { :alice :worksAt ?o }", 0, NULL, 0, &n) || n != 1)
        return fail("snapshot copy", GRAPHLAVA_ERR_INTERNAL);
    CHECK(graphlava_close(db));
    remove(journal);
    remove(copy);
    printf("ok\n");
    return 0;
}
