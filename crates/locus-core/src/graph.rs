//! Graph data model and read-only graph queries (U-016).
//!
//! Nodes are memories, edges are shared entities. Relationships come from
//! SQLite joins (`memory_entities` self-joins) — there is no graph database.
//! All graph queries run on their own read-only connections, never through the
//! single-writer queue, and are bounded (max nodes) so payloads stay small.

use rusqlite::{params, params_from_iter, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::memory::Memory;
use crate::store::Store;
use crate::Result;

/// Default cap on the number of nodes in a graph.
pub const DEFAULT_GRAPH_MAX_NODES: usize = 300;

/// A node in the memory graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub content: String,
    pub namespace: String,
    pub memory_type: String,
    pub importance: u8,
    pub access_count: u64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accessed_at: Option<i64>,
    pub entities: Vec<String>,
}

impl From<&Memory> for GraphNode {
    fn from(memory: &Memory) -> Self {
        Self {
            id: memory.id.clone(),
            title: memory.title.clone(),
            content: memory.content.clone(),
            namespace: memory.namespace.clone(),
            memory_type: memory.memory_type.as_str().to_string(),
            importance: memory.importance,
            access_count: memory.access_count,
            created_at: memory.created_at,
            updated_at: memory.updated_at,
            last_accessed_at: memory.last_accessed_at,
            entities: memory.entities.clone(),
        }
    }
}

/// An undirected edge between two memories sharing an entity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    /// The shared entity that produces this edge.
    pub label: String,
    /// Number of shared entities (1 for a single-entity edge).
    pub weight: u32,
}

/// The full graph payload: nodes, edges, and render-friendly stats.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub truncated: bool,
}

impl GraphData {
    fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            truncated: false,
        }
    }
}

/// Parameters for a full-graph query.
#[derive(Debug, Clone, Default)]
pub struct GraphRequest {
    pub namespace: Option<String>,
    /// Focus one memory plus its immediate neighbors (shared entities).
    pub expand: Option<String>,
    pub max_nodes: usize,
}

impl Store {
    /// Builds the memory graph, optionally scoped to a namespace and optionally
    /// focused on one memory (`--expand <id>`, depth 1).
    ///
    /// Runs on its own read-only connection (WAL snapshot isolation), never on
    /// the shared search connection and never through the single-writer queue.
    pub fn graph(&self, request: GraphRequest) -> Result<GraphData> {
        let max_nodes = if request.max_nodes == 0 {
            DEFAULT_GRAPH_MAX_NODES
        } else {
            request.max_nodes
        };

        let conn = self.connect_ro()?;

        let node_ids = match &request.expand {
            Some(focus) => {
                let focus_id = focus.trim();
                if focus_id.is_empty() {
                    return Err(crate::Error::InvalidInput(
                        "expand id must not be empty".to_string(),
                    ));
                }
                self.expand_node_ids(&conn, focus_id, request.namespace.as_deref(), max_nodes)?
            }
            None => self.graph_node_ids(&conn, request.namespace.as_deref(), max_nodes)?,
        };

        if node_ids.is_empty() {
            return Ok(GraphData::empty());
        }

        let nodes = self.load_graph_nodes(&conn, &node_ids)?;
        let edges = self.graph_edges(&conn, &node_ids)?;
        let truncated = nodes.len() >= max_nodes;

        Ok(GraphData {
            nodes,
            edges,
            truncated,
        })
    }

    /// Node ids for a full graph: memories in the namespace (or all), most
    /// recently updated first, capped at `max_nodes`.
    fn graph_node_ids(
        &self,
        conn: &rusqlite::Connection,
        namespace: Option<&str>,
        max_nodes: usize,
    ) -> Result<Vec<String>> {
        let sql = match namespace {
            Some(_) => format!(
                "SELECT id FROM memories WHERE namespace = ? ORDER BY updated_at DESC LIMIT {max_nodes}"
            ),
            None => format!("SELECT id FROM memories ORDER BY updated_at DESC LIMIT {max_nodes}"),
        };

        let mut ids = Vec::new();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = match namespace {
            Some(ns) => stmt.query(params![ns])?,
            None => stmt.query([])?,
        };
        while let Some(row) = rows.next()? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }

