#!/bin/sh
# Sequential driver for the full spike (each step deletes its data when done).
set -x
B=./target/release/bakeoff
wait_for() { while pgrep -f "bakeoff $1" >/dev/null; do sleep 10; done; }
wait_for "bake --engine redb "
$B bake --engine mdbx --n 10000000 > logs/A-mdbx.log 2>&1
$B bake --engine sana --n 10000000 > logs/A-sana.log 2>&1
$B orders --engine redb --layout 3 --n 10000000 > logs/B-redb-3.log 2>&1
$B orders --engine redb --layout 6 --n 10000000 > logs/B-redb-6.log 2>&1
$B crash-prep --n 10000000 --db data/crash/base-10M.redb > logs/C-prep-10M.log 2>&1
$B crash-run --base data/crash/base-10M.redb --reps 3 --out results/C-crash-10M.json > logs/C-run-10M.log 2>&1
rm -f data/crash/base-10M.redb data/crash/base-10M.meta.json
$B crash-prep --n 100000000 --db data/crash/base-100M.redb > logs/C-prep-100M.log 2>&1
$B crash-run --base data/crash/base-100M.redb --reps 2 --modes clean,uncommitted_big,after_noflush,after_flush,qr_after_noflush --out results/C-crash-100M.json > logs/C-run-100M.log 2>&1
echo ALLDONE
