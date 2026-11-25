<p align="center"><img src="https://graphlava.org/assets/logo.svg" width="120" alt="graphlava logo"></p>

<h1 align="center">graphlava-ffi</h1>

<p align="center"><strong>The C API of <a href="https://graphlava.org">graphlava</a>: an embedded metagraph database for agent memory, in one file.</strong><br>
Use it from C, C++, Zig, Go (cgo), Dart FFI or any language with a C FFI. The Swift and Kotlin bindings are built on it.</p>

<p align="center">
<a href="https://graphlava.org">Website</a> ·
<a href="https://graphlava.org/book/">Mini book</a> ·
<a href="https://graphlava.org/articles/">Articles</a> ·
<a href="https://github.com/Volland/graphlava/blob/main/crates/graphlava-ffi/include/graphlava.h">graphlava.h</a> ·
<a href="https://github.com/Volland/graphlava">Repository</a>
</p>

> **Status: pre-release.** ABI version 1. The file format is not frozen yet: files written now may not open in the next version.

## What you get

`libgraphlava_ffi` (static and dynamic) plus one generated header, [`include/graphlava.h`](https://github.com/Volland/graphlava/blob/main/crates/graphlava-ffi/include/graphlava.h). Through it you get the whole engine: statements with stable IDs you can annotate (RDF 1.2 reifiers), n-ary hyperedges, nested graphs, SPARQL 1.2 and Datalog, and `sync`, `ordered` or `relaxed` durability per write. Read [Statement graphs: the one idea under graphlava](https://graphlava.org/articles/statement-graphs.html) for the model.

## Build

```sh
cargo build -p graphlava-ffi --release
# target/release/libgraphlava_ffi.a  (and .so / .dylib / .dll)
cc -std=c99 app.c -I crates/graphlava-ffi/include target/release/libgraphlava_ffi.a -lpthread -ldl -lm
```

Tagged releases attach prebuilt archives for Linux, macOS, Windows and iOS. Android builds with `cargo ndk`; the [Kotlin binding](https://github.com/Volland/graphlava/tree/main/bindings/kotlin) packages it in an AAR.

## Example

```c
#include "graphlava.h"

graphlava_db *db = NULL;
graphlava_open_ex(GRAPHLAVA_ABI_VERSION, "memory.glv", 0, &db);   /* header and library must agree */

graphlava_sparql_update(db,
    "PREFIX : <http://example.org/>\n"
    "INSERT DATA { :alice :worksAt :acme {| :confidence 0.9 |} }",
    GRAPHLAVA_DURABILITY_ORDERED);

graphlava_prepared *q = NULL;
graphlava_rows *rows = NULL;
graphlava_prepare(db, GRAPHLAVA_LANG_SPARQL,
    "PREFIX : <http://example.org/>\n"
    "SELECT ?who ?c WHERE { << ?who :worksAt :acme ~ ?r >> . ?r :confidence ?c }", &q);
graphlava_query(db, q, &rows);

for (;;) {
    const uint64_t *ids; size_t nrows, ncols;
    graphlava_rows_next_batch(rows, &ids, &nrows, &ncols);   /* up to 1024 rows, column-major */
    if (nrows == 0) break;
    for (size_t r = 0; r < nrows; r++) {
        graphlava_term who;
        graphlava_term_decode(rows, ids[0 * nrows + r], &who); /* decode only what you print */
        printf("%s\n", who.value);
        graphlava_term_free(&who);
    }
}
graphlava_rows_free(rows);
graphlava_prepared_free(q);
graphlava_close(db);
```

The full program, with error checks, is [`tests/c/readme.c`](https://github.com/Volland/graphlava/blob/main/crates/graphlava-ffi/tests/c/readme.c); CI compiles and runs it.

## Conventions

- **Status codes.** Every function except `graphlava_version` returns a `graphlava_status`; outputs go through out-parameters. On failure `graphlava_last_error(&msg)` gives a message for the calling thread (free it with `graphlava_string_free`).
- **No panics cross the boundary.** Every entry point catches them and returns `GRAPHLAVA_ERR_INTERNAL`.
- **Handles are tokens.** `graphlava_db`, `graphlava_rows` and the other handles are checked on every call: using one after `close`, or committing twice, returns `GRAPHLAVA_ERR_CLOSED` instead of crashing.
- **Ownership.** Strings and terms you receive are yours to free with the matching `*_free`; batch buffers belong to the rows handle and stay valid until the next call on it.
- **One owner per file.** A second open of the same file, in any process, returns `GRAPHLAVA_ERR_LOCKED`. `graphlava_close` returns `GRAPHLAVA_ERR_BUSY` while result handles are alive.

## Function groups

| Group | Functions |
| --- | --- |
| Lifecycle | `graphlava_open`, `graphlava_open_ex`, `graphlava_close`, `graphlava_abi_version`, `graphlava_suspend`, `graphlava_resume`, `graphlava_flush`, `graphlava_is_suspended` |
| Writes | `graphlava_sparql_update`, `graphlava_datalog_update`, `graphlava_insert`, `graphlava_request_new` + `graphlava_request_add` + `graphlava_submit` (every core operation, all or nothing) |
| Interactive transactions | `graphlava_write_begin`, `graphlava_write_run`, `graphlava_commit`, `graphlava_abort` (100 ms limit) |
| Snapshots and core reads | `graphlava_read_begin`, `graphlava_read_end`, `graphlava_lookup`, `graphlava_decode`, `graphlava_stmt`, `graphlava_find`, `graphlava_out`, `graphlava_in`, `graphlava_props`, `graphlava_graph`, `graphlava_graphs_of`, `graphlava_scope`, `graphlava_hedge` |
| Queries | `graphlava_prepare` (SPARQL or Datalog), `graphlava_query`, `graphlava_rows_columns`, `graphlava_rows_next_batch`, `graphlava_term_decode` |
| Admin | `graphlava_sweep`, `graphlava_export_snapshot` |
| iOS extensions | `graphlava_journal_append`, `graphlava_journal_apply` (writes from a share extension while the app is closed) |

## Mobile lifecycle

`graphlava_suspend` ends snapshots, aborts an open write transaction, flushes the group commit and, for a file in an iOS app-group container (`GRAPHLAVA_OPEN_APP_GROUP`), releases the file lock so the app is never killed for holding it; `graphlava_resume` takes it back. Writes from an app extension go through a CRC-checked journal that the app applies exactly once on its next open.

## Quality

The API is tested from Rust and from C programs compiled with `-Wall -Wextra -Werror` and run under AddressSanitizer and LeakSanitizer in CI; a `cargo fuzz` target drives random call sequences. The header is generated with cbindgen, and CI fails when it drifts.

## Learn more

- [graphlava.org](https://graphlava.org) · [Mini book: Hypergraphs and Metagraphs in Statement Graphs](https://graphlava.org/book/) · [Articles](https://graphlava.org/articles/)
- [The C FFI design](https://github.com/Volland/graphlava/blob/main/tg.md/16-api.md) and [the Rust core](https://github.com/Volland/graphlava/tree/main/crates/graphlava)
- Bindings built on this API: [Swift](https://github.com/Volland/graphlava/tree/main/bindings/swift) and [Kotlin](https://github.com/Volland/graphlava/tree/main/bindings/kotlin)

## License

MIT OR Apache-2.0.
