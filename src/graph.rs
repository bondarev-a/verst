use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap};
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

/// Текущий файл всегда первый, дальше до `MAX_NODES - 1` соседей: больше
/// связей — раньше, при равенстве по пути. Без соседей — самые связанные
/// файлы пространства. Порядок не зависит от обхода HashMap, так что граф
/// не перескакивает между переиндексациями.
fn select_nodes(index: &LinkIndex, current: Option<&Path>) -> Vec<PathBuf> {
    let mut degree: HashMap<&Path, usize> = HashMap::new();
    for (from, targets) in &index.outgoing {
        *degree.entry(from.as_path()).or_default() += targets.len();
        for target in targets {
            *degree.entry(target.as_path()).or_default() += 1;
        }
    }
    let current = current.filter(|path| index.files.iter().any(|file| file == path));
    let mut chosen = Vec::new();
    let mut candidates: BTreeSet<&Path> = BTreeSet::new();
    if let Some(path) = current {
        chosen.push(path.to_path_buf());
        candidates.extend(index.outgoing_of(path).iter().map(PathBuf::as_path));
        candidates.extend(index.incoming_of(path).iter().map(PathBuf::as_path));
    }
    if candidates.iter().all(|item| Some(*item) == current) {
        candidates.extend(degree.keys().copied());
    }
    if let Some(path) = current {
        candidates.remove(path);
    }
    let mut ranked: Vec<&Path> = candidates.into_iter().collect();
    // Сортировка устойчивая, а BTreeSet уже по пути: равные идут по алфавиту.
    ranked.sort_by_key(|path| Reverse(degree.get(path).copied().unwrap_or(0)));
    let room = MAX_NODES.saturating_sub(chosen.len());
    chosen.extend(ranked.into_iter().take(room).map(Path::to_path_buf));
    chosen
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

    #[test]
    fn current_file_stays_among_many_neighbours() {
        let mut index = LinkIndex::default();
        let current = PathBuf::from("/repo/яблоко.md");
        let hub = PathBuf::from("/repo/хаб.md");
        let mut files = vec![current.clone(), hub.clone()];
        let mut targets = Vec::new();
        for n in 0..60 {
            let path = PathBuf::from(format!("/repo/заметка {n:02}.md"));
            files.push(path.clone());
            targets.push(path.clone());
            index.incoming.insert(path, vec![current.clone()]);
        }
        targets.push(hub.clone());
        index.outgoing.insert(hub.clone(), vec![current.clone()]);
        index.incoming.insert(current.clone(), vec![hub.clone()]);
        index.outgoing.insert(current.clone(), targets);
        index.files = files;
        let first = select_nodes(&index, Some(&current));
        assert_eq!(first.len(), MAX_NODES);
        assert_eq!(first[0], current);
        // У хаба две связи, у остальных по одной: он первый из соседей.
        assert_eq!(first[1], hub);
        assert_eq!(first[2], PathBuf::from("/repo/заметка 00.md"));
        for _ in 0..5 {
            assert_eq!(select_nodes(&index.clone(), Some(&current)), first);
        }
    }

    #[test]
    fn unlinked_current_file_comes_first_then_busiest() {
        let mut index = LinkIndex::default();
        let lonely = PathBuf::from("/repo/я одна.md");
        let a = PathBuf::from("/repo/а.md");
        let b = PathBuf::from("/repo/б.md");
        let c = PathBuf::from("/repo/в.md");
        let d = PathBuf::from("/repo/г.md");
        index.files = vec![lonely.clone(), a.clone(), b.clone(), c.clone(), d.clone()];
        // Связей: б — 3, а и в — по 2, г — 1.
        index.outgoing.insert(a.clone(), vec![b.clone(), c.clone()]);
        index.outgoing.insert(c.clone(), vec![b.clone()]);
        index.outgoing.insert(d.clone(), vec![b.clone()]);
        let nodes = select_nodes(&index, Some(&lonely));
        assert_eq!(nodes, vec![lonely, b.clone(), a.clone(), c.clone(), d.clone()]);
        let nodes = select_nodes(&index, None);
        assert_eq!(nodes, vec![b, a, c, d]);
    }
}
