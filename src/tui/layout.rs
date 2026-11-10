use crate::graph::{EdgeChangeKind, Impact};
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
    pub cluster: usize,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub title: String,
    pub description: String,
    pub extra: Vec<String>,
    pub edge_labels: Vec<String>,
    pub cycle: bool,
}

#[derive(Debug)]
pub struct LayoutEdge {
    pub from: usize,
    pub to: usize,
    pub caller: String,
    pub callee: String,
    pub port: usize,
    pub lane_x: i32,
    pub route_y: i32,
    pub status: Option<EdgeChangeKind>,
}

#[derive(Debug, Default)]
pub struct Layout {
    pub components: Vec<Component>,
    pub edges: Vec<LayoutEdge>,
    pub order: Vec<String>,
    pub width: i32,
    pub height: i32,
}

fn clear_segment(components: &[Component], from: (i32, i32), to: (i32, i32)) -> bool {
    components.iter().all(|box_| {
        let left = box_.x;
        let right = left + box_.width - 1;
        let top = box_.y;
        let bottom = top + box_.height - 1;
        if from.1 == to.1 {
            from.1 < top || from.1 > bottom || from.0.max(to.0) < left || from.0.min(to.0) > right
        } else {
            from.0 < left || from.0 > right || from.1.max(to.1) < top || from.1.min(to.1) > bottom
        }
    })
}

