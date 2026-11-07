use crate::graph::Impact;
use petgraph::algo::kosaraju_scc;
use petgraph::stable_graph::StableDiGraph;
use std::collections::{BTreeMap, BTreeSet};

pub const BOX_WIDTH: i32 = 26;
pub const BOX_HEIGHT: i32 = 3;
const COLUMN_GAP: i32 = 8;
const ROW_GAP: i32 = 3;

#[derive(Debug)]
pub struct Component {
    pub members: Vec<String>,
    pub x: i32,
    pub y: i32,
    pub cycle: bool,
}

#[derive(Debug, Default)]
pub struct Layout {
    pub components: Vec<Component>,
    pub edges: Vec<(usize, usize)>,
    pub order: Vec<String>,
    pub width: i32,
    pub height: i32,
}

fn level(id: usize, edges: &BTreeSet<(usize, usize)>, cache: &mut [Option<usize>]) -> usize {
    if let Some(value) = cache[id] {
        return value;
    }
    let value = edges
        .iter()
        .filter(|(caller, _)| *caller == id)
        .map(|(_, callee)| level(*callee, edges, cache) + 1)
        .max()
        .unwrap_or(0);
    cache[id] = Some(value);
    value
}

pub fn build(impact: &Impact) -> Layout {
    if impact.nodes.is_empty() {
        return Layout::default();
    }
    let mut focused = StableDiGraph::<String, ()>::new();
    let mut indices = BTreeMap::new();
    for (id, _) in &impact.nodes {
        indices.insert(id.clone(), focused.add_node(id.clone()));
    }
    for (caller, callee) in &impact.edges {
        focused.add_edge(indices[caller], indices[callee], ());
    }
    let mut groups: Vec<Vec<String>> = kosaraju_scc(&focused)
        .into_iter()
        .map(|group| {
            let mut members: Vec<_> = group
                .into_iter()
                .map(|node| focused[node].clone())
                .collect();
            members.sort();
            members
        })
        .collect();
    groups.sort_by(|a, b| a[0].cmp(&b[0]));
    let member_group: BTreeMap<_, _> = groups
        .iter()
        .enumerate()
        .flat_map(|(group, members)| members.iter().map(move |id| (id.clone(), group)))
        .collect();
    let edges: BTreeSet<_> = impact
        .edges
        .iter()
        .filter_map(|(caller, callee)| {
            let from = member_group[caller];
            let to = member_group[callee];
            (from != to).then_some((from, to))
        })
        .collect();
    let mut levels = vec![None; groups.len()];
    for id in 0..groups.len() {
        level(id, &edges, &mut levels);
    }
    let max_level = levels.iter().flatten().copied().max().unwrap_or(0);
    let mut rows = BTreeMap::<usize, i32>::new();
    let mut components = Vec::new();
    for (id, members) in groups.into_iter().enumerate() {
        let distance = levels[id].unwrap_or(0);
        let row = rows.entry(distance).or_default();
        let x = (max_level - distance) as i32 * (BOX_WIDTH + COLUMN_GAP);
        let y = *row * (BOX_HEIGHT + ROW_GAP);
        *row += 1;
        let cycle = members.len() > 1
            || impact
                .edges
                .iter()
                .any(|(caller, callee)| caller == callee && members.contains(caller));
        components.push(Component {
            members,
            x,
            y,
            cycle,
        });
    }
    let mut visual: Vec<_> = (0..components.len()).collect();
    visual.sort_by_key(|id| (components[*id].x, components[*id].y));
    let order = visual
        .into_iter()
        .flat_map(|id| components[id].members.iter().cloned())
        .collect();
    let width = components
        .iter()
        .map(|component| component.x + BOX_WIDTH)
        .max()
        .unwrap_or(0);
    let height = components
        .iter()
        .map(|component| component.y + BOX_HEIGHT)
        .max()
        .unwrap_or(0);
    Layout {
        components,
        edges: edges.into_iter().collect(),
        order,
        width,
        height,
    }
}
