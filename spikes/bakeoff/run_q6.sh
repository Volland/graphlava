#!/bin/sh
B=./target/release/bakeoff
while pgrep -f "bakeoff bake --engine mdbx --n 10000000 --phases lat " >/dev/null; do sleep 10; done
rm -rf data/bake-mdbx-10000000
MDBX_DBI_PER_TXN=1 $B bake --engine mdbx --mdbx_mode 2 --n 10000000 --phases lats --out results/A-bake-mdbx-durableenv-10000000.json > logs/A-mdbx-durableenv.log 2>&1
rm -rf data/bake-mdbx-10000000
$B bake --engine redb --n 10000000 --phases lat --cache_mb 8192 --out results/A-bake-redb-cache8g-10000000.json > logs/A-redb-cache8g.log 2>&1
rm -rf data/bake-redb-10000000
echo "C10 start $(date)"
$B crash-prep --n 10000000 --db data/crash/base-10M.redb > logs/C-prep-10M.log 2>&1
$B crash-run --base data/crash/base-10M.redb --reps 3 --out results/C-crash-10M.json > logs/C-run-10M.log 2>&1
$B crash-reqbytes --base data/crash/base-10M.redb --out results/C-reqbytes-10M.json > logs/C-reqbytes.log 2>&1
rm -f data/crash/base-10M.redb data/crash/base-10M.meta.json
echo "C100 start $(date)"
$B crash-prep --n 100000000 --db data/crash/base-100M.redb > logs/C-prep-100M.log 2>&1
$B crash-run --base data/crash/base-100M.redb --reps 2 --modes clean,uncommitted_big,after_noflush,after_flush,qr_after_noflush --out results/C-crash-100M.json > logs/C-run-100M.log 2>&1
rm -f data/crash/base-100M.redb data/crash/base-100M.meta.json
echo "redb4 start $(date)"
$B bake --engine redb4 --n 10000000 --phases lat,probe --out results/A-bake-redb4-10000000.json > logs/A-redb4.log 2>&1
rm -rf data/bake-redb4-10000000
echo "ALLDONE $(date)"
