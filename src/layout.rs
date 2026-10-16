//! Dwindle-style tiling: a binary tree of splits whose leaves are tab ids.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct R {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl R {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Node {
    Leaf(u64),
    /// `v`: side by side (vertical divider). `r`: share of space given to `a`.
    Split { v: bool, r: f64, a: Box<Node>, b: Box<Node> },
}

/// A draggable gap between the two children of a split.
#[derive(Serialize)]
pub struct Splitter {
    pub path: String,
    pub v: bool,
    pub gap: R,
    pub parent: R,
}

fn halves(v: bool, ratio: f64, r: R, gap: f64) -> (R, R, R) {
    if v {
        let aw = ((r.w - gap) * ratio).max(1.0);
        let a = R { w: aw, ..r };
        let g = R { x: r.x + aw, w: gap, ..r };
        let b = R { x: r.x + aw + gap, w: (r.w - gap - aw).max(1.0), ..r };
        (a, g, b)
    } else {
        let ah = ((r.h - gap) * ratio).max(1.0);
        let a = R { h: ah, ..r };
        let g = R { y: r.y + ah, h: gap, ..r };
        let b = R { y: r.y + ah + gap, h: (r.h - gap - ah).max(1.0), ..r };
        (a, g, b)
    }
}

impl Node {
    pub fn is_leaf(&self, id: u64) -> bool {
        matches!(self, Node::Leaf(x) if *x == id)
    }

    pub fn contains(&self, id: u64) -> bool {
        match self {
            Node::Leaf(x) => *x == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Leaf ids, left/top first.
    pub fn leaves(&self, out: &mut Vec<u64>) {
        match self {
            Node::Leaf(x) => out.push(*x),
            Node::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    pub fn first_leaf(&self) -> u64 {
        match self {
            Node::Leaf(x) => *x,
            Node::Split { a, .. } => a.first_leaf(),
        }
    }

    pub fn rects(&self, r: R, gap: f64, out: &mut Vec<(u64, R)>) {
        match self {
            Node::Leaf(x) => out.push((*x, r)),
            Node::Split { v, r: ratio, a, b } => {
                let (ra, _, rb) = halves(*v, *ratio, r, gap);
                a.rects(ra, gap, out);
                b.rects(rb, gap, out);
            }
        }
    }

    pub fn splitters(&self, r: R, gap: f64, path: &mut String, out: &mut Vec<Splitter>) {
        if let Node::Split { v, r: ratio, a, b } = self {
            let (ra, g, rb) = halves(*v, *ratio, r, gap);
            out.push(Splitter { path: path.clone(), v: *v, gap: g, parent: r });
            path.push('a');
            a.splitters(ra, gap, path, out);
            path.pop();
            path.push('b');
            b.splitters(rb, gap, path, out);
            path.pop();
        }
    }

    /// Remove a leaf, collapsing its parent split. The last leaf can't be removed.
    pub fn remove(&mut self, id: u64) -> bool {
        let Node::Split { a, b, .. } = self else { return false };
        if a.is_leaf(id) {
            let keep = std::mem::replace(&mut **b, Node::Leaf(0));
            *self = keep;
            return true;
        }
        if b.is_leaf(id) {
            let keep = std::mem::replace(&mut **a, Node::Leaf(0));
            *self = keep;
            return true;
        }
        a.remove(id) || b.remove(id)
    }

    pub fn replace(&mut self, old: u64, new: u64) -> bool {
        match self {
            Node::Leaf(x) if *x == old => {
                *x = new;
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.replace(old, new) || b.replace(old, new),
        }
    }

    pub fn swap(&mut self, x: u64, y: u64) {
        const TMP: u64 = u64::MAX;
        self.replace(x, TMP);
        self.replace(y, x);
        self.replace(TMP, y);
    }

    /// Split the `target` leaf, putting `new` before (left/top) or after it.
    pub fn split(&mut self, target: u64, new: u64, v: bool, new_first: bool) -> bool {
        match self {
            Node::Leaf(x) if *x == target => {
                let (a, b) = if new_first { (new, target) } else { (target, new) };
                *self = Node::Split {
                    v,
                    r: 0.5,
                    a: Box::new(Node::Leaf(a)),
                    b: Box::new(Node::Leaf(b)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                a.split(target, new, v, new_first) || b.split(target, new, v, new_first)
            }
        }
    }

    pub fn set_ratio(&mut self, path: &str, ratio: f64) {
        let mut n = self;
        for c in path.chars() {
            match n {
                Node::Split { a, b, .. } => n = if c == 'a' { a } else { b },
                Node::Leaf(_) => return,
            }
        }
        if let Node::Split { r, .. } = n {
            *r = ratio.clamp(0.1, 0.9);
        }
    }

    pub fn map(&self, f: &impl Fn(u64) -> Option<u64>) -> Option<Node> {
        match self {
            Node::Leaf(x) => f(*x).map(Node::Leaf),
            Node::Split { v, r, a, b } => match (a.map(f), b.map(f)) {
                (Some(a), Some(b)) => Some(Node::Split { v: *v, r: *r, a: Box::new(a), b: Box::new(b) }),
                (Some(n), None) | (None, Some(n)) => Some(n),
                (None, None) => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_remove_swap() {
        let mut n = Node::Leaf(1);
        assert!(n.split(1, 2, true, false));
        assert!(n.split(2, 3, false, true));
        let mut v = Vec::new();
        n.rects(R { x: 0.0, y: 0.0, w: 106.0, h: 106.0 }, 6.0, &mut v);
        let ids: Vec<u64> = v.iter().map(|x| x.0).collect();
        assert_eq!(ids, vec![1, 3, 2]);
        assert_eq!(v[0].1.w, 50.0);
        n.swap(1, 2);
        assert_eq!(n.first_leaf(), 2);
        assert!(n.remove(3));
        assert!(n.remove(1));
        assert!(n.is_leaf(2));
        assert!(!n.remove(2));
    }
}
