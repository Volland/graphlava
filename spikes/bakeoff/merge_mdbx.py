#!/usr/bin/env python3
"""Merge the libmdbx latency phases (results/A-bake-mdbx-10000000.json.partial, the original
run that later crashed in the 16-reader throughput phase with MDBX_CORRUPTED in a reader)
with the tail run (results/A-bake-mdbx-tail-10000000.json: fresh load, throughput with reader
errors counted, 10^6 overwrites, NoFlush probe) into results/A-bake-mdbx-10000000.json."""
import json
p = json.load(open("results/A-bake-mdbx-10000000.json.partial"))["result"]
t = json.load(open("results/A-bake-mdbx-tail-10000000.json"))
tr = t["result"]
out = dict(p)
for k in ["throughput", "size_before_overwrites", "overwrite_phase", "size_after_overwrites", "noflush_only_probe"]:
    if k in tr:
        out[k] = tr[k]
out["merged_from"] = {"latency_phases": "A-bake-mdbx-10000000.json.partial (shared dbi handles)",
                      "tail_phases": "A-bake-mdbx-tail-10000000.json (MDBX_DBI_PER_TXN=1, fresh 10M load)",
                      "tail_bulk_load": tr["bulk_load"]}
json.dump({"experiment": "A engine bake-off", "env": t["env"], "args": t["args"], "mix": t["mix"], "result": out},
          open("results/A-bake-mdbx-10000000.json", "w"), indent=1)
print("merged")
