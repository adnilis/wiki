//! Query the explicit wiki-link graph stored by wiki index.
//!
//! The graph intentionally stays conservative in this phase: a node is an
//! indexed Markdown page and an edge is an explicit wiki-link. The same module
//! also queries rule-extracted technical entities and their evidence-backed
//! co-occurrence relations. LLM extraction can be added later without changing
//! this query layer.

use rusqlite::Connection;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

const INDEX_DIR: &str = ".wiki";
const INDEX_DB: &str = "index.sqlite";
const MAX_TRAVERSAL_DEPTH: usize = 8;

#[derive(Clone, Debug)]
pub struct GraphOptions {
    pub path: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphDirection {
    Incoming,
    Outgoing,
    Both,
}

#[derive(Debug)]
pub enum GraphError {
    Invalid(String),
    Database(String),
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Database(message) => f.write_str(message),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GraphNode {
    pub id: String,
    pub path: String,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub link_text: Option<String>,
    pub resolved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GraphExport {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphStats {
    pub nodes: usize,
    pub links: usize,
    pub unique_edges: usize,
    pub resolved_links: usize,
    pub dangling_links: usize,
    pub entity_nodes: usize,
    pub entity_mentions: usize,
    pub entity_relations: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EntityRecord {
    pub id: String,
    pub canonical_name: String,
    pub entity_type: String,
    pub confidence: f64,
    pub status: String,
    pub mentions: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EntityEvidence {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub quote: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EntityNeighbor {
    pub direction: String,
    pub predicate: String,
    pub confidence: f64,
    pub entity: EntityRecord,
    pub evidence: Vec<EntityEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NeighborResult {
    pub root: GraphNode,
    pub depth: usize,
    pub direction: GraphDirection,
    pub edges: Vec<NeighborEdge>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NeighborEdge {
    pub depth: usize,
    pub direction: &'static str,
    pub source: String,
    pub target: String,
    pub link_text: Option<String>,
    pub resolved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathResult {
    pub from: GraphNode,
    pub to: GraphNode,
    pub steps: Vec<PathStep>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathStep {
    pub source: String,
    pub target: String,
    pub link_text: Option<String>,
}

#[derive(Clone, Debug)]
struct Graph {
    nodes: BTreeMap<String, GraphNode>,
    edges: Vec<GraphEdge>,
    outgoing: BTreeMap<String, Vec<usize>>,
    incoming: BTreeMap<String, Vec<usize>>,
}

pub fn stats(options: &GraphOptions) -> Result<GraphStats, GraphError> {
    let graph = load_graph(options)?;
    let connection = open_database(options)?;
    let unique_edges: BTreeSet<(&str, &str)> = graph
        .edges
        .iter()
        .map(|edge| (edge.source.as_str(), edge.target.as_str()))
        .collect();
    let resolved_links = graph.edges.iter().filter(|edge| edge.resolved).count();
    Ok(GraphStats {
        nodes: graph.nodes.len(),
        links: graph.edges.len(),
        unique_edges: unique_edges.len(),
        resolved_links,
        dangling_links: graph.edges.len().saturating_sub(resolved_links),
        entity_nodes: count_table(&connection, "entities")?,
        entity_mentions: count_table(&connection, "entity_mentions")?,
        entity_relations: count_table(&connection, "relations")?,
    })
}

pub fn entities(
    options: &GraphOptions,
    query: Option<&str>,
    entity_type: Option<&str>,
    limit: usize,
) -> Result<Vec<EntityRecord>, GraphError> {
    let connection = open_database(options)?;
    let pattern = query.map(|value| format!("%{}%", value.trim()));
    let limit = limit.clamp(1, 500) as i64;
    let mut statement = connection
        .prepare(
            "SELECT e.id, e.canonical_name, e.entity_type, e.confidence, e.status,
                    COUNT(m.id) AS mentions
             FROM entities e
             LEFT JOIN entity_mentions m ON m.entity_id = e.id
             WHERE (?1 IS NULL OR lower(e.canonical_name) LIKE lower(?1))
               AND (?2 IS NULL OR e.entity_type = ?2)
             GROUP BY e.id
             ORDER BY mentions DESC, e.entity_type, e.canonical_name
             LIMIT ?3",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(
            rusqlite::params![pattern, entity_type, limit],
            entity_from_row,
        )
        .map_err(database_error)?;
    rows.map(|row| row.map_err(database_error)).collect()
}

pub fn entity_neighbors(
    options: &GraphOptions,
    entity: &str,
    limit: usize,
) -> Result<(EntityRecord, Vec<EntityNeighbor>), GraphError> {
    let connection = open_database(options)?;
    let root = resolve_entity(&connection, entity)?;
    let limit = limit.clamp(1, 500) as i64;
    let mut statement = connection
        .prepare(
            "SELECT r.subject_entity_id, r.predicate, r.object_entity_id, r.confidence,
                    n.id, n.canonical_name, n.entity_type, n.confidence, n.status,
                    (SELECT COUNT(*) FROM entity_mentions nm WHERE nm.entity_id = n.id),
                    ev.path, ev.start_line, ev.end_line, ev.quote
             FROM relations r
             JOIN entities n ON n.id = CASE
                 WHEN r.subject_entity_id = ?1 THEN r.object_entity_id
                 ELSE r.subject_entity_id
             END
             LEFT JOIN (
                 SELECT re.relation_id, d.path, re.start_line, re.end_line, re.quote
                 FROM relation_evidence re
                 JOIN chunks c ON c.id = re.chunk_id
                 JOIN documents d ON d.id = c.document_id
             ) ev ON ev.relation_id = r.id
             WHERE r.subject_entity_id = ?1 OR r.object_entity_id = ?1
             ORDER BY n.entity_type, n.canonical_name, r.predicate, ev.path, ev.start_line",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(rusqlite::params![root.id], |row| {
            let subject_id: String = row.get(0)?;
            let predicate: String = row.get(1)?;
            let neighbor = EntityRecord {
                id: row.get(4)?,
                canonical_name: row.get(5)?,
                entity_type: row.get(6)?,
                confidence: row.get(7)?,
                status: row.get(8)?,
                mentions: row.get::<_, i64>(9)?.max(0) as usize,
            };
            let evidence = match (
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<i64>>(11)?,
                row.get::<_, Option<i64>>(12)?,
                row.get::<_, Option<String>>(13)?,
            ) {
                (Some(path), Some(start_line), Some(end_line), Some(quote)) => {
                    vec![EntityEvidence {
                        path,
                        start_line: start_line.max(0) as usize,
                        end_line: end_line.max(0) as usize,
                        quote,
                    }]
                }
                _ => Vec::new(),
            };
            Ok((
                EntityNeighbor {
                    direction: if subject_id == root.id {
                        "outgoing".to_string()
                    } else {
                        "incoming".to_string()
                    },
                    predicate,
                    confidence: row.get(3)?,
                    entity: neighbor,
                    evidence,
                },
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(database_error)?;

    let mut grouped: BTreeMap<(String, String, String), EntityNeighbor> = BTreeMap::new();
    for row in rows {
        let (mut neighbor, other_id) = row.map_err(database_error)?;
        let key = (
            other_id,
            neighbor.direction.clone(),
            neighbor.predicate.clone(),
        );
        if let Some(existing) = grouped.get_mut(&key) {
            existing.confidence = existing.confidence.max(neighbor.confidence);
            existing.evidence.append(&mut neighbor.evidence);
        } else {
            grouped.insert(key, neighbor);
        }
    }
    let mut result: Vec<EntityNeighbor> = grouped.into_values().collect();
    for neighbor in &mut result {
        neighbor.evidence.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.start_line.cmp(&right.start_line))
        });
        neighbor.evidence.dedup();
    }
    result.truncate(limit as usize);
    Ok((root, result))
}

pub fn format_entities(entities: &[EntityRecord]) -> String {
    let mut output = format!(
        "== Graph entities ==\nFound {} entity(s).\n",
        entities.len()
    );
    if entities.is_empty() {
        output.push_str("(No entities found.)\n");
        return output;
    }
    output.push('\n');
    for entity in entities {
        output.push_str(&format!(
            "- [{}] {} (mentions: {}, confidence: {:.2}, status: {})\n",
            entity.entity_type,
            entity.canonical_name,
            entity.mentions,
            entity.confidence,
            entity.status
        ));
    }
    output
}

pub fn format_entity_neighbors(root: &EntityRecord, neighbors: &[EntityNeighbor]) -> String {
    let mut output = format!(
        "== Entity neighbors: {} [{}] ==\nFound {} neighbor(s).\n",
        root.canonical_name,
        root.entity_type,
        neighbors.len()
    );
    if neighbors.is_empty() {
        output.push_str("(No entity neighbors found.)\n");
        return output;
    }
    output.push('\n');
    for neighbor in neighbors {
        output.push_str(&format!(
            "- [{}] {} --{}--> [{}] {} (confidence: {:.2})\n",
            neighbor.direction,
            root.canonical_name,
            neighbor.predicate,
            neighbor.entity.entity_type,
            neighbor.entity.canonical_name,
            neighbor.confidence
        ));
        for evidence in &neighbor.evidence {
            output.push_str(&format!(
                "  evidence: {}:{}-{} {}\n",
                evidence.path, evidence.start_line, evidence.end_line, evidence.quote
            ));
        }
    }
    output
}

fn entity_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntityRecord> {
    Ok(EntityRecord {
        id: row.get(0)?,
        canonical_name: row.get(1)?,
        entity_type: row.get(2)?,
        confidence: row.get(3)?,
        status: row.get(4)?,
        mentions: row.get::<_, i64>(5)?.max(0) as usize,
    })
}

fn resolve_entity(connection: &Connection, query: &str) -> Result<EntityRecord, GraphError> {
    let mut statement = connection
        .prepare(
            "SELECT e.id, e.canonical_name, e.entity_type, e.confidence, e.status,
                    (SELECT COUNT(*) FROM entity_mentions m WHERE m.entity_id = e.id)
             FROM entities e
             WHERE e.id = ?1 OR lower(e.canonical_name) = lower(?1)
             ORDER BY e.entity_type, e.canonical_name",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(rusqlite::params![query.trim()], entity_from_row)
        .map_err(database_error)?;
    let matches: Vec<EntityRecord> = rows
        .map(|row| row.map_err(database_error))
        .collect::<Result<_, _>>()?;
    match matches.as_slice() {
        [entity] => Ok(entity.clone()),
        [] => Err(GraphError::Invalid(format!(
            "graph entity not found: {}",
            query.trim()
        ))),
        _ => Err(GraphError::Invalid(format!(
            "graph entity is ambiguous: {}; use a more specific canonical name or entity id",
            query.trim()
        ))),
    }
}

fn open_database(options: &GraphOptions) -> Result<Connection, GraphError> {
    let database_path = database_path(options.path.as_deref())?;
    if !database_path.is_file() {
        return Err(GraphError::Invalid(format!(
            "wiki graph index does not exist: {}. Run wiki index first.",
            database_path.display()
        )));
    }
    Connection::open(&database_path).map_err(database_error)
}

fn count_table(connection: &Connection, table: &str) -> Result<usize, GraphError> {
    let sql = match table {
        "entities" | "entity_mentions" | "relations" => {
            format!("SELECT COUNT(*) FROM {table}")
        }
        _ => {
            return Err(GraphError::Database(format!(
                "unsupported graph table: {table}"
            )))
        }
    };
    let count: i64 = connection
        .query_row(&sql, [], |row| row.get(0))
        .map_err(database_error)?;
    Ok(count.max(0) as usize)
}

pub fn neighbors(
    options: &GraphOptions,
    entity: &str,
    depth: usize,
    direction: GraphDirection,
) -> Result<NeighborResult, GraphError> {
    let graph = load_graph(options)?;
    let root = resolve_node(&graph, entity)?;
    let max_depth = depth.clamp(1, MAX_TRAVERSAL_DEPTH);
    let mut queue = VecDeque::from([(root.id.clone(), 0usize)]);
    let mut seen_depth = BTreeMap::from([(root.id.clone(), 0usize)]);
    let mut result_edges = Vec::new();

    while let Some((current, current_depth)) = queue.pop_front() {
        if current_depth >= max_depth {
            continue;
        }
        let edge_ids = edge_ids_for_direction(&graph, &current, direction);
        for edge_id in edge_ids {
            let edge = &graph.edges[edge_id];
            let (neighbor, edge_direction) = if edge.source == current {
                (&edge.target, "outgoing")
            } else {
                (&edge.source, "incoming")
            };
            let next_depth = current_depth + 1;
            result_edges.push(NeighborEdge {
                depth: next_depth,
                direction: edge_direction,
                source: edge.source.clone(),
                target: edge.target.clone(),
                link_text: edge.link_text.clone(),
                resolved: edge.resolved,
            });
            if edge.resolved && !seen_depth.contains_key(neighbor) {
                seen_depth.insert(neighbor.clone(), next_depth);
                queue.push_back((neighbor.clone(), next_depth));
            }
        }
    }

    result_edges.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then_with(|| left.direction.cmp(right.direction))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.target.cmp(&right.target))
            .then_with(|| left.link_text.cmp(&right.link_text))
    });
    result_edges.dedup();

    Ok(NeighborResult {
        root,
        depth: max_depth,
        direction,
        edges: result_edges,
    })
}

pub fn path(
    options: &GraphOptions,
    from: &str,
    to: &str,
    max_depth: usize,
) -> Result<PathResult, GraphError> {
    let graph = load_graph(options)?;
    let from_node = resolve_node(&graph, from)?;
    let to_node = resolve_node(&graph, to)?;
    let max_depth = max_depth.min(MAX_TRAVERSAL_DEPTH);
    let mut queue = VecDeque::from([(from_node.id.clone(), 0usize)]);
    let mut distance = BTreeMap::from([(from_node.id.clone(), 0usize)]);
    let mut previous: BTreeMap<String, usize> = BTreeMap::new();

    while let Some((current, current_depth)) = queue.pop_front() {
        if current == to_node.id || current_depth >= max_depth {
            continue;
        }
        let mut edge_ids = graph.outgoing.get(&current).cloned().unwrap_or_default();
        edge_ids.sort_by_key(|edge_id| {
            let edge = &graph.edges[*edge_id];
            (edge.target.clone(), edge.link_text.clone())
        });
        for edge_id in edge_ids {
            let edge = &graph.edges[edge_id];
            if !edge.resolved || distance.contains_key(&edge.target) {
                continue;
            }
            distance.insert(edge.target.clone(), current_depth + 1);
            previous.insert(edge.target.clone(), edge_id);
            queue.push_back((edge.target.clone(), current_depth + 1));
        }
    }

    let mut steps = Vec::new();
    if from_node.id != to_node.id && previous.contains_key(&to_node.id) {
        let mut current = to_node.id.clone();
        while current != from_node.id {
            let edge_id = previous[&current];
            let edge = &graph.edges[edge_id];
            steps.push(PathStep {
                source: edge.source.clone(),
                target: edge.target.clone(),
                link_text: edge.link_text.clone(),
            });
            current = edge.source.clone();
        }
        steps.reverse();
    }

    Ok(PathResult {
        from: from_node,
        to: to_node,
        steps,
    })
}

pub fn export(options: &GraphOptions) -> Result<GraphExport, GraphError> {
    let graph = load_graph(options)?;
    Ok(GraphExport {
        nodes: graph.nodes.into_values().collect(),
        edges: graph.edges,
    })
}

pub fn format_stats(stats: &GraphStats) -> String {
    format!(
        "== Graph stats ==\nNodes: {}\nLinks: {}\nUnique edges: {}\nResolved links: {}\nDangling links: {}\nEntity nodes: {}\nEntity mentions: {}\nEntity relations: {}\n",
        stats.nodes,
        stats.links,
        stats.unique_edges,
        stats.resolved_links,
        stats.dangling_links,
        stats.entity_nodes,
        stats.entity_mentions,
        stats.entity_relations
    )
}

pub fn format_neighbors(result: &NeighborResult) -> String {
    let direction = match result.direction {
        GraphDirection::Incoming => "incoming",
        GraphDirection::Outgoing => "outgoing",
        GraphDirection::Both => "both",
    };
    let mut output = format!(
        "== Graph neighbors: {} ==\nDepth: {}\nDirection: {}\n\n",
        result.root.id, result.depth, direction
    );
    if result.edges.is_empty() {
        output.push_str("(No neighbors found.)\n");
        return output;
    }
    for edge in &result.edges {
        let label = edge
            .link_text
            .as_deref()
            .map(|text| format!(" [{text}]"))
            .unwrap_or_default();
        let status = if edge.resolved { "" } else { " [dangling]" };
        output.push_str(&format!(
            "- depth {} [{}] {} --wiki{}--> {}{}\n",
            edge.depth, edge.direction, edge.source, label, edge.target, status
        ));
    }
    output.push_str(&format!("\n(Complete: {} edge(s).)\n", result.edges.len()));
    output
}

pub fn format_path(result: &PathResult) -> String {
    let mut output = format!("== Graph path: {} -> {} ==\n", result.from.id, result.to.id);
    if result.from.id == result.to.id {
        output.push_str("Already at the target node (0 step(s)).\n");
        return output;
    }
    if result.steps.is_empty() {
        output.push_str("(No directed path found.)\n");
        return output;
    }
    output.push_str(&format!("Steps: {}\n", result.steps.len()));
    for (index, step) in result.steps.iter().enumerate() {
        let label = step
            .link_text
            .as_deref()
            .map(|text| format!(" [{text}]"))
            .unwrap_or_default();
        output.push_str(&format!(
            "{}. {} --wiki{}--> {}\n",
            index + 1,
            step.source,
            label,
            step.target
        ));
    }
    output
}

pub fn format_dot(export: &GraphExport) -> String {
    let mut output = String::from("digraph wiki {\n  rankdir=LR;\n");
    for node in &export.nodes {
        output.push_str(&format!(
            "  \"{}\" [label=\"{}\\n{}\"];\n",
            escape_dot(&node.id),
            escape_dot(&node.title),
            escape_dot(&node.path)
        ));
    }
    let mut dangling = BTreeSet::new();
    for edge in &export.edges {
        if !edge.resolved {
            dangling.insert(edge.target.clone());
        }
    }
    for target in dangling {
        output.push_str(&format!(
            "  \"{}\" [label=\"{}\", style=dashed];\n",
            escape_dot(&target),
            escape_dot(&target)
        ));
    }
    for edge in &export.edges {
        let label = edge.link_text.as_deref().unwrap_or("wiki");
        output.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
            escape_dot(&edge.source),
            escape_dot(&edge.target),
            escape_dot(label)
        ));
    }
    output.push_str("}\n");
    output
}

fn load_graph(options: &GraphOptions) -> Result<Graph, GraphError> {
    let connection = open_database(options)?;
    let mut nodes = BTreeMap::new();
    {
        let mut statement = connection
            .prepare("SELECT path, title FROM documents ORDER BY path")
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(database_error)?;
        for row in rows {
            let (path, title) = row.map_err(database_error)?;
            let id = canonical_path(&path);
            nodes.insert(id.clone(), GraphNode { id, path, title });
        }
    }

    let mut edges = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT d.path, w.target_path, w.link_text
                 FROM wiki_links w
                 JOIN documents d ON d.id = w.source_document_id
                 ORDER BY d.path, w.target_path, w.link_text",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(database_error)?;
        for row in rows {
            let (source_path, target_path, link_text) = row.map_err(database_error)?;
            let source = canonical_path(&source_path);
            let target = canonical_path(&target_path);
            edges.push(GraphEdge {
                source,
                resolved: nodes.contains_key(&target),
                target,
                link_text,
            });
        }
    }

    let mut outgoing: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut incoming: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (edge_id, edge) in edges.iter().enumerate() {
        outgoing
            .entry(edge.source.clone())
            .or_default()
            .push(edge_id);
        if edge.resolved {
            incoming
                .entry(edge.target.clone())
                .or_default()
                .push(edge_id);
        }
    }
    Ok(Graph {
        nodes,
        edges,
        outgoing,
        incoming,
    })
}

fn database_path(input: Option<&str>) -> Result<PathBuf, GraphError> {
    let raw = input.unwrap_or("docs");
    if raw.trim().is_empty() {
        return Err(GraphError::Invalid(
            "wiki graph path must be a non-empty directory path.".to_string(),
        ));
    }
    let root = PathBuf::from(raw);
    if !root.exists() {
        return Err(GraphError::Invalid(format!(
            "wiki graph path does not exist: {}",
            root.display()
        )));
    }
    if !root.is_dir() {
        return Err(GraphError::Invalid(format!(
            "wiki graph path is not a directory: {}",
            root.display()
        )));
    }
    Ok(root.join(INDEX_DIR).join(INDEX_DB))
}

fn resolve_node(graph: &Graph, query: &str) -> Result<GraphNode, GraphError> {
    let canonical = canonical_path(query);
    if let Some(node) = graph.nodes.get(&canonical) {
        return Ok(node.clone());
    }
    let folded = query.trim().to_lowercase();
    let matches: Vec<&GraphNode> = graph
        .nodes
        .values()
        .filter(|node| node.title.to_lowercase() == folded)
        .collect();
    match matches.as_slice() {
        [node] => Ok((*node).clone()),
        [] => Err(GraphError::Invalid(format!(
            "graph entity not found: {query}"
        ))),
        _ => Err(GraphError::Invalid(format!(
            "graph entity title is ambiguous: {query}; candidates: {}",
            matches
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn edge_ids_for_direction(graph: &Graph, current: &str, direction: GraphDirection) -> Vec<usize> {
    let mut edge_ids = Vec::new();
    if matches!(direction, GraphDirection::Outgoing | GraphDirection::Both) {
        edge_ids.extend(graph.outgoing.get(current).into_iter().flatten().copied());
    }
    if matches!(direction, GraphDirection::Incoming | GraphDirection::Both) {
        edge_ids.extend(graph.incoming.get(current).into_iter().flatten().copied());
    }
    edge_ids.sort_unstable();
    edge_ids.dedup();
    edge_ids
}

fn canonical_path(value: &str) -> String {
    let mut target = value.trim().replace('\\', "/");
    target = target.split('#').next().unwrap_or_default().to_string();
    while let Some(stripped) = target.strip_prefix("./") {
        target = stripped.to_string();
    }
    target.trim_end_matches(".md").to_string()
}

fn escape_dot(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn database_error(error: rusqlite::Error) -> GraphError {
    GraphError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{index, IndexOptions, DEFAULT_CHUNK_CHARS};
    use std::fs;
    use std::path::Path;

    fn setup() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(
            root.join("notes").join("a.md"),
            "# A\n\nSee [[notes/b|B]].\n",
        )
        .unwrap();
        fs::write(root.join("notes").join("b.md"), "# B\n\nSee [[notes/c]].\n").unwrap();
        fs::write(
            root.join("notes").join("c.md"),
            "# C\n\nSee [[missing/page]].\n",
        )
        .unwrap();
        index(&IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        })
        .unwrap();
        directory
    }

    fn options(root: &Path) -> GraphOptions {
        GraphOptions {
            path: Some(root.to_string_lossy().into_owned()),
        }
    }

    fn entity_setup() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("notes")).unwrap();
        let marker = '\x60';
        let text = format!(
            "# Service\n\n{marker}RoleService{marker} uses {marker}EquipBagSize{marker} and {marker}server.port{marker}.\n"
        );
        fs::write(root.join("notes").join("service.md"), text).unwrap();
        index(&IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        })
        .unwrap();
        directory
    }

    #[test]
    fn stats_count_resolved_and_dangling_links() {
        let directory = setup();
        let stats = stats(&options(directory.path())).unwrap();
        assert_eq!(stats.nodes, 3);
        assert_eq!(stats.links, 3);
        assert_eq!(stats.unique_edges, 3);
        assert_eq!(stats.resolved_links, 2);
        assert_eq!(stats.dangling_links, 1);
    }

    #[test]
    fn neighbors_traverse_both_directions() {
        let directory = setup();
        let result = neighbors(
            &options(directory.path()),
            "notes/b",
            1,
            GraphDirection::Both,
        )
        .unwrap();
        assert_eq!(result.root.title, "B");
        assert_eq!(result.edges.len(), 2);
        assert!(result
            .edges
            .iter()
            .any(|edge| edge.direction == "incoming" && edge.source == "notes/a"));
        assert!(result
            .edges
            .iter()
            .any(|edge| edge.direction == "outgoing" && edge.target == "notes/c"));
    }

    #[test]
    fn path_follows_resolved_outgoing_links() {
        let directory = setup();
        let result = path(&options(directory.path()), "A", "notes/c", 4).unwrap();
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[0].source, "notes/a");
        assert_eq!(result.steps[1].target, "notes/c");
    }

    #[test]
    fn path_returns_no_steps_when_target_is_unreachable() {
        let directory = setup();
        let result = path(&options(directory.path()), "notes/c", "notes/a", 4).unwrap();
        assert!(result.steps.is_empty());
    }

    #[test]
    fn export_keeps_dangling_edges_and_serializes() {
        let directory = setup();
        let graph = export(&options(directory.path())).unwrap();
        assert_eq!(graph.nodes.len(), 3);
        assert!(graph.edges.iter().any(|edge| !edge.resolved));
        let json = serde_json::to_string(&graph).unwrap();
        assert!(json.contains("missing/page"));
        assert!(format_dot(&graph).contains("digraph wiki"));
    }

    #[test]
    fn entities_lists_rule_extracted_names_and_types() {
        let directory = entity_setup();
        let result = entities(&options(directory.path()), Some("Role"), None, 10).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].canonical_name, "RoleService");
        assert_eq!(result[0].entity_type, "identifier");
        assert_eq!(result[0].mentions, 1);
    }

    #[test]
    fn entity_neighbors_return_cooccurrence_evidence() {
        let directory = entity_setup();
        let (root, neighbors) =
            entity_neighbors(&options(directory.path()), "RoleService", 10).unwrap();
        assert_eq!(root.canonical_name, "RoleService");
        assert!(neighbors
            .iter()
            .any(|neighbor| neighbor.entity.canonical_name == "EquipBagSize"));
        let evidence = neighbors
            .iter()
            .flat_map(|neighbor| neighbor.evidence.iter())
            .next()
            .expect("co-occurrence should retain evidence");
        assert_eq!(evidence.path, "notes/service.md");
        assert_eq!(evidence.start_line, 1);
    }

    #[test]
    fn entity_neighbors_applies_limit_after_grouping_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(
            root.join("service.md"),
            "# First\n\n`RootEntity` uses `AlphaEntity`.\n\n# Second\n\n`RootEntity` uses `AlphaEntity`.\n\n# Third\n\n`RootEntity` uses `BetaEntity`.\n",
        )
        .unwrap();
        index(&IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        })
        .unwrap();

        let (_, neighbors) = entity_neighbors(&options(root), "RootEntity", 2).unwrap();
        assert_eq!(neighbors.len(), 2);
        assert!(neighbors
            .iter()
            .any(|neighbor| neighbor.entity.canonical_name == "AlphaEntity"));
        assert!(neighbors
            .iter()
            .any(|neighbor| neighbor.entity.canonical_name == "BetaEntity"));
        let alpha = neighbors
            .iter()
            .find(|neighbor| neighbor.entity.canonical_name == "AlphaEntity")
            .unwrap();
        assert_eq!(alpha.evidence.len(), 2);
    }
}
