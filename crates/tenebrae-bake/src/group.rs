//! Object grouping ([`super::Params::group_cost`]): joins the baked objects into fewer, larger
//! ones, as Umbra's `ObjectGrouper` joins a scene's objects before the bake (the stock tomes
//! hold about one object per two engine users).
//!
//! The grouper is Umbra's: an agglomerative bounding-volume tree whose pair cost is
//! `(A_world - A_union) * OGC + (cost_i + cost_j) * A_union` over box surface areas, then a
//! saturation (primal-dual) cut that keeps a leaf single when its own cost is the tightest on
//! its chain to the root. Only the ratio of the per-object cost to the grouping cost `OGC`
//! matters, so one number sets it. Large boxes have little uncovered world area and stay
//! single; small boxes merge until an ancestor saturates (the stock parity tome: nothing under 1 m
//! single, 91 % of everything over 32 m single).
//!
//! The pass runs on the emitted leaf-only tome, before the LOD tiles: a group's box is the
//! union of its members' boxes, its user list their lists in order, and every cell's object
//! list the union of its members' entries. Grouping leaves cells and portals unchanged.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};

use tenebrae_tome::{Aabb, Run, RunList, Tome, Users};

/// Neighbours considered per node when pairing (Umbra: 32).
const NEIGHBOURS: usize = 32;
/// Umbra's saturation tolerance in the cut.
const SLACK: f64 = 0.001;

/// Surface area of a box.
fn area(b: &Aabb) -> f64 {
    let d: [f64; 3] = std::array::from_fn(|a| f64::from(b.max[a] - b.min[a]).max(0.0));
    2.0 * (d[0] * d[1] + d[0] * d[2] + d[1] * d[2])
}

fn union(a: &Aabb, b: &Aabb) -> Aabb {
    Aabb {
        min: std::array::from_fn(|k| a.min[k].min(b.min[k])),
        max: std::array::from_fn(|k| a.max[k].max(b.max[k])),
    }
}

/// A heap entry: the cheapest pair first.
#[derive(PartialEq)]
struct Pair {
    cost: f64,
    a: usize,
    b: usize,
}

impl Eq for Pair {}

impl PartialOrd for Pair {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Pair {
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost
            .total_cmp(&self.cost)
            .then(o.a.cmp(&self.a))
            .then(o.b.cmp(&self.b))
    }
}

/// A node of the grouping tree.
struct Node {
    bounds: Aabb,
    children: Option<[usize; 2]>,
    parent: Option<usize>,
    /// Summed per-object cost.
    cost: f64,
    /// The node's own cost `c` (its pair cost; a leaf's single cost).
    c: f64,
}

