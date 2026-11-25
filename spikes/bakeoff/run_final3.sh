#!/bin/sh
B=./target/release/bakeoff
echo "C100 run start $(date)"
BAKEOFF_BIG=100000 $B crash-run --base data/crash/base-100M.redb --reps 2 --modes clean,uncommitted_big,after_noflush,after_flush,qr_after_noflush --out results/C-crash-100M.json > logs/C-run-100M.log 2>&1
rm -f data/crash/base-100M.redb data/crash/base-100M.meta.json
echo "redb4 start $(date)"
$B bake --engine redb4 --n 10000000 --phases lat,tp,probe --out results/A-bake-redb4-10000000.json > logs/A-redb4.log 2>&1
rm -rf data/bake-redb4-10000000
echo "ALLDONE $(date)"
