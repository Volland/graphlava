#!/bin/sh
B=./target/release/bakeoff
while pgrep -f "bakeoff orders --engine redb --layout 6" >/dev/null; do sleep 10; done
echo "B3 flush-builds start $(date)"
$B orders --engine redb --layout 3 --n 10000000 --skip_writes 1 --build_dur flush --out results/B-orders-redb-3-flushbuild-10000000.json > logs/B-redb-3-flushbuild.log 2>&1
echo "mdbx tpcheck start $(date)"
$B tpcheck --engine mdbx --n 2000000 --readers 16 --reps 3 > logs/mdbx-tpcheck-shared-dbi.log 2>&1
MDBX_DBI_PER_TXN=1 $B tpcheck --engine mdbx --n 2000000 --readers 16 --reps 3 > logs/mdbx-tpcheck-per-txn-dbi.log 2>&1
$B tpcheck --engine redb --n 2000000 --readers 16 --reps 1 > logs/redb-tpcheck.log 2>&1
echo "mdbx tail start $(date)"
MDBX_DBI_PER_TXN=1 $B bake --engine mdbx --n 10000000 --phases tp,ow,probe --out results/A-bake-mdbx-tail-10000000.json > logs/A-mdbx-tail.log 2>&1
rm -rf data/bake-mdbx-10000000
echo "lat reruns start $(date)"
$B bake --engine redb --n 10000000 --phases lat --out results/A-bake-redb-latrerun-10000000.json > logs/A-redb-latrerun.log 2>&1
rm -rf data/bake-redb-10000000
MDBX_DBI_PER_TXN=1 $B bake --engine mdbx --n 10000000 --phases lat --out results/A-bake-mdbx-latrerun-10000000.json > logs/A-mdbx-latrerun.log 2>&1
rm -rf data/bake-mdbx-10000000
$B bake --engine redb --n 10000000 --phases lat --cache_mb 8192 --out results/A-bake-redb-cache8g-10000000.json > logs/A-redb-cache8g.log 2>&1
rm -rf data/bake-redb-10000000
echo "C10 start $(date)"
$B crash-prep --n 10000000 --db data/crash/base-10M.redb > logs/C-prep-10M.log 2>&1
$B crash-run --base data/crash/base-10M.redb --reps 3 --out results/C-crash-10M.json > logs/C-run-10M.log 2>&1
rm -f data/crash/base-10M.redb data/crash/base-10M.meta.json
echo "C100 start $(date)"
$B crash-prep --n 100000000 --db data/crash/base-100M.redb > logs/C-prep-100M.log 2>&1
$B crash-run --base data/crash/base-100M.redb --reps 2 --modes clean,uncommitted_big,after_noflush,after_flush,qr_after_noflush --out results/C-crash-100M.json > logs/C-run-100M.log 2>&1
rm -f data/crash/base-100M.redb data/crash/base-100M.meta.json
echo "redb4 start $(date)"
$B bake --engine redb4 --n 10000000 --phases lat,probe --out results/A-bake-redb4-10000000.json > logs/A-redb4.log 2>&1
rm -rf data/bake-redb4-10000000
echo "ALLDONE $(date)"