fn route_row(components: &[Component], lane: i32, stem: i32, sy: i32, ty: i32) -> i32 {
    // Only a box boundary can change whether a horizontal row is clear.
    // Prefer the existing direct row on ties; otherwise use the shortest clear detour.
    let candidates = std::iter::once(ty).chain(std::iter::once(sy)).chain(
        components
            .iter()
            .flat_map(|box_| [box_.y - 1, box_.y + box_.height]),
    );
    candidates
        .filter(|&y| {
            y >= 0
                && clear_segment(components, (lane, sy), (lane, y))
                && clear_segment(components, (lane, y), (stem, y))
                && clear_segment(components, (stem, y), (stem, ty))
        })
        .min_by_key(|&y| (sy.abs_diff(y) + ty.abs_diff(y), y != ty))
        .unwrap_or(ty)
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
    let mut neighbors = vec![Vec::new(); groups.len()];
    for &(from, to) in &edges {
        neighbors[from].push(to);
        neighbors[to].push(from);
    }
    let mut clusters = vec![usize::MAX; groups.len()];
    let mut cluster_members = Vec::new();
    for start in 0..groups.len() {
        if clusters[start] != usize::MAX {
            continue;
        }
        let cluster = cluster_members.len();
        let mut pending = vec![start];
        clusters[start] = cluster;
        let mut members = Vec::new();
        while let Some(id) = pending.pop() {
            members.push(id);
            for &neighbor in &neighbors[id] {
                if clusters[neighbor] == usize::MAX {
                    clusters[neighbor] = cluster;
                    pending.push(neighbor);
                }
            }
        }
        cluster_members.push(members);
    }
    let mut levels = vec![None; groups.len()];
    for id in 0..groups.len() {
        level(id, &edges, &mut levels);
    }
    let max_levels: Vec<_> = cluster_members
        .iter()
        .map(|members| {
            members
                .iter()
                .map(|id| levels[*id].unwrap())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut components = Vec::new();
    for (id, members) in groups.into_iter().enumerate() {
        let column = max_levels[clusters[id]] - levels[id].unwrap_or(0);
        let cycle = members.len() > 1
            || impact
                .edges
                .iter()
                .any(|(caller, callee)| caller == callee && members.contains(caller));
        components.push(Component {
            members,
            cluster: clusters[id],
            column,
            x: 0,
            y: 0,
            width: MIN_BOX_WIDTH,
            height: 5,
            title: String::new(),
            description: String::new(),
            extra: Vec::new(),
            edge_labels: Vec::new(),
            cycle,
        });
    }
    let mut visual: Vec<_> = (0..components.len()).collect();
    visual.sort_by_key(|id| (components[*id].cluster, components[*id].column, *id));
    let order = visual
        .into_iter()
        .flat_map(|id| components[id].members.iter().cloned())
        .collect();
    let mut routes: Vec<_> = impact
        .edges
        .iter()
        .filter_map(|(caller, callee)| {
            let from = member_group[caller];
            let to = member_group[callee];
            (from != to).then(|| LayoutEdge {
                from,
                to,
                caller: caller.clone(),
                callee: callee.clone(),
                port: 0,
                status: None,
                lane_x: 0,
                route_y: 0,
            })
        })
        .collect();
    routes.sort_by(|a, b| (a.to, &a.caller, &a.callee).cmp(&(b.to, &b.caller, &b.callee)));
    let mut incoming = vec![0; components.len()];
    for route in &mut routes {
        route.port = incoming[route.to];
        incoming[route.to] += 1;
    }
    Layout {
        components,
        edges: routes,
        order,
        width: 0,
        height: 0,
    }
}

pub fn place(view: &mut Layout) {
    if view.components.is_empty() {
        return;
    }
    let clusters = view.components.iter().map(|c| c.cluster).max().unwrap() + 1;
    let mut widths = vec![Vec::new(); clusters];
    for component in &view.components {
        let columns = &mut widths[component.cluster];
        columns.resize(columns.len().max(component.column + 1), MIN_BOX_WIDTH);
    }
    for (id, component) in view.components.iter_mut().enumerate() {
        let incoming = view.edges.iter().filter(|edge| edge.to == id).count() as i32;
        component.width = (component.title.len() as i32 + 4)
            .max(component.description.len() as i32 + 4)
            .max(
                component
                    .extra
                    .iter()
                    .map(|line| line.len() as i32 + 2)
                    .max()
                    .unwrap_or(0),
            )
            .max(
                component
                    .edge_labels
                    .iter()
                    .map(|line| line.len() as i32 + 2)
                    .max()
                    .unwrap_or(0),
            )
            .max(MIN_BOX_WIDTH);
        component.height = (5
            + component.edge_labels.len() as i32
            + if component.extra.is_empty() {
                0
            } else {
                component.extra.len() as i32 + 1
            })
        .max(incoming + 2);
        widths[component.cluster][component.column] =
            widths[component.cluster][component.column].max(component.width);
    }
    let positions: Vec<Vec<i32>> = widths
        .into_iter()
        .map(|cluster_widths| {
            let mut x = 0;
            cluster_widths
                .into_iter()
                .map(|width| {
                    let at = x;
                    x += width + COLUMN_GAP;
                    at
                })
                .collect()
        })
        .collect();
    let mut rows: Vec<Vec<i32>> = positions
        .iter()
        .map(|columns| vec![0; columns.len()])
        .collect();
    for component in &mut view.components {
        component.x = positions[component.cluster][component.column];
        component.y = rows[component.cluster][component.column];
        rows[component.cluster][component.column] += component.height + ROW_GAP;
    }
    let mut origin_y = 0;
    let origins: Vec<_> = rows
        .into_iter()
        .map(|columns| {
            let at = origin_y;
            origin_y += columns.into_iter().max().unwrap_or(0);
            at
        })
        .collect();
    for component in &mut view.components {
        component.y += origins[component.cluster];
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
    for edge in &mut view.edges {
        let caller = &view.components[edge.from];
        let callee = &view.components[edge.to];
        let sy = caller.y + 1;
        let ty = callee.y + 1 + edge.port as i32;
        edge.lane_x = view
            .components
            .iter()
            .filter(|component| {
                component.cluster == caller.cluster && component.column == caller.column
            })
            .map(|component| component.x + component.width)
            .max()
            .unwrap()
            + 3;
        edge.route_y = route_row(&view.components, edge.lane_x, callee.x - 3, sy, ty);
        view.height = view.height.max(edge.route_y + 1);
    }
}
