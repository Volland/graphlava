#!/usr/bin/env python3
"""Summarize results/*.json into markdown tables (printed to stdout).
Reported value per configuration = median across runs; [min-max] across runs."""
import glob
import json
import statistics as st
import sys

R = sys.argv[1] if len(sys.argv) > 1 else "results"


def med(xs):
    return st.median(xs) if xs else float("nan")


def rng(xs):
    return f"{min(xs):.0f}-{max(xs):.0f}" if xs else ""


def f(x, d=1):
    return f"{x:,.{d}f}"


def bake():
    files = [f"{R}/A-bake-{e}-10000000.json" for e in ["redb", "mdbx", "sana"]]
    files = [f for f in files if glob.glob(f)]
    if not files:
        return
    rows = []
    for fn in files:
        j = json.load(open(fn))["result"]
        rows.append(j)
    print("### A. Bulk load and file size (10M)\n")
    print("| engine | load s | stmts/s | MB after load | B/stmt after load (alloc) | B/stmt before overwrites | B/stmt after 10^6 overwrites | NoFlush-only probe growth MB |")
    print("|---|---|---|---|---|---|---|---|")
    for j in rows:
        bl = j["bulk_load"]
        sa = j["size_after_load"]
        sb = j["size_before_overwrites"]
        so = j["size_after_overwrites"]
        pr = j.get("noflush_only_probe")
        g = (pr["after_noflush"]["alloc_bytes"] - pr["before"]["alloc_bytes"]) / 1e6 if pr else float("nan")
        print(f"| {j['engine']} | {f(bl['wall_s'])} | {f(bl['stmts_per_s'],0)} | {f(sa['alloc_bytes']/1e6,0)} | {f(sa['bytes_per_stmt_alloc'])} | {f(sb['bytes_per_stmt_alloc'])} | {f(so['bytes_per_stmt_alloc'])} | {f(g,0)} |")
    print()
    for lab in ["latency_relaxed", "latency_sync"]:
        print(f"### A. Mix latency, {lab.split('_')[1]} (µs; median of runs, [min-max of runs] for p99)\n")
        print("| engine | runs x ops | mix p50 | mix p99 | mix p99 range | mix p99.9 | writes p50 | writes p99 | ops/s |")
        print("|---|---|---|---|---|---|---|---|---|")
        for j in rows:
            rs = j[lab]
            p99s = [r["all"]["p99_us"] for r in rs]
            print(f"| {j['engine']} | {len(rs)} x {rs[0]['ops']} | {f(med([r['all']['p50_us'] for r in rs]))} | {f(med(p99s))} | {rng(p99s)} | {f(med([r['all']['p999_us'] for r in rs]))} | {f(med([r['writes']['p50_us'] for r in rs]))} | {f(med([r['writes']['p99_us'] for r in rs]))} | {f(med([r['ops_per_s'] for r in rs]),0)} |")
        print()
        classes = list(rows[0][lab][0]["per_class"].keys())
        print(f"Per class p50 / p99 / p99.9 (µs), {lab.split('_')[1]}:\n")
        print("| class | " + " | ".join(j["engine"] for j in rows) + " |")
        print("|---|" + "---|" * len(rows))
        for c in classes:
            cells = []
            for j in rows:
                rs = j[lab]
                cells.append(" / ".join(f(med([r["per_class"][c][k] for r in rs])) for k in ["p50_us", "p99_us", "p999_us"]))
            print(f"| {c} | " + " | ".join(cells) + " |")
        print()
        print(f"Write I/O during the {lab.split('_')[1]} mix (median of runs): disk bytes per write op / bytes per kv byte changed\n")
        print("| engine | disk B per write op | disk B / kv B |")
        print("|---|---|---|")
        for j in rows:
            rs = j[lab]
            print(f"| {j['engine']} | {f(med([r['disk_bytes_per_write_op'] for r in rs]),0)} | {f(med([r['write_amp_vs_kv_bytes'] for r in rs]))} |")
        print()
    print("### A. Throughput, 1 writer + N readers (ops/s, 10 s, 1 run)\n")
    print("| engine | durability | readers | writer ops/s | reader ops/s total | per reader thread | reader errors |")
    print("|---|---|---|---|---|---|---|")
    for j in rows:
        for t in j.get("throughput", []):
            print(f"| {j['engine']} | {t['durability']} | {t['readers']} | {f(t['writer_ops_per_s'],0)} | {f(t['reader_ops_per_s'],0)} | {f(t['reader_ops_per_s_per_thread'],0)} | {t.get('reader_errors', 'n/a')} |")
    print()
    print("### A. 10^6 overwrites (1 per txn, NoFlush, every 100th Flush)\n")
    print("| engine | wall s | disk bytes/overwrite | disk B / kv B |")
    print("|---|---|---|---|")
    for j in rows:
        o = j["overwrite_phase"]
        print(f"| {j['engine']} | {f(o['wall_s'])} | {f(o['disk_bytes_per_overwrite'],0)} | {f(o['write_amp_vs_kv_bytes'])} |")
    print()