/// Groups `boxes` with per-object cost `cost` against a grouping cost of 1: per box, a dense
/// group id in first-seen order. Every box is its own group at `cost` 0 or with fewer than two
/// boxes.
pub(crate) fn group(boxes: &[Aabb], cost: f64) -> Vec<u32> {
    let n = boxes.len();
    if n < 2 || cost.partial_cmp(&0.0) != Some(Ordering::Greater) {
        return (0..n as u32).collect();
    }
    let world = boxes[1..].iter().fold(boxes[0], |w, b| union(&w, b));
    let a_world = area(&world);
    let mut nodes: Vec<Node> = boxes
        .iter()
        .map(|b| {
            let a = area(b);
            Node {
                bounds: *b,
                children: None,
                parent: None,
                cost,
                c: (a_world - a) + a * cost,
            }
        })
        .collect();
    let pair_cost = |nodes: &[Node], i: usize, j: usize| {
        let a = area(&union(&nodes[i].bounds, &nodes[j].bounds));
        (a_world - a) + (nodes[i].cost + nodes[j].cost) * a
    };
    // The `NEIGHBOURS` roots whose union with `i` has the least surface area.
    let pairs_of = |nodes: &[Node], roots: &[usize], i: usize| -> Vec<Pair> {
        let mut near: Vec<(f64, usize)> = roots
            .iter()
            .filter(|&&j| j != i)
            .map(|&j| (area(&union(&nodes[i].bounds, &nodes[j].bounds)), j))
            .collect();
        near.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        near.truncate(NEIGHBOURS);
        near.into_iter()
            .map(|(_, j)| Pair {
                cost: pair_cost(nodes, i, j),
                a: i.min(j),
                b: i.max(j),
            })
            .collect()
    };
    let mut roots: Vec<usize> = (0..n).collect();
    let mut heap = BinaryHeap::new();
    for &i in &roots {
        heap.extend(pairs_of(&nodes, &roots, i));
    }
    while roots.len() > 1 {
        let Some(Pair { cost: c, a, b }) = heap.pop() else {
            // No pair left between roots: pair every root again (at least one pair is valid).
            for &i in &roots {
                heap.extend(pairs_of(&nodes, &roots, i));
            }
            continue;
        };
        if nodes[a].parent.is_some() || nodes[b].parent.is_some() {
            continue;
        }
        let k = nodes.len();
        nodes.push(Node {
            bounds: union(&nodes[a].bounds, &nodes[b].bounds),
            children: Some([a, b]),
            parent: None,
            cost: nodes[a].cost + nodes[b].cost,
            c,
        });
        nodes[a].parent = Some(k);
        nodes[b].parent = Some(k);
        roots.retain(|&r| r != a && r != b);
        heap.extend(pairs_of(&nodes, &roots, k));
        roots.push(k);
    }
    let root = roots[0];
    // Phase A: saturate. Leaves in right-first order under unsaturated nodes; each raises
    // `paid` on its chain (root excluded) by the least slack, which saturates one node.
    let mut paid = vec![0.0f64; nodes.len()];
    let saturated = |nodes: &[Node], paid: &[f64], v: usize| nodes[v].c - paid[v] <= 0.0;
    loop {
        let mut stack = vec![root];
        let mut leaf = None;
        while let Some(v) = stack.pop() {
            if v != root && saturated(&nodes, &paid, v) {
                continue;
            }
            match nodes[v].children {
                Some([l, r]) => {
                    stack.push(l);
                    stack.push(r);
                }
                None => {
                    leaf = Some(v);
                    break;
                }
            }
        }
        let Some(leaf) = leaf else { break };
        let mut chain = vec![leaf];
        while let Some(p) = nodes[*chain.last().unwrap_or(&leaf)].parent {
            if p == root {
                break;
            }
            chain.push(p);
        }
        let (tight, j) = chain
            .iter()
            .map(|&v| (v, nodes[v].c - paid[v]))
            .fold((leaf, f64::INFINITY), |m, x| if x.1 < m.1 { x } else { m });
        for &v in &chain {
            paid[v] += j;
        }
        paid[tight] = nodes[tight].c;
    }
    // Phase B: top down, a node splits when it is not saturated or both children are internal
    // and saturated; otherwise it is a group root.
    let mut group_of = vec![u32::MAX; n];
    let mut next = 0u32;
    let mut stack = vec![root];
    let mut members = Vec::new();
    while let Some(v) = stack.pop() {
        let split = match nodes[v].children {
            Some([l, r]) => {
                (nodes[v].c - paid[v]).abs() >= SLACK
                    || (nodes[l].children.is_some()
                        && nodes[r].children.is_some()
                        && saturated(&nodes, &paid, l)
                        && saturated(&nodes, &paid, r))
            }
            None => false,
        };
        if split {
            let [l, r] = nodes[v].children.unwrap_or([v, v]);
            stack.push(l);
            stack.push(r);
            continue;
        }
        members.clear();
        members.push(v);
        while let Some(u) = members.pop() {
            match nodes[u].children {
                Some([l, r]) => {
                    members.push(l);
                    members.push(r);
                }
                None => group_of[u] = u32::MAX - 1 - next,
            }
        }
        next += 1;
    }
    // Dense ids in first-seen object order.
    let mut dense: BTreeMap<u32, u32> = BTreeMap::new();
    for g in &mut group_of {
        let k = dense.len() as u32;
        *g = *dense.entry(*g).or_insert(k);
    }
    group_of
}

/// Bits that hold `n`.
fn bits_for(n: u32) -> u32 {
    32 - n.leading_zeros()
}

