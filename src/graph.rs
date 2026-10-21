use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::index::LinkIndex;

const MAX_NODES: usize = 36;

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedNode {
    pub path: PathBuf,
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphLayout {
    pub nodes: Vec<PlacedNode>,
    pub edges: Vec<GraphEdge>,
}

impl GraphLayout {
    pub fn hit(&self, x: f64, y: f64) -> Option<&PlacedNode> {
        self.nodes
            .iter()
            .filter(|node| dist(node.x, node.y, x, y) <= 22.0)
            .min_by(|a, b| {
                dist(a.x, a.y, x, y)
                    .partial_cmp(&dist(b.x, b.y, x, y))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }
}

pub fn layout(index: &LinkIndex, current: Option<&Path>, width: f64, height: f64) -> GraphLayout {
    let paths = select_nodes(index, current);
    let current_path = current.map(Path::to_path_buf);
    let mut nodes = Vec::with_capacity(paths.len());
    let cx = width / 2.0;
    let cy = height / 2.0;
    let radius = (width.min(height) / 2.0 - 40.0).max(24.0);
    let center_current = current_path
        .as_ref()
        .is_some_and(|path| paths.first() == Some(path))
        && paths.len() > 1;
    let ring = if center_current {
        paths.len() - 1
    } else {
        paths.len()
    };
    for (index, path) in paths.iter().enumerate() {
        let is_current = current_path.as_ref() == Some(path);
        let (x, y) = if center_current && index == 0 {
            (cx, cy)
        } else {
            let slot = if center_current { index - 1 } else { index };
            let denom = ring.max(1) as f64;
            let angle = slot as f64 / denom * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
            (cx + radius * angle.cos(), cy + radius * angle.sin())
        };
        nodes.push(PlacedNode {
            path: path.clone(),
            label: label_of(path),
            x,
            y,
            current: is_current,
        });
    }
    let mut edges = Vec::new();
    for (from_index, from) in paths.iter().enumerate() {
        for target in index.outgoing_of(from) {
            if let Some(to_index) = paths.iter().position(|path| path == target) {
                if from_index != to_index {
                    edges.push(GraphEdge {
                        from: from_index,
                        to: to_index,
                    });
                }
            }
        }
    }
    GraphLayout { nodes, edges }
}

fn select_nodes(index: &LinkIndex, current: Option<&Path>) -> Vec<PathBuf> {
    let mut chosen = BTreeSet::new();
    if let Some(path) = current {
        if index.files.iter().any(|file| file == path) {
            chosen.insert(path.to_path_buf());
            for item in index.outgoing_of(path) {
                chosen.insert(item.clone());
            }
            for item in index.incoming_of(path) {
                chosen.insert(item.clone());
            }
        }
    }
    if chosen.len() <= 1 {
        for (from, targets) in &index.outgoing {
            chosen.insert(from.clone());
            for target in targets {
                chosen.insert(target.clone());
            }
            if chosen.len() >= MAX_NODES {
                break;
            }
        }
    }
    let mut paths: Vec<PathBuf> = chosen.into_iter().take(MAX_NODES).collect();
    if let Some(path) = current {
        if let Some(pos) = paths.iter().position(|item| item == path) {
            let current_path = paths.remove(pos);
            paths.insert(0, current_path);
        }
    }
    paths
}

fn label_of(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("файл");
    let mut label: String = stem.chars().take(18).collect();
    if stem.chars().count() > 18 {
        label.push('…');
    }
    label
}

fn dist(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let dx = x1 - x2;
    let dy = y1 - y2;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_file_sits_in_the_center() {
        let mut index = LinkIndex::default();
        let a = PathBuf::from("/repo/a.md");
        let b = PathBuf::from("/repo/b.md");
        index.files = vec![a.clone(), b.clone()];
        index.outgoing.insert(a.clone(), vec![b.clone()]);
        index.incoming.insert(b.clone(), vec![a.clone()]);
        let layout = layout(&index, Some(&a), 400.0, 300.0);
        assert_eq!(layout.nodes[0].path, a);
        assert!((layout.nodes[0].x - 200.0).abs() < 0.1);
        assert!((layout.nodes[0].y - 150.0).abs() < 0.1);
        assert_eq!(layout.edges.len(), 1);
        assert!(layout.hit(200.0, 150.0).unwrap().current);
    }
}
