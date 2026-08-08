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
/// wiring.
const RENDERER_JS: &str = r##"
(function () {
  "use strict";
  var app = document.getElementById("app");
  var panel = document.getElementById("panel");
  var legendEl = document.getElementById("legend");
  var truncatedEl = document.getElementById("truncated");
  var nodes = [];
  var nodeMap = {};
  var edges = [];
  var maxAccess = 1;
  var selected = null;
  var hoveredId = null;
  var W = window.innerWidth, H = window.innerHeight;
  var svg, view, linkLayer, nodeLayer;
  var nodeEls = {}, hitEls = {};
  var linkPath = null, activePath = null;
  var nodeEdges = {};
  var cam = { x: 0, y: 0, k: 1 };
  var autoFit = true, userControlled = false;
  // camTarget is a destination the camera eases toward each tick (used by the
  // legend namespace focus); null means no focused destination is pending.
  var camTarget = null;
  // pinnedNs is the namespace whose cloud stays pinned when a legend row is
  // clicked. While set, leaving the legend returns to this cloud instead of the
  // overview; hovering other rows still previews, but the pinned cloud wins on
  // mouseleave. Clicking the pinned row again unpins back to the overview.
  var pinnedNs = null;
  var drag = null, dragMoved = false;
  // Incremental render flags. Links and nodes are heavy (57k+ links, 2k dots on
  // a big graph), so draw() only touches the DOM when something actually moved:
  // linkDirty/nodeDirty while the force sim runs, camDirty while the camera
  // eases, and focusDirty when the hovered/pinned node changes.
  var linkDirty = true, nodeDirty = true, camDirty = true, focusDirty = true;
  var lastCamK = 1;
  var lastFocus = "";
  // Layout freeze: once the largest node velocity stays below EPS for
  // SETTLE_FRAMES consecutive frames, the physics stops re-applying forces and
  // positions lock, so a converged graph no longer "dances" under 60fps ticks.
  // A wall-clock simulation budget guarantees freeze even for very large graphs
  // where the O(n^2) repulsion never settles below EPS on its own: forces cool
  // to zero over SIM_MS (regardless of frame rate), then the layout is locked.
  var settled = false, stillFrames = 0;
  var SETTLE_EPS = 0.02;
  var SETTLE_FRAMES = 30;
  var simStart = 0;
  var SIM_MS = 3500;
  var lastTick = Date.now();
  // Maximum per-pair repulsion acceleration (world units/frame^2) and the
  // radius beyond which nodes stop repelling. Together they spread each cluster
  // locally while letting namespace clouds sit close to each other instead of
  // repelling across the whole canvas.
  var REPULSE_ACCEL = 0.9;
  var REPULSE_DIST = 260;

  // Absolute, per-visit dot growth: base radius with a logarithmic ramp so a
  // node keeps getting bigger every time it is visited, but never beyond
  // MAX_DOT_RADIUS. Growth is independent of the busiest node, so one very hot
  // node can not shrink its neighbours. The draw() screen clamp guarantees a
  // heavily visited node can never cover the view even fully zoomed in.
  var BASE_DOT_RADIUS = 12;
  var DOT_GROWTH = 9;
  var MAX_DOT_RADIUS = 64;
  function radius(n) {
    return Math.min(MAX_DOT_RADIUS, BASE_DOT_RADIUS + DOT_GROWTH * Math.log10(1 + (n.acc || 0)));
  }
  // Distribute namespace hues across the whole wheel using the golden angle,
  // so many namespaces stay visually distinct. Hues are assigned per data set
  // in sorted namespace order (see buildNsHues), guaranteeing adjacent
  // namespaces land far apart on the wheel instead of colliding.
  var GOLDEN_ANGLE = 137.508;
  var nsHues = {};
  function buildNsHues() {
    nsHues = {};
    var names = [];
    for (var i = 0; i < nodes.length; i++) {
      var ns = nodes[i].ns || "global";
      if (names.indexOf(ns) === -1) names.push(ns);
    }
    names.sort();
    for (var k = 0; k < names.length; k++) {
      if (names[k] === "global") continue;
      nsHues[names[k]] = Math.round((k * GOLDEN_ANGLE) % 360);
    }
  }
  function nsHue(ns) {
    if (!ns || ns === "global") return -1;
    if (nsHues[ns] !== undefined) return nsHues[ns];
    var h = 0;
    for (var i = 0; i < ns.length; i++) h = (h * 31 + ns.charCodeAt(i)) >>> 0;
    return Math.round((h * GOLDEN_ANGLE) % 360);
  }
  function color(n) {
    var now = Date.now() / 1000;
    var ageDays = Math.max(0, (now - (n.updated || now)) / 86400);
    var t = Math.min(1, ageDays / 90);
    var hue = nsHue(n.ns);
    if (hue < 0) {
      return "hsl(0,0%," + Math.round(55 + 22 * t) + "%)";
    }
    var sat = Math.round(70 - 35 * t);
    var lit = Math.round(50 + 18 * t);
    return "hsl(" + hue + "," + sat + "%," + lit + "%)";
  }
  function nsColor(ns) {
    var hue = nsHue(ns);
    if (hue < 0) return "hsl(0,0%,65%)";
    return "hsl(" + hue + ",70%,52%)";
  }
  function buildLegend() {
    if (!legendEl) return;
    var seen = {};
    for (var i = 0; i < nodes.length; i++) seen[nodes[i].ns] = true;
    var lines = [];
    Object.keys(seen).sort().forEach(function (ns) {
      lines.push('<div class="lrow" data-ns="' + esc(ns) + '"><span class="dot" style="background:' +
        nsColor(ns) + '"></span>' + esc(ns) + '</div>');
    });
    lines.push('<div>node size = retrieval frequency</div>');
    lines.push('<div>brighter = more recently touched</div>');
    lines.push('<div>hover to inspect &middot; click to pin</div>');
    lines.push('<div>hover a namespace to focus it</div>');
    legendEl.innerHTML = lines.join("");
  }
  // Bounding box for the nodes in one namespace, or the whole graph when no
  // namespace is given. Used by fitCamera() (overview) and focusNamespace()
  // (legend zoom).
  function nsBounds(ns) {
    var minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (var i = 0; i < nodes.length; i++) {
      if (ns && nodes[i].ns !== ns) continue;
      if (nodes[i].x < minX) minX = nodes[i].x;
      if (nodes[i].x > maxX) maxX = nodes[i].x;
      if (nodes[i].y < minY) minY = nodes[i].y;
      if (nodes[i].y > maxY) maxY = nodes[i].y;
    }
    return minX === Infinity ? null : { minX: minX, maxX: maxX, minY: minY, maxY: maxY };
  }
  function fitToBounds(b, pad) {
    if (!b) return null;
    var bw = Math.max(1, b.maxX - b.minX), bh = Math.max(1, b.maxY - b.minY);
    var k = Math.max(0.05, Math.min(3, Math.min((W - pad * 2) / bw, (H - pad * 2) / bh) * 0.9));
    return { x: W / 2 - (b.minX + b.maxX) / 2 * k, y: H / 2 - (b.minY + b.maxY) / 2 * k, k: k };
  }
  function fitCamera() {
    return fitToBounds(nsBounds(), 80);
  }
  function focusNamespace(ns) {
    var b = nsBounds(ns);
    var t = fitToBounds(b, 90);
    if (t) camTarget = t;
  }
  function fit() {
    var t = fitCamera();
    if (t) { cam.x = t.x; cam.y = t.y; cam.k = t.k; }
  }
  function buildSvg() {
    svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    view = document.createElementNS("http://www.w3.org/2000/svg", "g");
    linkLayer = document.createElementNS("http://www.w3.org/2000/svg", "g");
    nodeLayer = document.createElementNS("http://www.w3.org/2000/svg", "g");
    nodeEls = {};
    hitEls = {};
    // All links render as one <path> (a single d attribute instead of one
    // <line> element per edge) so large graphs stay cheap to draw. A second
    // path holds the connections of the hovered/pinned node on top.
    linkPath = document.createElementNS("http://www.w3.org/2000/svg", "path");
    linkPath.setAttribute("fill", "none");
    linkPath.setAttribute("stroke", "#21262d");
    linkPath.setAttribute("stroke-width", 1);
    activePath = document.createElementNS("http://www.w3.org/2000/svg", "path");
    activePath.setAttribute("fill", "none");
    activePath.setAttribute("stroke", "#8b949e");
    activePath.setAttribute("stroke-width", 2);
    linkLayer.appendChild(linkPath);
    linkLayer.appendChild(activePath);
    svg.appendChild(view);
    view.appendChild(linkLayer);
    view.appendChild(nodeLayer);
    app.appendChild(svg);
    // Pre-index edges by endpoint node id so the focus path can be rebuilt in
    // O(deg) instead of scanning every edge. Edge endpoints are node indexes
    // into `nodes`, so resolve them to ids here.
    nodeEdges = {};
    for (var k = 0; k < edges.length; k++) {
      var e = edges[k];
      var sa = nodes[e.source], sb = nodes[e.target];
      if (!sa || !sb) continue;
      (nodeEdges[sa.id] = nodeEdges[sa.id] || []).push(k);
      (nodeEdges[sb.id] = nodeEdges[sb.id] || []).push(k);
    }
    linkDirty = true;
    nodeDirty = true;
    camDirty = true;
    focusDirty = true;
    nodeLayer.addEventListener("click", function (e) {
      if (dragMoved) return;
      var c = e.target.closest && e.target.closest("circle");
      if (!c) return;
      var n = nodeMap[c.getAttribute("data-id")];
      if (!n) return;
      if (selected && selected.id === n.id) {
        // Clicking the pinned node again unpins it.
        selected = null;
        focusDirty = true;
        hidePanel();
      } else {
        select(n);
      }
    });
    // Hover reveals the panel immediately; clicking pins it (the panel stays
    // open even after the pointer leaves).
    nodeLayer.addEventListener("pointerover", function (e) {
      var c = e.target.closest && e.target.closest("circle");
      if (!c) return;
      var n = nodeMap[c.getAttribute("data-id")];
      if (n && n.id !== hoveredId) {
        hoveredId = n.id;
        focusDirty = true;
        showPanel(n);
      }
    });
    nodeLayer.addEventListener("pointerout", function (e) {
      if (hoveredId) focusDirty = true;
      hoveredId = null;
      // relatedTarget is what the pointer moved onto. Only hide when leaving
      // nodes entirely (not onto another node, whose pointerover reopens the
      // panel). If a node is pinned, revert the panel to it instead of hiding.
      var rt = e.relatedTarget;
      var onto = rt && rt.closest ? rt.closest("circle") : null;
      if (!onto) {
        if (selected) showPanel(selected);
        else hidePanel();
      }
    });
    svg.addEventListener("pointerdown", function (e) {
      dragMoved = false;
      if (e.target === nodeLayer || e.target.closest && e.target.closest("circle")) return;
      camTarget = null;
      drag = { x: e.clientX, y: e.clientY, camX: cam.x, camY: cam.y };
      svg.setPointerCapture(e.pointerId);
    });
    svg.addEventListener("pointermove", function (e) {
      if (!drag) return;
      if (Math.abs(e.clientX - drag.x) + Math.abs(e.clientY - drag.y) > 3) dragMoved = true;
      cam.x = drag.camX + (e.clientX - drag.x);
      cam.y = drag.camY + (e.clientY - drag.y);
      camDirty = true;
      userControlled = true;
      autoFit = false;
    });
    svg.addEventListener("pointerup", function () { drag = null; });
    svg.addEventListener("wheel", function (e) {
      e.preventDefault();
      camTarget = null;
      var f = Math.exp(-e.deltaY * 0.0012);
      var wx = (e.clientX - cam.x) / cam.k;
      var wy = (e.clientY - cam.y) / cam.k;
      cam.k = Math.max(0.05, Math.min(20, cam.k * f));
      cam.x = e.clientX - wx * cam.k;
      cam.y = e.clientY - wy * cam.k;
      camDirty = true;
      userControlled = true;
      autoFit = false;
    }, { passive: false });
    window.addEventListener("resize", function () {
      W = window.innerWidth;
      H = window.innerHeight;
      camDirty = true;
      if (!userControlled) fit();
    });
    // Legend namespaces are focus links: hovering one eases the camera to that
    // namespace's cloud (full fit on screen). Moving off it returns to the
    // overall fit, and any drag/wheel/click takes over from there. Clicking a
    // row pins that cloud: the camera returns to it on mouseleave and the row
    // gets a pin marker. Clicking the pinned row again unpins.
    if (legendEl && !legendEl._bound) {
      legendEl._bound = true;
      legendEl.addEventListener("mouseover", function (e) {
        var row = e.target.closest && e.target.closest(".lrow");
        if (row) {
          var ns = row.getAttribute("data-ns");
          focusNamespace(ns);
          autoFit = false;
          userControlled = false;
        }
      });
      legendEl.addEventListener("mouseleave", function () {
        if (pinnedNs) {
          focusNamespace(pinnedNs);
        } else {
          camTarget = fitCamera();
        }
      });
      legendEl.addEventListener("click", function (e) {
        var row = e.target.closest && e.target.closest(".lrow");
        if (!row) return;
        var ns = row.getAttribute("data-ns");
        if (pinnedNs === ns) {
          pinnedNs = null;
          camTarget = fitCamera();
        } else {
          pinnedNs = ns;
          focusNamespace(ns);
          autoFit = false;
          userControlled = false;
        }
        for (var i = 0; i < legendEl.children.length; i++) {
          var ch = legendEl.children[i];
          if (ch.classList) ch.classList.toggle("pin", ch === row && !!pinnedNs);
        }
      });
    }
  }
  function setData(data) {
    var prev = nodeMap;
    while (app.firstChild) app.removeChild(app.firstChild);
    nodes = (data.nodes || []).map(function (n) {
      var old = prev[n.id];
      return { id: n.id, title: n.title, ns: n.namespace, type: n.memory_type,
        imp: n.importance, acc: n.access_count, created: n.created_at,
        updated: n.updated_at, entities: n.entities || [], content: n.content || "",
        x: old ? old.x : 0, y: old ? old.y : 0, vx: old ? old.vx : 0, vy: old ? old.vy : 0,
        fresh: !old,
        fade: old ? (old.fade === undefined ? 1 : old.fade) : 0,
        pulse: old ? (old.pulse || 0) : 0 };
    });
    nodeMap = {};
    maxAccess = 1;
    for (var i = 0; i < nodes.length; i++) {
      nodeMap[nodes[i].id] = nodes[i];
      if (nodes[i].acc > maxAccess) maxAccess = nodes[i].acc;
    }
    edges = (data.edges || []).slice();
    if (data.truncated && truncatedEl) truncatedEl.style.display = "block";
    buildSvg();
    scatter();
    fit();
    autoFit = true;
    settled = false;
    stillFrames = 0;
    simStart = Date.now();
    buildNsHues();
    buildLegend();
  }
  function scatter() {
    // Cluster-aware seed: each namespace's nodes start grouped around its own
    // center, and the namespace centers sit on a small ring near the origin.
    // This keeps the clouds close together in the initial view (instead of a
    // random square whose span grows with node count), so the whole graph is
    // reachable by mouse drag without long panning. The force layout then
    // tightens each cluster while cooling locks it in place.
    var names = [];
    var counts = {};
    for (var i = 0; i < nodes.length; i++) {
      var ns = nodes[i].ns || "global";
      if (!counts[ns]) { counts[ns] = 0; names.push(ns); }
      counts[ns]++;
    }
    names.sort();
    var ring = 40 + Math.sqrt(names.length) * 40;
    var centers = {};
    for (var i = 0; i < names.length; i++) {
      var a = (i / names.length) * Math.PI * 2;
      centers[names[i]] = { x: Math.cos(a) * ring, y: Math.sin(a) * ring };
    }
    for (var i = 0; i < nodes.length; i++) {
      var n = nodes[i];
      if (!n.fresh) continue;
      var ns = n.ns || "global";
      var c = centers[ns] || { x: 0, y: 0 };
      var cr = 25 + Math.sqrt(counts[ns]) * 6;
      n.x = c.x + (Math.random() * 2 - 1) * cr;
      n.y = c.y + (Math.random() * 2 - 1) * cr;
    }
  }
  function applyEvent(ev) {
    var n = nodeMap[ev.memory_id];
    if (!n) { refresh(); return; }
    // Unknown memory_id means a memory created after the initial /data fetch;
    // the event carries no title/content, so a full refresh pulls the node in.
    // Existing nodes update in place (positions survive via the prev lookup).
    n.acc = (n.acc || 0) + (ev.access_delta || 0);
    if (n.acc > maxAccess) maxAccess = n.acc;
    n.updated = ev.timestamp;
    if (ev.access_delta) {
      n.pulse = 1;
      nodeDirty = true;
    }
  }
  function tick() {
    if (!settled) {
      // Cooling factor: forces ramp down as the simulation budget is spent, so
      // even a huge O(n^2) layout converges and locks instead of jittering.
      var elapsed = simStart ? (Date.now() - simStart) : 0;
      var cool = elapsed < SIM_MS ? 1 - elapsed / SIM_MS : 0;      // Pairwise repulsion is O(n^2) per frame, but only acts within a local
      // radius. A cutoff keeps clouds from shoving each other across the whole
      // canvas: nodes inside a namespace repel to spread the cluster, while
      // clusters beyond the cutoff leave each other alone and stay close. The
      // per-pair acceleration is capped so densely seeded clusters spread out
      // instead of launching each other across the view.
      var REPULSE_DIST = 260;
      for (var i = 0; i < nodes.length; i++) {
        var a = nodes[i];
        for (var j = i + 1; j < nodes.length; j++) {
          var b = nodes[j];
          var dx = a.x - b.x, dy = a.y - b.y;
          var d2 = dx * dx + dy * dy + 1;
          if (d2 > REPULSE_DIST * REPULSE_DIST) continue;
          // Force factor such that accel = f*dx; raw = 2600/d^2 gives a 1/d
          // acceleration that blows up for overlapping seeds. Clamp so a pair
          // never pushes harder than REPULSE_ACCEL regardless of distance.
          var d = Math.sqrt(d2);
          var f = Math.min(2600 / d2, REPULSE_ACCEL / d) * cool;
          a.vx += f * dx; a.vy += f * dy;
          b.vx -= f * dx; b.vy -= f * dy;
        }
      }
      for (var k = 0; k < edges.length; k++) {
        var e = edges[k];
        var a = nodeMap[e.source], b = nodeMap[e.target];
        if (!a || !b) continue;
        var dx = b.x - a.x, dy = b.y - a.y;
        var d = Math.sqrt(dx * dx + dy * dy) || 1;
        var f = 0.012 * (d - 90) / d * cool;
        a.vx += f * dx; a.vy += f * dy;
        b.vx -= f * dx; b.vy -= f * dy;
      }
      for (var i = 0; i < nodes.length; i++) {
        var n = nodes[i];
        n.vx += (0 - n.x) * 0.004 * cool;
        n.vy += (0 - n.y) * 0.004 * cool;
        n.x += n.vx; n.y += n.vy;
        n.vx *= 0.82; n.vy *= 0.82;
        if (n.fade < 1) n.fade = Math.min(1, n.fade + 0.05);
        if (n.pulse > 0) n.pulse *= 0.92;
      }
      // Freeze the layout once it has converged, so the dots stop dancing.
      var maxV = 0;
      for (var i = 0; i < nodes.length; i++) {
        var n = nodes[i];
        var v = Math.abs(n.vx) > Math.abs(n.vy) ? Math.abs(n.vx) : Math.abs(n.vy);
        if (v > maxV) maxV = v;
      }
      if (maxV < SETTLE_EPS || elapsed >= SIM_MS) {
        if (++stillFrames >= SETTLE_FRAMES || elapsed >= SIM_MS) {
          for (var i = 0; i < nodes.length; i++) { nodes[i].vx = 0; nodes[i].vy = 0; }
          settled = true;
        }
      } else {
        stillFrames = 0;
      }
      linkDirty = true;
      nodeDirty = true;
    }
    // Time-based camera easing: speed depends on real elapsed time, not frame
    // count, so centering a cloud feels the same at any frame rate. The rate is
    // tuned fast enough that focusing a namespace from the overview settles in
    // well under a second on a heavy graph.
    var now = Date.now();
    var dt = Math.min(0.1, (now - lastTick) / 1000);
    lastTick = now;
    var ease = 1 - Math.exp(-dt * 12);
    if (camTarget) {
      // Ease toward a focused destination (legend namespace). Once converged,
      // the target is cleared so the user's own drag/wheel take over.
      var tx = camTarget;
      cam.k += (tx.k - cam.k) * ease;
      cam.x += (tx.x - cam.x) * ease;
      cam.y += (tx.y - cam.y) * ease;
      camDirty = true;
      if (Math.abs(cam.k - tx.k) < 0.0005 &&
          Math.abs(cam.x - tx.x) < 0.5 &&
          Math.abs(cam.y - tx.y) < 0.5) {
        cam = tx;
        camTarget = null;
      }
    } else if (autoFit) {
      var t = fitCamera();
      if (t) {
        cam.k += (t.k - cam.k) * ease;
        cam.x += (t.x - cam.x) * ease;
        cam.y += (t.y - cam.y) * ease;
        camDirty = true;
        if (Math.abs(cam.k - t.k) < 0.0005 &&
            Math.abs(cam.x - t.x) < 0.5 &&
            Math.abs(cam.y - t.y) < 0.5) {
          cam = t;
          autoFit = false;
        }
      }
    }
    draw();
    requestAnimationFrame(tick);
  }
  function draw() {
    if (!linkLayer || !nodeLayer) return;
    if (camDirty) {
      view.setAttribute("transform",
        "translate(" + cam.x.toFixed(1) + "," + cam.y.toFixed(1) + ") scale(" + cam.k.toFixed(4) + ")");
      camDirty = false;
    }
    var focus = hoveredId || (selected ? selected.id : null);
    var posChanged = linkDirty;
    // Links: one <path> for every edge, rebuilt only when the layout moved.
    if (linkDirty) {
      var d = "";
      for (var k = 0; k < edges.length; k++) {
        var e = edges[k];
        var a = nodes[e.source], b = nodes[e.target];
        if (!a || !b) continue;
        d += "M" + a.x.toFixed(1) + " " + a.y.toFixed(1) + "L" + b.x.toFixed(1) + " " + b.y.toFixed(1);
      }
      linkPath.setAttribute("d", d);
      linkDirty = false;
    }
    // Focus links: the connections of the hovered/pinned node, drawn on top.
    // Rebuilt when focus changes or when the layout moved (node positions).
    if (focusDirty || posChanged) {
      var fd = "";
      if (focus) {
        var idxs = nodeEdges[focus];
        for (var m = 0; idxs && m < idxs.length; m++) {
          var fe = edges[idxs[m]];
          var fa = nodes[fe.source], fb = nodes[fe.target];
          if (!fa || !fb) continue;
          fd += "M" + fa.x.toFixed(1) + " " + fa.y.toFixed(1) + "L" + fb.x.toFixed(1) + " " + fb.y.toFixed(1);
        }
      }
      activePath.setAttribute("d", fd);
      focusDirty = false;
    }
    // Nodes: update positions while the sim runs, appearance when the camera
    // zooms (radius clamps are screen-space), and highlight when focus changes.
    var usedNodes = {};
    for (var i = 0; i < nodes.length; i++) {
      var n = nodes[i];
      usedNodes[n.id] = true;
      // Hit target first (underneath): transparent, at least ~16px on screen so
      // even tiny dots are easy to hover/touch. Screen size is zoom-invariant
      // because the whole view group is scaled by cam.k.
      var ht = hitEls[n.id];
      if (!ht) {
        ht = document.createElementNS("http://www.w3.org/2000/svg", "circle");
        ht.setAttribute("data-id", n.id);
        ht.setAttribute("fill", "transparent");
        ht.setAttribute("stroke", "none");
        ht.setAttribute("cursor", "pointer");
        hitEls[n.id] = ht;
        nodeLayer.appendChild(ht);
      }
      if (nodeDirty) {
        ht.setAttribute("cx", n.x); ht.setAttribute("cy", n.y);
        ht.setAttribute("opacity", n.fade === undefined ? 1 : n.fade);
      }

      var c = nodeEls[n.id];
      if (!c) {
        c = document.createElementNS("http://www.w3.org/2000/svg", "circle");
        c.setAttribute("data-id", n.id);
        c.setAttribute("stroke-width", 2);
        c.setAttribute("pointer-events", "none");
        nodeEls[n.id] = c;
        nodeLayer.appendChild(c);
      }
      var r = radius(n) * (1 + (n.pulse || 0) * 0.5);
      // Dots never shrink below ~5px on screen (so every node stays reachable
      // when zoomed out) and never exceed ~120px on screen (so a heavily
      // visited node can not cover the view, even fully zoomed in).
      var visR = Math.min(Math.max(r, 5 / cam.k), 120 / cam.k);
      if (nodeDirty) { c.setAttribute("cx", n.x); c.setAttribute("cy", n.y); }
      if (nodeDirty || camDirty) { c.setAttribute("r", visR); ht.setAttribute("r", Math.max(18 / cam.k, 0)); }
      if (nodeDirty || camDirty) { c.setAttribute("fill", color(n)); }
      if (focusDirty || nodeDirty) {
        var isSel = selected && selected.id === n.id;
        var isHov = hoveredId === n.id;
        c.setAttribute("stroke", isSel ? "#f0f6fc" : isHov ? "#ffd33d" : "#30363d");
        c.setAttribute("stroke-width", isSel || isHov ? 3 : 2);
      }
      if (nodeDirty) { c.setAttribute("opacity", n.fade === undefined ? 1 : n.fade); }
    }
    for (var id in nodeEls) if (!usedNodes[id]) { nodeEls[id].remove(); delete nodeEls[id]; }
    for (var id in hitEls) if (!usedNodes[id]) { hitEls[id].remove(); delete hitEls[id]; }
    nodeDirty = false;
    focusDirty = false;
  }
  function fmtTs(ts) {
    var d = new Date(ts * 1000);
    return d.toISOString().replace("T", " ").slice(0, 19) + "Z";
  }
  function esc(s) {
    return String(s).replace(/[&<>"]/g, function (ch) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[ch];
    });
  }
  function fillPanel(n) {
    panel.innerHTML =
      '<h2>' + esc(n.title) + '</h2>' +
      '<div><span class="badge">' + esc(n.type) + '</span>' +
      '<span class="badge ns">' + esc(n.ns) + '</span></div>' +
      (n.content ? '<p class="content">' + esc(n.content) + '</p>' : "") +
      '<p><b>importance</b> ' + n.imp + ' &nbsp; <b>accesses</b> ' + n.acc + '</p>' +
      '<p><b>created</b> ' + fmtTs(n.created) + '</p>' +
      '<p><b>updated</b> ' + fmtTs(n.updated) + '</p>' +
      (n.entities.length ? '<p><b>entities</b> ' + n.entities.map(esc).join(", ") + '</p>' : "");
  }
  function showPanel(n) {
    panel.style.display = "block";
    fillPanel(n);
  }
  function hidePanel() {
    panel.style.display = "none";
  }
  function select(n) {
    selected = n;
    showPanel(n);
  }
  function refresh() {
    fetch("/data").then(function (r) { return r.json(); }).then(setData);
  }

  if (typeof GRAPH_DATA !== "undefined") {
    setData(GRAPH_DATA);
  } else {
    refresh();
    var es = new EventSource("/events");
    es.onmessage = function (e) { applyEvent(JSON.parse(e.data)); };
  }
  requestAnimationFrame(tick);
})();
"##;

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
                  transition: background .12s; }
  #legend .lrow:hover { background: #21262d; text-decoration: underline; }
  #legend .lrow.pin { background: #21262d; box-shadow: inset 0 0 0 1px #8b949e; }
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
                  transition: background .12s; }
   #legend .lrow:hover { background: #21262d; text-decoration: underline; }
   #legend .lrow.pin { background: #21262d; box-shadow: inset 0 0 0 1px #8b949e; }
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
        // instead of throwing on undefined layer nodes.
        let html = live_html();
        let draw_start = "function draw() {";
        let idx = html.find(draw_start).expect("draw fn");
        let tail = &html[idx..];
        let draw_body = &tail[..tail.find("function fmtTs").expect("fmtTs fn")];
        assert!(
            draw_body.contains("if (!linkLayer || !nodeLayer) return;"),
            "draw() must guard against layers that are not built yet: {draw_body}"
        );
    }
}
