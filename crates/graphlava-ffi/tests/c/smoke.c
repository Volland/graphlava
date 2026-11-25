/* Smoke test of the graphlava C API, compiled against include/graphlava.h and
 * linked with the staticlib by tests/c_program.rs. Usage: smoke <db-path>. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "graphlava.h"

static int fail(const char *what, graphlava_status st) {
    char *msg = NULL;
    graphlava_last_error(&msg);
    fprintf(stderr, "%s failed with status %d: %s\n", what, (int)st, msg ? msg : "(no message)");
    graphlava_string_free(msg);
    return 1;
}

#define CHECK(call)                                    \
    do {                                               \
        graphlava_status st_ = (call);                 \
        if (st_ != GRAPHLAVA_OK) return fail(#call, st_); \
    } while (0)

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: smoke <db-path>\n");
        return 2;
    }
    printf("graphlava %s\n", graphlava_version());

    graphlava_db *db = NULL;
    CHECK(graphlava_open(argv[1], &db));
    CHECK(graphlava_sparql_update(db,
        "PREFIX : <http://ex.org/> INSERT DATA { :alice :knows :bob . :bob :knows :carol . }",
        GRAPHLAVA_DURABILITY_SYNC));
    uint64_t sid = 0;
    CHECK(graphlava_insert(db, "http://ex.org/alice", "http://ex.org/name", GRAPHLAVA_TERM_LITERAL,
                           "Alice", NULL, "en", GRAPHLAVA_DURABILITY_ORDERED, &sid));

    graphlava_prepared *q = NULL;
    CHECK(graphlava_prepare(db, GRAPHLAVA_LANG_DATALOG,
        "@prefix : <http://ex.org/> .\n"
        "reach(X, Y) :- triple(X, :knows, Y) .\n"
        "reach(X, Z) :- reach(X, Y), triple(Y, :knows, Z) .\n"
        "?- reach(:alice, Y) .\n", &q));
    graphlava_rows *rows = NULL;
    CHECK(graphlava_query(db, q, &rows));

    if (graphlava_close(db) != GRAPHLAVA_ERR_BUSY) {
        fprintf(stderr, "close did not refuse while rows are alive\n");
        return 1;
    }

    size_t ncols = 0;
    CHECK(graphlava_rows_columns(rows, &ncols));
    size_t total = 0;
    int saw_carol = 0;
    for (;;) {
        const uint64_t *ids = NULL;
        size_t n = 0, k = 0;
        CHECK(graphlava_rows_next_batch(rows, &ids, &n, &k));
        if (n == 0) break;
        for (size_t r = 0; r < n; r++) {
            graphlava_term t;
            CHECK(graphlava_term_decode(rows, ids[0 * n + r], &t));
            if (t.kind == GRAPHLAVA_TERM_IRI && strcmp(t.value, "http://ex.org/carol") == 0) saw_carol = 1;
            printf("Y = %s\n", t.value ? t.value : "(unbound)");
            graphlava_term_free(&t);
        }
        total += n;
    }
    graphlava_rows_free(rows);
    graphlava_prepared_free(q);

    /* An invalid query fails with a status and a message. */
    CHECK(graphlava_prepare(db, GRAPHLAVA_LANG_SPARQL, "SELEKT", &q));
    graphlava_status st = graphlava_query(db, q, &rows);
    graphlava_prepared_free(q);
    if (st != GRAPHLAVA_ERR_INVALID_REQUEST || rows != NULL) {
        fprintf(stderr, "invalid query returned %d\n", (int)st);
        return 1;
    }

    CHECK(graphlava_close(db));
    if (ncols != 1 || total != 2 || !saw_carol) {
        fprintf(stderr, "unexpected result: cols=%zu rows=%zu carol=%d\n", ncols, total, saw_carol);
        return 1;
    }
    printf("ok\n");
    return 0;
}
