/* Misuse of the C FFI returns error statuses, never undefined behaviour
 * ([[openspec:mobile-bindings#FFI misuse safety]]). Built and run under
 * AddressSanitizer and LeakSanitizer in CI. Prints "ok".
 *
 * @tg: verifies:: [[openspec:mobile-bindings#FFI misuse safety#Use after close]] {level: e2e}
 * @tg: verifies:: [[openspec:mobile-bindings#Complete C FFI#Header version mismatch]] {level: unit}
 */
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include "graphlava.h"

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

#define EXPECT_ST(call, want)                                   \
    do {                                                        \
        enum graphlava_status st_ = (call);                     \
        if (st_ != (want)) return fail(#call " (want " #want ")", st_); \
    } while (0)

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    const char *path = argv[1];
    char other[1024];
    snprintf(other, sizeof other, "%s.abi", path);
    remove(other);

    /* Header version mismatch: no file is created. */
    graphlava_db *db = NULL;
    EXPECT_ST(graphlava_open_ex(GRAPHLAVA_ABI_VERSION + 1, other, 0, &db), GRAPHLAVA_ERR_ABI);
    EXPECT_ST(graphlava_open_ex(0, other, 0, &db), GRAPHLAVA_ERR_ABI);
    if (db != NULL || access(other, F_OK) == 0) return fail("ABI mismatch created a file", GRAPHLAVA_ERR_INTERNAL);
    EXPECT_ST(graphlava_open_ex(GRAPHLAVA_ABI_VERSION, other, 0x80, &db), GRAPHLAVA_ERR_INVALID_ARGUMENT);

    /* Use after close. */
    CHECK(graphlava_open(path, &db));
    CHECK(graphlava_close(db));
    graphlava_read *r = NULL;
    EXPECT_ST(graphlava_read_begin(db, &r), GRAPHLAVA_ERR_CLOSED);
    EXPECT_ST(graphlava_close(db), GRAPHLAVA_ERR_CLOSED);
    EXPECT_ST(graphlava_suspend(db), GRAPHLAVA_ERR_CLOSED);
    EXPECT_ST(graphlava_sparql_update(db, "INSERT DATA {}", GRAPHLAVA_DURABILITY_SYNC), GRAPHLAVA_ERR_CLOSED);
    /* A fresh open of the file succeeds. */
    CHECK(graphlava_open(path, &db));

    /* Double commit, commit after abort, run after commit. */
    graphlava_write *w = NULL;
    CHECK(graphlava_write_begin(db, GRAPHLAVA_DURABILITY_SYNC, &w));
    CHECK(graphlava_commit(w));
    EXPECT_ST(graphlava_commit(w), GRAPHLAVA_ERR_CLOSED);
    EXPECT_ST(graphlava_abort(w), GRAPHLAVA_ERR_CLOSED);
    graphlava_request *req = NULL;
    CHECK(graphlava_request_new(&req));
    EXPECT_ST(graphlava_write_run(w, req, NULL), GRAPHLAVA_ERR_CLOSED);

    /* A handle of the wrong kind, and a pointer that is not a handle. */
    EXPECT_ST(graphlava_submit(db, (graphlava_request *)db, GRAPHLAVA_DURABILITY_SYNC, NULL),
              GRAPHLAVA_ERR_INVALID_ARGUMENT);
    int not_a_handle = 0;
    EXPECT_ST(graphlava_flush((graphlava_db *)&not_a_handle), GRAPHLAVA_ERR_INVALID_ARGUMENT);

    /* Null out-parameters and arguments. */
    EXPECT_ST(graphlava_read_begin(db, NULL), GRAPHLAVA_ERR_INVALID_ARGUMENT);
    EXPECT_ST(graphlava_abi_version(NULL), GRAPHLAVA_ERR_INVALID_ARGUMENT);
    EXPECT_ST(graphlava_request_add(req, GRAPHLAVA_OP_INSERT, NULL, 3), GRAPHLAVA_ERR_INVALID_ARGUMENT);
    graphlava_arg bad = {99, 0, NULL, NULL, NULL};
    EXPECT_ST(graphlava_request_add(req, GRAPHLAVA_OP_INSERT, &bad, 1), GRAPHLAVA_ERR_INVALID_ARGUMENT);
    uint8_t junk[3] = {1, 9, 9};
    graphlava_request *dec = NULL;
    EXPECT_ST(graphlava_request_decode(junk, sizeof junk, &dec), GRAPHLAVA_ERR_INVALID_REQUEST);

    /* Double free is ignored. */
    graphlava_request_free(req);
    graphlava_request_free(req);

    /* A live result handle makes close busy; the handle stays valid. */
    graphlava_read_begin(db, &r);
    EXPECT_ST(graphlava_close(db), GRAPHLAVA_ERR_BUSY);
    graphlava_read_end(r);
    CHECK(graphlava_close(db));
    printf("ok\n");
    return 0;
}