    /// Node ids for an `--expand <id>` query: the focus memory plus every
    /// memory sharing at least one entity with it (depth 1), capped.
    fn expand_node_ids(
        &self,
        conn: &rusqlite::Connection,
        focus_id: &str,
        namespace: Option<&str>,
        max_nodes: usize,
    ) -> Result<Vec<String>> {
        // Confirm the focus memory exists.
        let focus_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM memories WHERE id = ?)",
            params![focus_id],
            |row| row.get(0),
        )?;
        if !focus_exists {
            return Err(crate::Error::NotFound(format!(
                "memory not found: {focus_id}"
            )));
        }

        let mut ids = vec![focus_id.to_string()];
        let sql = format!(
            "SELECT DISTINCT me2.memory_id
             FROM memory_entities me1
             INNER JOIN memory_entities me2 ON me1.entity_id = me2.entity_id
             WHERE me1.memory_id = ? AND me2.memory_id != ?
             {} 
             ORDER BY me2.memory_id ASC
             LIMIT {max_nodes}",
            match namespace {
                Some(_) => "AND EXISTS (SELECT 1 FROM memories m WHERE m.id = me2.memory_id AND m.namespace = ?)",
                None => "",
            }
        );

        let mut stmt = conn.prepare(&sql)?;
        let mut rows = match namespace {
            Some(ns) => stmt.query(params![focus_id, focus_id, ns])?,
            None => stmt.query(params![focus_id, focus_id])?,
        };
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            if ids.len() < max_nodes {
                ids.push(id);
            }
        }

        Ok(ids)
    }

    /// Loads full graph nodes (with entities) for a set of node ids, preserving
    /// the given order.
    fn load_graph_nodes(
        &self,
        conn: &rusqlite::Connection,
        node_ids: &[String],
    ) -> Result<Vec<GraphNode>> {
        let mut by_id: Vec<(String, GraphNode)> = Vec::with_capacity(node_ids.len());
        for id in node_ids {
            let row = conn
                .query_row(
                    "
                    SELECT id, namespace, type, title, content, importance,
                           created_at, updated_at, access_count, last_accessed_at
                    FROM memories
                    WHERE id = ?
                    ",
                    params![id],
                    |row| {
                        let memory_type = row.get::<_, String>(2)?;
                        let importance_i64: i64 = row.get(5)?;
                        let importance = u8::try_from(importance_i64)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?;
                        Ok(GraphNode {
                            id: row.get(0)?,
                            namespace: row.get(1)?,
                            memory_type,
                            title: row.get(3)?,
                            content: row.get(4)?,
                            importance,
                            created_at: row.get(6)?,
                            updated_at: row.get(7)?,
                            access_count: row.get(8)?,
                            last_accessed_at: row.get(9)?,
                            entities: Vec::new(),
                        })
                    },
                )
                .optional()?;

            if let Some(mut node) = row {
                node.entities = crate::store::entities_of(conn, id)?;
                by_id.push((node.id.clone(), node));
            }
        }

        Ok(by_id.into_iter().map(|(_, node)| node).collect())
    }

    /// Edges between the given node ids: one edge per shared entity pair,
    /// deduplicated and ordered deterministically.
    fn graph_edges(
        &self,
        conn: &rusqlite::Connection,
        node_ids: &[String],
    ) -> Result<Vec<GraphEdge>> {
        if node_ids.len() < 2 {
            return Ok(Vec::new());
        }

        // Bound the placeholder count: node ids are capped at max_nodes.
        let placeholders = vec!["?"; node_ids.len()].join(",");
        let sql = format!(
            "
            SELECT me1.memory_id AS a, me2.memory_id AS b, e.name
            FROM memory_entities me1
            INNER JOIN memory_entities me2
                ON me1.entity_id = me2.entity_id AND me1.memory_id < me2.memory_id
            INNER JOIN entities e ON e.id = me1.entity_id
            INNER JOIN memories ma ON ma.id = me1.memory_id
            INNER JOIN memories mb ON mb.id = me2.memory_id
            WHERE me1.memory_id IN ({placeholders}) AND me2.memory_id IN ({placeholders})
              AND ma.namespace = mb.namespace
            ORDER BY a ASC, b ASC, e.name ASC
            "
        );

        let mut ids: Vec<&str> = node_ids.iter().map(String::as_str).collect();
        let params = {
            let mut combined: Vec<&str> = ids.clone();
            combined.append(&mut ids);
            combined
        };

        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;

        let mut seen = std::collections::HashSet::new();
        let mut edges = Vec::new();
        while let Some(row) = rows.next()? {
            let source: String = row.get(0)?;
            let target: String = row.get(1)?;
            let label: String = row.get(2)?;
            let key = (source.clone(), target.clone(), label.clone());
            if !seen.insert(key) {
                continue;
            }
            edges.push(GraphEdge {
                source,
                target,
                label,
                weight: 1,
            });
        }
        Ok(edges)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::memory::{MemoryType, NewMemory};

    fn test_store() -> (Store, TempDir) {
        let tmp = TempDir::new().expect("temp dir");
        let store = Store::open_at(tmp.path().join("locus.db")).expect("store");
        (store, tmp)
    }

    fn memory(namespace: &str, title: &str, entities: &[&str]) -> NewMemory {
        NewMemory {
            namespace: Some(namespace.to_string()),
            memory_type: MemoryType::Fact,
            title: title.to_string(),
            content: format!("{title} content"),
            entities: entities.iter().map(|s| s.to_string()).collect(),
            importance: 50,
            source: None,
        }
    }

    #[test]
    fn graph_node_set_matches_namespace_filter() {
        let (store, _tmp) = test_store();
        store
            .insert_memory(memory("project:auth", "Use Postgres", &["postgres"]))
            .unwrap();
        store
            .insert_memory(memory("project:payments", "Use Stripe", &["stripe"]))
            .unwrap();

        let all = store.graph(GraphRequest::default()).unwrap();
        assert_eq!(all.nodes.len(), 2);

        let auth = store
            .graph(GraphRequest {
                namespace: Some("project:auth".to_string()),
                ..GraphRequest::default()
            })
            .unwrap();
        assert_eq!(auth.nodes.len(), 1);
        assert_eq!(auth.nodes[0].title, "Use Postgres");
        assert!(auth.edges.is_empty());
    }

    #[test]
    fn graph_edge_set_reflects_shared_entities() {
        let (store, _tmp) = test_store();
        let id_a = store
            .insert_memory(memory(
                "project:auth",
                "Postgres primary",
                &["postgres", "auth"],
            ))
            .unwrap();
        let id_b = store
            .insert_memory(memory("project:auth", "Postgres backup", &["postgres"]))
            .unwrap();
        // No shared entity with this one.
        store
            .insert_memory(memory("project:auth", "Unrelated", &["payments"]))
            .unwrap();

        let graph = store.graph(GraphRequest::default()).unwrap();
        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].label, "postgres");
        assert_eq!(graph.edges[0].weight, 1);
        let pair = (
            graph.edges[0].source.as_str(),
            graph.edges[0].target.as_str(),
        );
        assert!(pair == (id_a.as_str(), id_b.as_str()) || pair == (id_b.as_str(), id_a.as_str()));
    }

    #[test]
    fn graph_edges_between_pairs_sharing_multiple_entities() {
        let (store, _tmp) = test_store();
        store
            .insert_memory(memory("project:auth", "A", &["postgres", "redis", "auth"]))
            .unwrap();
        store
            .insert_memory(memory("project:auth", "B", &["postgres", "redis"]))
            .unwrap();

        let graph = store.graph(GraphRequest::default()).unwrap();
        // Two shared entities -> two edges between the same pair.
        assert_eq!(graph.edges.len(), 2);
        for edge in &graph.edges {
            assert_eq!(edge.weight, 1);
        }
    }

    #[test]
    fn graph_edges_never_cross_namespaces() {
        let (store, _tmp) = test_store();
        store
            .insert_memory(memory("project:auth", "Postgres primary", &["postgres"]))
            .unwrap();
        store
            .insert_memory(memory("project:billing", "Postgres billing", &["postgres"]))
            .unwrap();

        let graph = store.graph(GraphRequest::default()).unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert!(
            graph.edges.is_empty(),
            "nodes sharing an entity across namespaces must not be linked"
        );

        let scoped = store
            .graph(GraphRequest {
                namespace: Some("project:auth".to_string()),
                ..GraphRequest::default()
            })
            .unwrap();
        assert_eq!(scoped.nodes.len(), 1);
        assert!(scoped.edges.is_empty());
    }

    #[test]
    fn graph_expand_shows_focus_and_immediate_links_only() {
        let (store, _tmp) = test_store();
        store
            .insert_memory(memory("project:auth", "Focus", &["postgres", "auth"]))
            .unwrap();
        store
            .insert_memory(memory("project:auth", "Linked", &["postgres"]))
            .unwrap();
        store
            .insert_memory(memory("project:auth", "Isolated", &["stripe"]))
            .unwrap();

        let focus_id = store
            .graph(GraphRequest::default())
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.title == "Focus")
            .unwrap()
            .id
            .clone();

        let graph = store
            .graph(GraphRequest {
                expand: Some(focus_id.clone()),
                ..GraphRequest::default()
            })
            .unwrap();
        let titles: Vec<&str> = graph.nodes.iter().map(|n| n.title.as_str()).collect();
        assert!(titles.contains(&"Focus"));
        assert!(titles.contains(&"Linked"));
        assert!(!titles.contains(&"Isolated"));
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].label, "postgres");
    }

    #[test]
    fn graph_expand_unknown_id_errors() {
        let (store, _tmp) = test_store();
        let err = store
            .graph(GraphRequest {
                expand: Some("nope".to_string()),
                ..GraphRequest::default()
            })
            .expect_err("unknown id should error");
        assert!(matches!(err, crate::Error::NotFound(_)));
    }

    #[test]
    fn graph_payload_is_capped() {
        let (store, _tmp) = test_store();
        for i in 0..10 {
            let entity = format!("entity-{i}");
            store
                .insert_memory(memory(
                    "project:auth",
                    &format!("Memory {i}"),
                    &[entity.as_str()],
                ))
                .unwrap();
        }

        let graph = store
            .graph(GraphRequest {
                max_nodes: 4,
                ..GraphRequest::default()
            })
            .unwrap();
        assert_eq!(graph.nodes.len(), 4);
        assert!(graph.truncated);
    }

    #[test]
    fn graph_node_includes_access_stats() {
        let (store, _tmp) = test_store();
        let id = store
            .insert_memory(memory("project:auth", "Visited", &["postgres"]))
            .unwrap();
        store.record_access(std::slice::from_ref(&id)).unwrap();

        let graph = store.graph(GraphRequest::default()).unwrap();
        let node = graph.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(node.access_count, 1);
        assert!(node.last_accessed_at.is_some());
    }

    #[test]
    fn graph_query_does_not_deadlock_concurrent_insert() {
        let (store, _tmp) = test_store();
        store
            .insert_memory(memory("project:auth", "seed", &["postgres"]))
            .unwrap();

        let writer_store = store.clone();
        let writer = std::thread::spawn(move || {
            for i in 0..50 {
                writer_store
                    .insert_memory(memory(
                        "project:auth",
                        &format!("writer-{i}"),
                        &["postgres"],
                    ))
                    .unwrap();
            }
        });

        let reader_store = store.clone();
        let reader = std::thread::spawn(move || {
            for _ in 0..50 {
                let graph = reader_store.graph(GraphRequest::default()).unwrap();
                assert!(!graph.nodes.is_empty());
            }
        });

        writer.join().expect("writer");
        reader.join().expect("reader");
    }
}
