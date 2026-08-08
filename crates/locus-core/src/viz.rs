//! Self-contained HTML rendering for the memory graph (U-016).
//!
//! The snapshot is a single offline file: all CSS and JS are inlined, graph
//! data is embedded as JSON, and no external resources are referenced. The live
//! page shares the same renderer but loads its initial data from `/data` and
//! updates over an SSE `/events` stream.
//!
//! Every rendered string (node title/content/entities/namespace/memory-type
//! and edge labels) is passed through the secret scanner before serialization,
//! so the rendered graph can never leak a detected secret even if one was
//! stored verbatim via `allow_secret`.
//!
//! The force-layout renderer itself lives in `renderer.js` alongside this
//! file rather than as an inline Rust string: it's a few hundred lines of
//! real interactive logic (drag-to-orbit, namespace staging/decluster,
//! incremental SVG diffing), and keeping it as a `.js` file lets editors and
//! linters actually understand it. `include_str!` inlines it at compile time,
//! so the output is still a single offline HTML file — nothing changes at
//! runtime.

use crate::graph::{GraphData, GraphEdge, GraphNode};
use crate::Result;

use serde::Serialize;

/// Wire-format edge: node indexes into `LeanPayload::nodes` instead of full
/// UUIDs. The renderer only needs source/target, and a graph with tens of
/// thousands of edges would otherwise ship 36-char ids twice per edge.
#[derive(Serialize)]
struct LeanEdge {
    source: usize,
    target: usize,
}

/// Wire-format payload: nodes unchanged, edges reduced to indexes.
#[derive(Serialize)]
struct LeanPayload {
    nodes: Vec<GraphNode>,
    edges: Vec<LeanEdge>,
    truncated: bool,
}

/// Redacts any detected secret from the text rendered into a graph, replacing
/// it with its `[REDACTED:rule-id]` placeholder.
fn redacted(text: &str) -> String {
    crate::security::redact(text).0
}

fn redacted_graph(data: &GraphData) -> GraphData {
    let nodes = data
        .nodes
        .iter()
        .map(|node| GraphNode {
            title: redacted(&node.title),
            content: redacted(&node.content),
            // namespace is user-supplied (CLI/host payload), so scan it too;
            // memory_type is scanned for uniformity even though it is
            // enum-derived and can never carry a secret.
            namespace: redacted(&node.namespace),
            memory_type: redacted(&node.memory_type),
            entities: node
                .entities
                .iter()
                .map(|entity| redacted(entity))
                .collect(),
            ..node.clone()
        })
        .collect();
    let edges = data
        .edges
        .iter()
        .map(|edge| GraphEdge {
            label: redacted(&edge.label),
            ..edge.clone()
        })
        .collect();
    GraphData {
        nodes,
        edges,
        truncated: data.truncated,
    }
}

/// Serializes graph data for embedding into a `<script>` block or the `/data`
/// endpoint. Detected secrets are redacted and `<` is escaped as `\u003c` so a
/// hostile title can never break out of the script element.
///
/// Edges are serialized as node indexes rather than ids: the renderer addresses
/// nodes positionally, and this keeps the payload small even when a graph has
/// tens of thousands of edges.
pub fn graph_payload_json(data: &GraphData) -> Result<String> {
    let data = redacted_graph(data);
    let index: std::collections::HashMap<&str, usize> = data
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let edges = data
        .edges
        .iter()
        .filter_map(
            |e| match (index.get(e.source.as_str()), index.get(e.target.as_str())) {
                (Some(&s), Some(&t)) => Some(LeanEdge {
                    source: s,
                    target: t,
                }),
                _ => None,
            },
        )
        .collect();
    let payload = LeanPayload {
        nodes: data.nodes,
        edges,
        truncated: data.truncated,
    };
    let json = serde_json::to_string(&payload)
        .map_err(|err| crate::Error::Other(format!("failed to serialize graph: {err}")))?;
    Ok(json.replace('<', "\\u003c"))
}

/// A complete, offline-capable HTML page with the graph data embedded.
pub fn snapshot_html(data: &GraphData) -> Result<String> {
    let json = graph_payload_json(data)?;
    Ok(SNAPSHOT_TEMPLATE
        .replace("__GRAPH_DATA__", &json)
        .replace("__RENDERER__", RENDERER_JS))
}

