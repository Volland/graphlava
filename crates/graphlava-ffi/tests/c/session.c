/* The agent-memory session of [[21-e2e-tests#Agent memory session]] through
 * the C FFI ([[21-e2e-tests#C FFI lifecycle]]). Built and run under
 * AddressSanitizer and LeakSanitizer in CI; any leak or error code other than
 * the documented one fails. Prints "ok" on success. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "graphlava.h"

#define P "PREFIX : <http://ex.org/>\nPREFIX meta: <https://graphlava.org/ns#>\n"

static int fail(const char *what, enum graphlava_status st) {
    char *msg = NULL;
    graphlava_last_error(&msg);
    fprintf(stderr, "%s: status %d: %s\n", what, (int)st, msg ? msg : "(none)");
    graphlava_string_free(msg);
    return 1;
}

#define CHECK(call)                                   \
    do {                                              \
        enum graphlava_status st_ = (call);           \
        if (st_ != GRAPHLAVA_OK) return fail(#call, st_); \
    } while (0)

/* Runs a one-column-or-more SPARQL query; copies the value of column `col`
 * of the single expected row into `out` (when out != NULL) and returns the
 * row count in *n. */
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
        CHECK(graphlava_rows_next_batch(rows, &ids, &nr, &nc));
        if (nr == 0) break;
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
    graphlava_db *db = NULL;
    CHECK(graphlava_open(argv[1], &db));
    CHECK(graphlava_sparql_update(db, P "INSERT DATA { :episode1 meta:in :agent }", GRAPHLAVA_DURABILITY_ORDERED));
    CHECK(graphlava_sparql_update(db,
        P "INSERT DATA { GRAPH :episode1 { :alice :worksAt :acme {| :confidence 0.9 |} . :alice :age 42 } }",
        GRAPHLAVA_DURABILITY_ORDERED));
    CHECK(graphlava_sparql_update(db, P "INSERT DATA { << :alice :age 42 ~ _:r >> :source :doc5 }",
        GRAPHLAVA_DURABILITY_ORDERED));
    char before[256], age[256], buf[256];
    size_t n = 0;
    if (query(db, P "SELECT ?r WHERE { << :alice :worksAt :acme ~ ?r >> }", 0, before, sizeof before, &n) || n != 1)
        return fail("reifier before", GRAPHLAVA_ERR_INTERNAL);
    if (query(db, P "SELECT ?r WHERE { << :alice :age 42 ~ ?r >> }", 0, age, sizeof age, &n) || n != 1)
        return fail("age reifier", GRAPHLAVA_ERR_INTERNAL);
    CHECK(graphlava_sparql_update(db,
        P "DELETE { :alice :worksAt :acme } INSERT { << :alice :worksAt :globex ~ ?r >> } "
          "WHERE { << :alice :worksAt :acme ~ ?r >> }",
        GRAPHLAVA_DURABILITY_SYNC));
    CHECK(graphlava_close(db));
    db = NULL;
    CHECK(graphlava_open(argv[1], &db));

    /* documented error codes */
    graphlava_db *second = NULL;
    if (graphlava_open(argv[1], &second) != GRAPHLAVA_ERR_LOCKED) return fail("second open", GRAPHLAVA_ERR_INTERNAL);
    if (graphlava_open(NULL, &second) != GRAPHLAVA_ERR_INVALID_ARGUMENT) return fail("null path", GRAPHLAVA_ERR_INTERNAL);
    graphlava_prepared *bad = NULL;
    graphlava_rows *badrows = NULL;
    CHECK(graphlava_prepare(db, GRAPHLAVA_LANG_SPARQL, "SELEKT nothing", &bad));
    if (graphlava_query(db, bad, &badrows) != GRAPHLAVA_ERR_INVALID_REQUEST || badrows != NULL)
        return fail("invalid query", GRAPHLAVA_ERR_INTERNAL);
    graphlava_prepared_free(bad);
    char *msg = NULL;
    graphlava_last_error(&msg);
    graphlava_string_free(msg);

    if (query(db, P "SELECT ?o ?c ?r WHERE { GRAPH :agent { << :alice :worksAt ?o ~ ?r >> . ?r :confidence ?c } }",
              0, buf, sizeof buf, &n) || n != 1 || strcmp(buf, "http://ex.org/globex") != 0)
        return fail("employer", GRAPHLAVA_ERR_INTERNAL);
    if (query(db, P "SELECT ?o ?c ?r WHERE { GRAPH :agent { << :alice :worksAt ?o ~ ?r >> . ?r :confidence ?c } }",
              1, buf, sizeof buf, &n) || strcmp(buf, "0.9") != 0)
        return fail("confidence", GRAPHLAVA_ERR_INTERNAL);
    if (query(db, P "SELECT ?o ?c ?r WHERE { GRAPH :agent { << :alice :worksAt ?o ~ ?r >> . ?r :confidence ?c } }",
              2, buf, sizeof buf, &n) || strcmp(buf, before) != 0)
        return fail("reifier unchanged", GRAPHLAVA_ERR_INTERNAL);
    if (query(db, P "SELECT ?r ?s WHERE { << :alice :age 42 ~ ?r >> . ?r :source ?s }", 0, buf, sizeof buf, &n) ||
        n != 1 || strcmp(buf, age) != 0)
        return fail("age id stable", GRAPHLAVA_ERR_INTERNAL);
    if (query(db, P "SELECT ?o WHERE { GRAPH! :agent { :alice :worksAt ?o } }", 0, NULL, 0, &n) || n != 0)
        return fail("GRAPH! is empty", GRAPHLAVA_ERR_INTERNAL);
    CHECK(graphlava_close(db));
    printf("ok\n");
    return 0;
}
