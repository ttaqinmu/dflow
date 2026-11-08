use crate::graph::Impact;
use petgraph::algo::kosaraju_scc;
use petgraph::stable_graph::StableDiGraph;
use std::collections::{BTreeMap, BTreeSet};

const MIN_BOX_WIDTH: i32 = 26;
const COLUMN_GAP: i32 = 8;
const ROW_GAP: i32 = 3;

#[derive(Debug)]
pub struct Component {
    pub members: Vec<String>,
    pub column: usize,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub title: String,
    pub description: String,
    pub extra: Vec<String>,
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
    let mut components = Vec::new();
    for (id, members) in groups.into_iter().enumerate() {
        let distance = levels[id].unwrap_or(0);
        let column = max_level - distance;
        let cycle = members.len() > 1
            || impact
                .edges
                .iter()
                .any(|(caller, callee)| caller == callee && members.contains(caller));
        components.push(Component {
            members,
            column,
            x: 0,
            y: 0,
            width: MIN_BOX_WIDTH,
            height: 5,
            title: String::new(),
            description: String::new(),
            extra: Vec::new(),
            cycle,
        });
    }
    let mut visual: Vec<_> = (0..components.len()).collect();
    visual.sort_by_key(|id| (components[*id].column, *id));
    let order = visual
        .into_iter()
        .flat_map(|id| components[id].members.iter().cloned())
        .collect();
    Layout {
        components,
        edges: edges.into_iter().collect(),
        order,
        width: 0,
        height: 0,
    }
}

pub fn place(view: &mut Layout) {
    if view.components.is_empty() {
        return;
    }
    let columns = view.components.iter().map(|c| c.column).max().unwrap() + 1;
    let mut widths = vec![MIN_BOX_WIDTH; columns];
    for component in &mut view.components {
        component.width = (component
            .extra
            .iter()
            .map(String::len)
            .chain([component.title.len(), component.description.len()])
            .max()
            .unwrap_or(0) as i32
            + 2)
        .max(MIN_BOX_WIDTH);
        component.height = 5 + if component.extra.is_empty() {
            0
        } else {
            component.extra.len() as i32 + 1
        };
        widths[component.column] = widths[component.column].max(component.width);
    }
    let mut positions = Vec::with_capacity(columns);
    let mut x = 0;
    for width in widths {
        positions.push(x);
        x += width + COLUMN_GAP;
    }
    let mut rows = vec![0; columns];
    for component in &mut view.components {
        component.x = positions[component.column];
        component.y = rows[component.column];
        rows[component.column] += component.height + ROW_GAP;
    }
    view.width = view
        .components
        .iter()
        .map(|component| component.x + component.width)
        .max()
        .unwrap_or(0);
    view.height = view
        .components
        .iter()
        .map(|component| component.y + component.height)
        .max()
        .unwrap_or(0);
}
