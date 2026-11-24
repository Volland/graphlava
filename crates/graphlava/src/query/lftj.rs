//! Leapfrog Triejoin (Veldhuizen, ICDT 2014) for cyclic conjunctions of
//! triple patterns (rules `plan-join-choice`, `plan-ltj-order`).
//!
//! Each atom is read once from the ordering whose prefix covers its
//! constants, projected onto its variables in the global variable order and
//! kept as a sorted trie of ID tuples; the join intersects, variable by
//! variable, the sorted ranges of every atom that mentions the variable with
//! galloping `seek`s. When an atom would want an on-demand ordering that is
//! not ready, the base ordering's rows are sorted in memory instead (the
//! fallback of [[adr:0041]]) and the miss is counted.

use std::collections::{HashMap, HashSet};

use crate::error::Result;
use crate::id::Id;
use crate::query::exec::{Exec, Row};
use crate::query::ir::*;

/// Whether the variable hypergraph of the parts is cyclic (GYO reduction).
pub fn is_cyclic(edges: &[Vec<Slot>]) -> bool {
    let mut es: Vec<HashSet<Slot>> = edges.iter().map(|e| e.iter().copied().collect()).collect();
    loop {
        let mut changed = false;
        // remove vertices occurring in exactly one edge
        let mut occ: HashMap<Slot, usize> = HashMap::new();
        for e in &es {
            for v in e {
                *occ.entry(*v).or_default() += 1;
            }
        }
        for e in es.iter_mut() {
            let before = e.len();
            e.retain(|v| occ[v] > 1);
            changed |= e.len() != before;
        }
        // remove edges contained in another edge (and empty edges)
        let mut i = 0;
        while i < es.len() {
            let contained = es[i].is_empty()
                || es
                    .iter()
                    .enumerate()
                    .any(|(j, f)| j != i && es[i].is_subset(f) && (es[i] != *f || j < i));
            if contained {
                es.remove(i);
                changed = true;
            } else {
                i += 1;
            }
        }
        if !changed {
            break;
        }
    }
    !es.is_empty()
}

struct Atom {
    /// Variables of the atom in global order.
    vars: Vec<Slot>,
    /// Sorted, deduplicated tuples over `vars`.
    rows: Vec<Vec<Id>>,
}

/// Runs the join with Leapfrog Triejoin when every part is a triple pattern
/// without a reifier and the pattern is cyclic; `None` otherwise.
pub fn try_join(ex: &Exec<'_>, parts: &[Node], input: &[Row]) -> Result<Option<Vec<(usize, Row)>>> {
    // Standard mode reads statement objects as triple terms at bind time,
    // which only the scan operator does.
    if parts.len() < 3 || input.is_empty() || ex.standard {
        return Ok(None);
    }
    let mut pats = Vec::new();
    for p in parts {
        match p {
            Node::Triple(t)
                if t.id.is_none() && !matches!(t.graph, GraphSpec::Graph { g: Pt::Var(_), .. }) =>
            {
                pats.push(t)
            }
            _ => return Ok(None),
        }
    }
    let width = input[0].len();
    let bound: HashSet<Slot> = (0..width)
        .filter(|s| input.iter().all(|r| !r[*s].is_null()))
        .collect();
    let free = |t: &TriplePat| -> Vec<Slot> {
        let mut v = Vec::new();
        for p in [&t.s, &t.p, &t.o] {
            if let Pt::Var(x) = p {
                if !bound.contains(x) && !v.contains(x) {
                    v.push(*x);
                }
            }
        }
        v
    };
    let edges: Vec<Vec<Slot>> = pats.iter().map(|t| free(t)).collect();
    if !is_cyclic(&edges) {
        return Ok(None);
    }
    // Global variable order: most frequent variables first.
    let mut freq: HashMap<Slot, usize> = HashMap::new();
    for e in &edges {
        for v in e {
            *freq.entry(*v).or_default() += 1;
        }
    }
    let mut order: Vec<Slot> = freq.keys().copied().collect();
    order.sort_by(|a, b| freq[b].cmp(&freq[a]).then(a.cmp(b)));
    let rank: HashMap<Slot, usize> = order.iter().enumerate().map(|(i, v)| (*v, i)).collect();

    // The ordering each atom wants: constants first, then its variables in
    // the global order. A wanted on-demand ordering that is not ready is a
    // miss; the atom is then sorted in memory (the fallback of [[adr:0041]]).
    for t in &pats {
        let pos = |p: &Pt| match p {
            Pt::Const(_) => 0usize,
            Pt::Var(v) if bound.contains(v) => 0,
            Pt::Var(v) => 1 + rank[v],
        };
        let mut cols = [('s', pos(&t.s)), ('p', pos(&t.p)), ('o', pos(&t.o))];
        cols.sort_by_key(|c| c.1);
        let key: String = cols.iter().map(|c| c.0).collect();
        let want = match key.as_str() {
            "pso" => Some(0),
            "sop" => Some(1),
            "ops" => Some(2),
            _ => None,
        };
        if let Some(k) = want {
            let ready = ex.stats.as_ref().is_some_and(|s| s.orderings[k] == 2);
            if !ready {
                let mut w = ex.wanted.get();
                w[k] += 1;
                ex.wanted.set(w);
            }
        }
    }
    let mut out = Vec::new();
    for (i, row) in input.iter().enumerate() {
        let mut atoms = Vec::with_capacity(pats.len());
        for t in &pats {
            let mut vars = free(t);
            vars.sort_by_key(|v| rank[v]);
            let val = |p: &Pt| match p {
                Pt::Const(c) => Some(*c),
                Pt::Var(v) if bound.contains(v) => Some(row[*v]),
                Pt::Var(_) => None,
            };
            let single = Node::Triple((*t).clone());
            let matches = ex.eval(&single, std::slice::from_ref(row))?;
            let _ = (val(&t.s), val(&t.p), val(&t.o));
            let mut rows: Vec<Vec<Id>> = matches
                .into_iter()
                .map(|(_, r)| vars.iter().map(|v| r[*v]).collect())
                .collect();
            rows.sort();
            rows.dedup();
            atoms.push(Atom { vars, rows });
        }
        let mut binding = row.clone();
        let ranges: Vec<(usize, usize)> = atoms.iter().map(|a| (0, a.rows.len())).collect();
        leapfrog(&atoms, &order, 0, ranges, &mut binding, &mut |r| {
            out.push((i, r.clone()))
        });
    }
    Ok(Some(out))
}

