/* Every snippet of crates/graphlava-ffi/README.md, run end to end.
 * Prints "ok" on success. */
#include <stdio.h>
#include <string.h>
#include "graphlava.h"

static int check(graphlava_status st, const char *what) {
    if (st == GRAPHLAVA_OK) return 0;
    char *msg = NULL;
    graphlava_last_error(&msg);
    fprintf(stderr, "%s failed (%d): %s\n", what, (int)st, msg ? msg : "");
    graphlava_string_free(msg);
    return 1;
}

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    graphlava_db *db = NULL;
    /* open, checking the header and the library agree */
    if (check(graphlava_open_ex(GRAPHLAVA_ABI_VERSION, argv[1], 0, &db), "open")) return 1;

    /* write */
    if (check(graphlava_sparql_update(db,
            "PREFIX : <http://example.org/>\n"
            "INSERT DATA { :alice :worksAt :acme {| :confidence 0.9 |} }",
            GRAPHLAVA_DURABILITY_ORDERED), "update")) return 1;

    /* query: prepare once, run, read batches of IDs, decode on demand */
    graphlava_prepared *q = NULL;
    graphlava_rows *rows = NULL;
    if (check(graphlava_prepare(db, GRAPHLAVA_LANG_SPARQL,
            "PREFIX : <http://example.org/>\n"
            "SELECT ?who ?c WHERE { << ?who :worksAt :acme ~ ?r >> . ?r :confidence ?c }",
            &q), "prepare")) return 1;
    if (check(graphlava_query(db, q, &rows), "query")) return 1;
    size_t total = 0;
    for (;;) {
        const uint64_t *ids = NULL;
        size_t nrows = 0, ncols = 0;
        if (check(graphlava_rows_next_batch(rows, &ids, &nrows, &ncols), "next_batch")) return 1;
        if (nrows == 0) break;                 /* end of the result */
        for (size_t r = 0; r < nrows; r++) {
            graphlava_term who;                /* column 0 of row r: ids[0 * nrows + r] */
            if (check(graphlava_term_decode(rows, ids[0 * nrows + r], &who), "decode")) return 1;
            if (strcmp(who.value, "http://example.org/alice") != 0) return 1;
            graphlava_term_free(&who);
        }
        total += nrows;
    }
    graphlava_rows_free(rows);
    graphlava_prepared_free(q);
    if (total != 1) return 1;

    if (check(graphlava_close(db), "close")) return 1;
    printf("ok\n");
    return 0;
}