/// Replaces the objects of the leaf-only tome `t` by their groups (`group_of`, one dense id
/// per object), updating the object boxes, user lists, every cell's object list and the
/// target map `map`. Lists are rebuilt packed as [`super::global::pack_lists`] packs them when
/// `share` is set.
pub(crate) fn apply(t: &mut Tome, map: &mut [Option<u32>], group_of: &[u32], share: bool) {
    let n = group_of.iter().map(|&g| g as usize + 1).max().unwrap_or(0);
    if n == 0 || n == group_of.len() {
        return;
    }
    let mut boxes: Vec<Option<Aabb>> = vec![None; n];
    let mut users: Vec<Vec<u32>> = vec![Vec::new(); n];
    let user_list = |o: usize| -> &[u32] {
        match &t.users.prefix {
            Some(p) => &t.users.ids[p[o] as usize..p[o + 1] as usize],
            None => &t.users.ids[o..o + 1],
        }
    };
    for (o, &g) in group_of.iter().enumerate() {
        let g = g as usize;
        boxes[g] = Some(boxes[g].map_or(t.objects[o], |b| union(&b, &t.objects[o])));
        users[g].extend_from_slice(user_list(o));
    }
    // Every group has a member, so every box is set.
    t.objects = boxes.into_iter().flatten().collect();
    let multi = users.iter().any(|u| u.len() != 1);
    let mut prefix = vec![0u32];
    let mut ids = Vec::new();
    for u in &users {
        ids.extend_from_slice(u);
        prefix.push(ids.len() as u32);
    }
    t.users = Users {
        prefix: multi.then_some(prefix),
        ids,
    };
    for o in map.iter_mut().flatten() {
        *o = group_of[*o as usize];
    }
    // Every cell's list, remapped.
    let old = std::mem::replace(
        &mut t.object_runs,
        RunList {
            index_bits: bits_for(n.saturating_sub(1) as u32).max(1),
            run_bits: bits_for(31),
            entries: Vec::new(),
        },
    );
    let expand = |start: u32, count: u32| -> Vec<u32> {
        let mut out = Vec::with_capacity(count as usize);
        let mut at = start as usize;
        while (out.len() as u32) < count {
            let r = old.entries[at];
            let take = (count - out.len() as u32).min(r.run);
            out.extend((r.index..r.index + take).map(|o| group_of[o as usize]));
            at += 1;
        }
        out.sort_unstable();
        out.dedup();
        out
    };
    let runs_of = |list: &[u32]| -> Vec<Run> {
        let mut runs = Vec::new();
        let mut k = 0;
        while k < list.len() {
            let mut run = 1;
            while k + run < list.len() && list[k + run] == list[k] + run as u32 && run < 31 {
                run += 1;
            }
            runs.push(Run {
                index: list[k],
                run: run as u32,
            });
            k += run;
        }
        runs
    };
    let mut lists: Vec<(usize, usize, Vec<Run>)> = Vec::new();
    for (node, tile) in t.tiles.iter_mut().enumerate() {
        let Some(tile) = tile else { continue };
        for (i, cell) in tile.cells.iter_mut().enumerate() {
            let list = expand(cell.0[2], cell.0[3]);
            cell.0[3] = list.len() as u32;
            let runs = runs_of(&list);
            if share {
                lists.push((node, i, runs));
            } else {
                cell.0[2] = t.object_runs.entries.len() as u32;
                t.object_runs.entries.extend(runs);
            }
        }
    }
    if share {
        for (node, i, start) in super::global::pack_lists(&mut t.object_runs.entries, lists) {
            if let Some(tile) = &mut t.tiles[node] {
                tile.cells[i].0[2] = start;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(min: [f32; 3], size: f32) -> Aabb {
        Aabb {
            min,
            max: [min[0] + size, min[1] + size, min[2] + size],
        }
    }

    /// Two far-apart large boxes stay single; a cluster of small boxes between them joins into
    /// one group; ids are dense in first-seen order. A pair merges when its pair cost is below
    /// its members' summed single costs, i.e. (for small boxes) when the union's surface area
    /// is under about `A_world / (2 cost)`: here 41,600 / 2,000 = 20.8 m², which eight 0.5 m
    /// cubes in a row (about 10 m²) satisfy and a 40 m cube (9,600 m²) never does. (At the stock
    /// 10,000 on the parity scene's 240,000 m² world the bound is 12 m², i.e. boxes to about 1.4 m,
    /// matching the stock profile.)
    #[test]
    fn large_boxes_stay_single_and_small_ones_cluster() {
        let mut boxes = vec![b([0.0, 0.0, 0.0], 40.0), b([200.0, 0.0, 0.0], 40.0)];
        for i in 0..8 {
            boxes.push(b([100.0 + i as f32 * 0.6, 0.0, 0.0], 0.5));
        }
        let g = group(&boxes, 1000.0);
        assert_eq!(g[0], 0);
        assert_eq!(g[1], 1);
        assert!(g[2..].iter().all(|&x| x == g[2]), "{g:?}");
        assert_ne!(g[2], 0);
        assert_ne!(g[2], 1);
        let mut ids = g.clone();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids, (0..ids.len() as u32).collect::<Vec<_>>());
    }

    /// Cost 0 and fewer than two boxes group nothing.
    #[test]
    fn off_or_trivial_inputs_keep_every_box() {
        let boxes = vec![b([0.0; 3], 1.0), b([5.0; 3], 1.0)];
        assert_eq!(group(&boxes, 0.0), vec![0, 1]);
        assert_eq!(group(&boxes[..1], 40_000.0), vec![0]);
        assert_eq!(group(&[], 40_000.0), Vec::<u32>::new());
    }
}