/// Narrows `range` of `atom` to the rows whose column `col` equals `key`.
fn narrow(atom: &Atom, col: usize, range: (usize, usize), key: Id) -> (usize, usize) {
    let rows = &atom.rows[range.0..range.1];
    let lo = rows.partition_point(|r| r[col] < key);
    let hi = rows.partition_point(|r| r[col] <= key);
    (range.0 + lo, range.0 + hi)
}

fn leapfrog(
    atoms: &[Atom],
    order: &[Slot],
    depth: usize,
    ranges: Vec<(usize, usize)>,
    binding: &mut Row,
    emit: &mut dyn FnMut(&Row),
) {
    if depth == order.len() {
        emit(binding);
        return;
    }
    let var = order[depth];
    // Atoms that mention this variable, with the column it sits in.
    let parts: Vec<(usize, usize)> = atoms
        .iter()
        .enumerate()
        .filter_map(|(i, a)| a.vars.iter().position(|v| *v == var).map(|c| (i, c)))
        .collect();
    if parts.is_empty() {
        leapfrog(atoms, order, depth + 1, ranges, binding, emit);
        return;
    }
    // Leapfrog over the sorted keys of each participating range: the
    // iterator with the smallest key seeks to the largest key so far.
    let mut pos: Vec<usize> = parts.iter().map(|(i, _)| ranges[*i].0).collect();
    loop {
        let mut max = Id(0);
        for (k, (i, c)) in parts.iter().enumerate() {
            if pos[k] >= ranges[*i].1 {
                return;
            }
            max = max.max(atoms[*i].rows[pos[k]][*c]);
        }
        let mut all_equal = true;
        for (k, (i, c)) in parts.iter().enumerate() {
            let a = &atoms[*i];
            let rows = &a.rows[pos[k]..ranges[*i].1];
            // seek: first row with key >= max (galloping through the range)
            let skip = rows.partition_point(|r| r[*c] < max);
            pos[k] += skip;
            if pos[k] >= ranges[*i].1 {
                return;
            }
            if a.rows[pos[k]][*c] != max {
                all_equal = false;
            }
        }
        if all_equal {
            let mut next = ranges.clone();
            for (i, c) in &parts {
                next[*i] = narrow(&atoms[*i], *c, ranges[*i], max);
            }
            let old = binding[var];
            binding[var] = max;
            leapfrog(atoms, order, depth + 1, next.clone(), binding, emit);
            binding[var] = old;
            // move every participant past `max`
            for (k, (i, _)) in parts.iter().enumerate() {
                pos[k] = next[*i].1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_is_cyclic_path_is_not() {
        assert!(is_cyclic(&[vec![0, 1], vec![1, 2], vec![2, 0]]));
        assert!(!is_cyclic(&[vec![0, 1], vec![1, 2], vec![2, 3]]));
        assert!(!is_cyclic(&[vec![0, 1], vec![0, 2], vec![0, 3]]));
    }

    /// Leapfrog intersection equals set intersection (test plan of
    /// [[14-planner-execution#Test plan]]).
    #[test]
    fn leapfrog_equals_set_intersection() {
        let mut x: u64 = 17;
        let mut next = || {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            (x >> 40) % 64
        };
        for _ in 0..50 {
            let sets: Vec<Vec<u64>> = (0..3)
                .map(|_| {
                    let mut v: Vec<u64> = (0..30).map(|_| next() + 1).collect();
                    v.sort();
                    v.dedup();
                    v
                })
                .collect();
            let atoms: Vec<Atom> = sets
                .iter()
                .map(|s| Atom {
                    vars: vec![0],
                    rows: s.iter().map(|v| vec![Id(*v)]).collect(),
                })
                .collect();
            let mut got = Vec::new();
            let mut binding = vec![Id(0)];
            let ranges = atoms.iter().map(|a| (0, a.rows.len())).collect();
            leapfrog(&atoms, &[0], 0, ranges, &mut binding, &mut |r| {
                got.push(r[0].raw())
            });
            let want: Vec<u64> = sets[0]
                .iter()
                .copied()
                .filter(|v| sets[1].contains(v) && sets[2].contains(v))
                .collect();
            assert_eq!(got, want);
        }
    }
}