def variants():
    files = sorted(glob.glob(f"{R}/A-bake-*-10000000.json"))
    print("### A. Mix latency, all runs including reruns and variants (µs)\n")
    print("| file | engine | durability | runs | mix p50 | mix p99 (median) | p99 per run | mix p99.9 | writes p50 | writes p99 |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for fn in files:
        j = json.load(open(fn))["result"]
        for lab in ["latency_relaxed", "latency_sync"]:
            rs = j.get(lab)
            if not rs:
                continue
            p99s = [r["all"]["p99_us"] for r in rs]
            print(f"| {fn.split('/')[-1]} | {j['engine']} (cache {j['engine_info'].get('cache_bytes', 0)//2**20} MiB) | {lab.split('_')[1]} | {len(rs)} | {f(med([r['all']['p50_us'] for r in rs]))} | {f(med(p99s))} | {', '.join(f'{x:.0f}' for x in p99s)} | {f(med([r['all']['p999_us'] for r in rs]))} | {f(med([r['writes']['p50_us'] for r in rs]))} | {f(med([r['writes']['p99_us'] for r in rs]))} |")
    print()


def orders():
    files = sorted(glob.glob(f"{R}/B-orders-*.json"))
    if not files:
        return
    print("### B. Ordering count\n")
    print("| engine | layout | load s | B/stmt alloc | small-txn write p50 relaxed | p99 relaxed | p50 sync | p99 sync |")
    print("|---|---|---|---|---|---|---|---|")
    cyc = []
    builds = []
    for fn in files:
        j = json.load(open(fn))["result"]
        wr = j.get("write_relaxed", [])
        ws = j.get("write_sync", [])
        tag = fn.split("/")[-1]
        print(f"| {j['engine']} ({tag}) | {','.join(j['layout'])} | {f(j['bulk_load_s'])} | {f(j['size_after_load']['bytes_per_stmt_alloc'])} | {f(med([r['all']['p50_us'] for r in wr]))} | {f(med([r['all']['p99_us'] for r in wr]))} | {f(med([r['all']['p50_us'] for r in ws]))} | {f(med([r['all']['p99_us'] for r in ws]))} |")
        for c in j["cyclic"]:
            cyc.append((j["engine"] + " " + fn.split("/")[-1].replace("B-orders-", "").replace("-10000000.json", ""), str(c["layout"]), c["plan"], c["queries"]))
        if "online_builds" in j:
            builds.append((fn.split("/")[-1], j["online_builds"], j["size_after_load"], j["size_after_builds"]))
    print()
    print("Cyclic queries p50 / p99 (µs; median over runs):\n")
    groups = {}
    for e, lay, plan, q in cyc:
        groups.setdefault((e, lay, plan), []).append(q)
    qn = ["tri1_anchored_p0", "cyc4_anchored_p0", "tri_anypred_anchored", "tri_all_rare_pred"]
    print("| engine | layout | plan | " + " | ".join(qn) + " |")
    print("|---|---|---|" + "---|" * len(qn))
    for (e, lay, plan), qs in groups.items():
        cells = []
        for q in qn:
            cells.append(f"{f(med([x[q]['p50_us'] for x in qs]))} / {f(med([x[q]['p99_us'] for x in qs]))}")
        print(f"| {e} | {lay} | {plan} | " + " | ".join(cells) + " |")
    print()
    for e, bs, s0, s1 in builds:
        print(f"Online builds ({e}): " + "; ".join(f"{b['target']} ({b.get('batch_durability','NoFlush')} batches of {b['batch']}): {f(b['wall_s'])} s, +{f(b['growth_bytes_alloc']/1e6,0)} MB alloc, {f(b['disk_bytes_written']/1e6,0)} MB written" for b in bs) + f"; B/stmt {f(s0['bytes_per_stmt_alloc'])} -> {f(s1['bytes_per_stmt_alloc'])}\n")


def crash():
    for fn in sorted(glob.glob(f"{R}/C-crash-*.json")):
        j = json.load(open(fn))
        print(f"### C. Crash repair, {fn} (base {j['base_alloc']/1e9:.2f} GB, {j['meta']['loaded_stmts']:,} stmts)\n")
        print("| mode | reps | open s (median) | min-max | repaired (callback calls>0) | first query ms | clean reopen s |")
        print("|---|---|---|---|---|---|---|")
        modes = {}
        for t in j["trials"]:
            modes.setdefault(t["mode"], []).append(t)
        for m, ts in modes.items():
            os_ = [t["open_after"]["open_s"] for t in ts]
            print(f"| {m} | {len(ts)} | {med(os_):.3f} | {min(os_):.3f}-{max(os_):.3f} | {sum(1 for t in ts if t['open_after']['repair_callback_calls']>0)}/{len(ts)} | {med([t['open_after']['first_query_s']*1e3 for t in ts]):.2f} | {med([t['reopen_after_clean_close']['open_s'] for t in ts]):.3f} |")
        print()


bake()
variants()
orders()
crash()
