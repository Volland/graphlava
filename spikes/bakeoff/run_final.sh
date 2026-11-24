#!/bin/sh
# Final queue: C10 crash-run, bytes/request, C100, redb4 comparison.
B=./target/release/bakeoff
echo "C10 run start $(date)"
$B crash-run --base data/crash/base-10M.redb --reps 3 --out results/C-crash-10M.json > logs/C-run-10M.log 2>&1
$B crash-reqbytes --base data/crash/base-10M.redb --out results/C-reqbytes-10M.json > logs/C-reqbytes.log 2>&1
rm -f data/crash/base-10M.redb data/crash/base-10M.meta.json
echo "C100 start $(date)"
$B crash-prep --n 100000000 --db data/crash/base-100M.redb > logs/C-prep-100M.log 2>&1
$B crash-run --base data/crash/base-100M.redb --reps 2 --modes clean,uncommitted_big,after_noflush,after_flush,qr_after_noflush --out results/C-crash-100M.json > logs/C-run-100M.log 2>&1
rm -f data/crash/base-100M.redb data/crash/base-100M.meta.json
echo "redb4 start $(date)"
$B bake --engine redb4 --n 10000000 --phases lat,tp,probe --out results/A-bake-redb4-10000000.json > logs/A-redb4.log 2>&1
rm -rf data/bake-redb4-10000000
echo "ALLDONE $(date)"