/// The live page: fetches `/data` for the initial graph and subscribes to the
/// SSE `/events` stream for updates.
pub fn live_html() -> String {
    LIVE_TEMPLATE.replace("__RENDERER__", RENDERER_JS)
}

/// The force-layout renderer shared by snapshot and live views. Compiled with
/// either an embedded `GRAPH_DATA` (snapshot) or the `/data` + `/events` live
/// wiring. Lives in `renderer.js`, next to this file, so it can be edited and
/// linted as ordinary JavaScript; `include_str!` pulls it in verbatim at
/// compile time.
const RENDERER_JS: &str = include_str!("renderer.js");

const SNAPSHOT_TEMPLATE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Locus Memory Graph</title>
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body { margin: 0; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
         background: #0d1117; color: #c9d1d9; overflow: hidden; }
  #app { position: fixed; inset: 0; }
  svg { width: 100%; height: 100%; display: block; cursor: grab; touch-action: none; }
  #panel { position: fixed; left: 12px; top: 12px; max-width: 360px; max-height: 80vh;
           overflow: auto; background: rgba(13,17,23,.94); border: 1px solid #30363d;
           border-radius: 8px; padding: 12px 14px; font-size: 12px; display: none; line-height: 1.5; }
  #panel h2 { margin: 0 0 6px; font-size: 14px; word-break: break-word; }
  #panel p { margin: 4px 0; }
  #panel code { background: #161b22; padding: 1px 5px; border-radius: 4px; word-break: break-all; }
  #panel .content { margin: 8px 0; padding: 8px 10px; background: #161b22; border-left: 2px solid #e14d8c;
                       border-radius: 4px; font-size: 12px; line-height: 1.6; white-space: pre-wrap; word-break: break-word; }
  svg circle { cursor: pointer; }
  .badge { display: inline-block; border-radius: 4px; padding: 1px 7px; margin-right: 6px;
           font-size: 11px; background: #1f6feb; color: #fff; }
  .badge.ns { background: #30363d; }
  #legend { position: fixed; right: 12px; bottom: 12px; background: rgba(13,17,23,.9);
            border: 1px solid #30363d; border-radius: 8px; padding: 10px 14px; font-size: 11px; }
  #legend .dot { display: inline-block; width: 10px; height: 10px; border-radius: 50%;
                 margin-right: 6px; vertical-align: middle; }
  #legend .lrow { cursor: pointer; padding: 2px 6px; margin: 0 -6px; border-radius: 4px;
                  transition: background .12s; display: flex; align-items: center; gap: 4px; }
  #legend .lrow:hover { background: #21262d; text-decoration: underline; }
  #legend .lrow.pin { background: #21262d; box-shadow: inset 0 0 0 1px #8b949e; }
  #legend .ncount { margin-left: auto; color: #8b949e; font-size: 10px; }
  #legend .hint { color: #8b949e; margin-top: 4px; }
  #pinchip { display: none; position: fixed; left: 50%; top: 12px; transform: translateX(-50%);
             background: rgba(13,17,23,.94); border: 1px solid #8b949e; border-radius: 8px;
             padding: 8px 14px; font-size: 12px; cursor: pointer; z-index: 5;
             box-shadow: 0 4px 16px rgba(0,0,0,.35); }
  #pinchip:hover { border-color: #c9d1d9; background: #161b22; }
  #pinchip .unpin { color: #8b949e; margin-left: 8px; font-size: 11px; }
  #truncated { display: none; position: fixed; left: 12px; bottom: 12px; background: #3d2f00;
               border: 1px solid #9e6a03; color: #f2cc60; border-radius: 8px;
               padding: 6px 10px; font-size: 11px; }
</style>
</head>
<body>
<div id="app"></div>
<div id="panel"></div>
<div id="legend">
  <div>namespaces</div>
</div>
<div id="truncated">Graph truncated to the configured node cap.</div>
<script>
"use strict";
const GRAPH_DATA = __GRAPH_DATA__;
</script>
<script>
__RENDERER__
</script>
</body>
</html>
"##;

const LIVE_TEMPLATE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Locus Memory Graph — live</title>
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body { margin: 0; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
         background: #0d1117; color: #c9d1d9; overflow: hidden; }
  #app { position: fixed; inset: 0; }
  svg { width: 100%; height: 100%; display: block; cursor: grab; touch-action: none; }
  #panel { position: fixed; left: 12px; top: 12px; max-width: 360px; max-height: 80vh;
           overflow: auto; background: rgba(13,17,23,.94); border: 1px solid #30363d;
           border-radius: 8px; padding: 12px 14px; font-size: 12px; display: none; line-height: 1.5; }
  #panel h2 { margin: 0 0 6px; font-size: 14px; word-break: break-word; }
  #panel p { margin: 4px 0; }
   #panel code { background: #161b22; padding: 1px 5px; border-radius: 4px; word-break: break-all; }
   #panel .content { margin: 8px 0; padding: 8px 10px; background: #161b22; border-left: 2px solid #e14d8c;
                     border-radius: 4px; font-size: 12px; line-height: 1.6; white-space: pre-wrap; word-break: break-word; }
   svg circle { cursor: pointer; }
   .badge { display: inline-block; border-radius: 4px; padding: 1px 7px; margin-right: 6px;
           font-size: 11px; background: #1f6feb; color: #fff; }
  .badge.ns { background: #30363d; }
  #legend { position: fixed; right: 12px; bottom: 12px; background: rgba(13,17,23,.9);
            border: 1px solid #30363d; border-radius: 8px; padding: 10px 14px; font-size: 11px; }
  #legend .dot { display: inline-block; width: 10px; height: 10px; border-radius: 50%;
                 margin-right: 6px; vertical-align: middle; }
  #legend .lrow { cursor: pointer; padding: 2px 6px; margin: 0 -6px; border-radius: 4px;
                  transition: background .12s; display: flex; align-items: center; gap: 4px; }
   #legend .lrow:hover { background: #21262d; text-decoration: underline; }
   #legend .lrow.pin { background: #21262d; box-shadow: inset 0 0 0 1px #8b949e; }
  #legend .ncount { margin-left: auto; color: #8b949e; font-size: 10px; }
  #legend .hint { color: #8b949e; margin-top: 4px; }
  #pinchip { display: none; position: fixed; left: 50%; top: 12px; transform: translateX(-50%);
             background: rgba(13,17,23,.94); border: 1px solid #8b949e; border-radius: 8px;
             padding: 8px 14px; font-size: 12px; cursor: pointer; z-index: 5;
             box-shadow: 0 4px 16px rgba(0,0,0,.35); }
  #pinchip:hover { border-color: #c9d1d9; background: #161b22; }
  #pinchip .unpin { color: #8b949e; margin-left: 8px; font-size: 11px; }
   #status { position: fixed; right: 12px; top: 12px; background: rgba(13,17,23,.9);
            border: 1px solid #30363d; border-radius: 8px; padding: 6px 10px; font-size: 11px; }
  #status.live { border-color: #238636; color: #3fb950; }
  #truncated { display: none; position: fixed; left: 12px; bottom: 12px; background: #3d2f00;
               border: 1px solid #9e6a03; color: #f2cc60; border-radius: 8px;
               padding: 6px 10px; font-size: 11px; }
</style>
</head>
<body>
<div id="app"></div>
<div id="panel"></div>
<div id="status" class="live">live</div>
<div id="legend">
  <div>namespaces</div>
</div>
<div id="truncated">Graph truncated to the configured node cap.</div>
<script>
__RENDERER__
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphData, GraphEdge, GraphNode};

    fn sample() -> GraphData {
        GraphData {
            nodes: vec![
                GraphNode {
                    id: "n1".to_string(),
                    title: "Deploy token".to_string(),
                    content: "Use a short-lived deploy token".to_string(),
                    namespace: "project:auth".to_string(),
                    memory_type: "fact".to_string(),
                    importance: 80,
                    access_count: 5,
                    created_at: 1_000,
                    updated_at: 2_000,
                    last_accessed_at: Some(2_000),
                    entities: vec![
                        "deploy".to_string(),
                        "ghp_123456789012345678901234567890123456".to_string(),
                    ],
                },
                GraphNode {
                    id: "n2".to_string(),
                    title: "Database choice".to_string(),
                    content: "Postgres for auth state".to_string(),
                    namespace: "project:auth".to_string(),
                    memory_type: "decision".to_string(),
                    importance: 90,
                    access_count: 2,
                    created_at: 1_000,
                    updated_at: 2_000,
                    last_accessed_at: None,
                    entities: vec!["deploy".to_string()],
                },
            ],
            edges: vec![GraphEdge {
                source: "n1".to_string(),
                target: "n2".to_string(),
                label: "deploy".to_string(),
                weight: 1,
            }],
            truncated: false,
        }
    }

    #[test]
    fn snapshot_html_embeds_data_and_is_self_contained() {
        let html = snapshot_html(&sample()).unwrap();
        // The renderer creates the SVG dynamically via the SVG namespace.
        assert!(html.contains("createElementNS"));
        assert!(html.contains("const GRAPH_DATA ="));
        assert!(html.contains("\"n1\""));
        // Offline: no external resources are referenced.
        assert!(!html.contains("<script src="));
        assert!(!html.contains("<link "));
        assert!(!html.contains("<img "));
    }

    #[test]
    fn rendered_payload_redacts_secrets() {
        let json = graph_payload_json(&sample()).unwrap();
        assert!(
            !json.contains("ghp_123456789012345678901234567890123456"),
            "secret must not appear in the rendered payload"
        );
        assert!(json.contains("[REDACTED:github-pat]"));
        assert!(json.contains("\"Deploy token\""));
    }

    #[test]
    fn rendered_payload_redacts_namespace_and_memory_type() {
        let mut data = sample();
        data.nodes[0].namespace = "ghp_123456789012345678901234567890123456-ns".to_string();
        data.nodes[0].memory_type = "ghp_123456789012345678901234567890123456-type".to_string();
        let json = graph_payload_json(&data).unwrap();
        assert!(
            !json.contains("ghp_123456789012345678901234567890123456"),
            "secret must not appear in namespace or memory_type"
        );
        assert!(json.contains("[REDACTED:github-pat]-ns"));
        assert!(json.contains("[REDACTED:github-pat]-type"));
    }

    #[test]
    fn payload_escapes_script_breaking_characters() {
        let mut data = sample();
        data.nodes[0].title = "</script><script>alert(1)</script>".to_string();
        let json = graph_payload_json(&data).unwrap();
        assert!(
            !json.contains("</script>"),
            "a hostile title must not terminate the script element"
        );
        assert!(json.contains("\\u003c"));
    }

    #[test]
    fn live_html_wires_data_and_events() {
        let html = live_html();
        assert!(html.contains("fetch(\"/data\")"));
        assert!(html.contains("EventSource(\"/events\")"));
        assert!(!html.contains("const GRAPH_DATA"));
        assert!(!html.contains("<script src="));
        assert!(!html.contains("<link "));
        assert!(!html.contains("<img "));
    }

    #[test]
    fn snapshot_of_empty_graph_is_valid() {
        let html = snapshot_html(&GraphData {
            nodes: vec![],
            edges: vec![],
            truncated: false,
        })
        .unwrap();
        assert!(html.contains("const GRAPH_DATA ="));
        assert!(html.contains("[]"));
    }

    #[test]
    fn renderer_guards_draw_until_svg_is_built() {
        // Live mode calls refresh() asynchronously; the first tick can fire
        // before setData()/buildSvg() runs. draw() must no-op in that window
        // instead of throwing on undefined layer nodes. Checked directly
        // against RENDERER_JS (not the substring-sliced HTML) and requires
        // the guard to be the first statement inside draw() specifically —
        // adjacency to the function signature makes this robust to
        // reordering other functions in renderer.js, unlike slicing text
        // between two function names.
        assert!(
            RENDERER_JS.contains("function draw() {\n    if (!linkLayer || !nodeLayer) return;"),
            "draw() must guard against layers that are not built yet, as its first statement"
        );
    }

    #[test]
    fn renderer_js_has_no_duplicate_repulse_dist_declaration() {
        // Regression guard: tick() previously redeclared `var REPULSE_DIST`
        // inside its pinned-namespace branch, shadowing the module-level
        // tuning constant of the same name. Only one `var REPULSE_DIST`
        // declaration should exist.
        let decls = RENDERER_JS.matches("var REPULSE_DIST").count();
        assert_eq!(decls, 1, "REPULSE_DIST must be declared exactly once");
    }
}
