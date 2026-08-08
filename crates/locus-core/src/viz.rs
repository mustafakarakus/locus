//! Self-contained HTML rendering for the memory graph (U-016).
//!
//! The snapshot is a single offline file: all CSS and JS are inlined, graph
//! data is embedded as JSON, and no external resources are referenced. The live
//! page shares the same renderer but loads its initial data from `/data` and
//! updates over an SSE `/events` stream.
//!
//! Every rendered string (node titles, edge labels) is passed through the
//! secret scanner before serialization, so the rendered graph can never leak a
//! detected secret even if one was stored verbatim via `allow_secret`.

use crate::graph::{GraphData, GraphEdge, GraphNode};
use crate::Result;

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
pub fn graph_payload_json(data: &GraphData) -> Result<String> {
    let data = redacted_graph(data);
    let json = serde_json::to_string(&data)
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
  var nodeEls = {}, hitEls = {}, linkEls = {};
  var cam = { x: 0, y: 0, k: 1 };
  var autoFit = true, settleFrames = 0, userControlled = false;
  var drag = null, dragMoved = false;

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
      lines.push('<div><span class="dot" style="background:' + nsColor(ns) +
        '"></span>' + esc(ns) + '</div>');
    });
    lines.push('<div>node size = retrieval frequency</div>');
    lines.push('<div>brighter = more recently touched</div>');
    lines.push('<div>hover to inspect &middot; click to pin</div>');
    legendEl.innerHTML = lines.join("");
  }
  function fitCamera() {
    if (!nodes.length) return null;
    var minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (var i = 0; i < nodes.length; i++) {
      if (nodes[i].x < minX) minX = nodes[i].x;
      if (nodes[i].x > maxX) maxX = nodes[i].x;
      if (nodes[i].y < minY) minY = nodes[i].y;
      if (nodes[i].y > maxY) maxY = nodes[i].y;
    }
    var bw = Math.max(1, maxX - minX), bh = Math.max(1, maxY - minY);
    var pad = 80;
    var k = Math.max(0.05, Math.min(3, Math.min((W - pad * 2) / bw, (H - pad * 2) / bh) * 0.9));
    return { x: W / 2 - (minX + maxX) / 2 * k, y: H / 2 - (minY + maxY) / 2 * k, k: k };
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
    linkEls = {};
    svg.appendChild(view);
    view.appendChild(linkLayer);
    view.appendChild(nodeLayer);
    app.appendChild(svg);
    nodeLayer.addEventListener("click", function (e) {
      if (dragMoved) return;
      var c = e.target.closest && e.target.closest("circle");
      if (!c) return;
      var n = nodeMap[c.getAttribute("data-id")];
      if (!n) return;
      if (selected && selected.id === n.id) {
        // Clicking the pinned node again unpins it.
        selected = null;
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
        showPanel(n);
      }
    });
    nodeLayer.addEventListener("pointerout", function (e) {
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
      drag = { x: e.clientX, y: e.clientY, camX: cam.x, camY: cam.y };
      svg.setPointerCapture(e.pointerId);
    });
    svg.addEventListener("pointermove", function (e) {
      if (!drag) return;
      if (Math.abs(e.clientX - drag.x) + Math.abs(e.clientY - drag.y) > 3) dragMoved = true;
      cam.x = drag.camX + (e.clientX - drag.x);
      cam.y = drag.camY + (e.clientY - drag.y);
      userControlled = true;
      autoFit = false;
    });
    svg.addEventListener("pointerup", function () { drag = null; });
    svg.addEventListener("wheel", function (e) {
      e.preventDefault();
      var f = Math.exp(-e.deltaY * 0.0012);
      var wx = (e.clientX - cam.x) / cam.k;
      var wy = (e.clientY - cam.y) / cam.k;
      cam.k = Math.max(0.05, Math.min(20, cam.k * f));
      cam.x = e.clientX - wx * cam.k;
      cam.y = e.clientY - wy * cam.k;
      userControlled = true;
      autoFit = false;
    }, { passive: false });
    window.addEventListener("resize", function () {
      W = window.innerWidth;
      H = window.innerHeight;
      if (!userControlled) fit();
    });
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
    settleFrames = 0;
    buildNsHues();
    buildLegend();
  }
  function scatter() {
    var r = 180;
    for (var i = 0; i < nodes.length; i++) {
      var n = nodes[i];
      if (!n.fresh) continue;
      n.x = (Math.random() * 2 - 1) * r;
      n.y = (Math.random() * 2 - 1) * r;
    }
  }
  function applyEvent(ev) {
    var n = nodeMap[ev.memory_id];
    if (!n) { refresh(); return; }
    n.acc = (n.acc || 0) + (ev.access_delta || 0);
    if (n.acc > maxAccess) maxAccess = n.acc;
    n.updated = ev.timestamp;
    if (ev.access_delta) n.pulse = 1;
  }
  function tick() {
    for (var i = 0; i < nodes.length; i++) {
      var a = nodes[i];
      for (var j = i + 1; j < nodes.length; j++) {
        var b = nodes[j];
        var dx = a.x - b.x, dy = a.y - b.y;
        var d2 = dx * dx + dy * dy + 1;
        var f = 2600 / d2;
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
      var f = 0.012 * (d - 90) / d;
      a.vx += f * dx; a.vy += f * dy;
      b.vx -= f * dx; b.vy -= f * dy;
    }
    for (var i = 0; i < nodes.length; i++) {
      var n = nodes[i];
      n.vx += (0 - n.x) * 0.004;
      n.vy += (0 - n.y) * 0.004;
      n.x += n.vx; n.y += n.vy;
      n.vx *= 0.82; n.vy *= 0.82;
      if (n.fade < 1) n.fade = Math.min(1, n.fade + 0.05);
      if (n.pulse > 0) n.pulse *= 0.92;
    }
    if (autoFit) {
      var t = fitCamera();
      if (t) {
        settleFrames++;
        var ease = settleFrames < 60 ? 0.04 : 0.06;
        cam.k += (t.k - cam.k) * ease;
        cam.x += (t.x - cam.x) * ease;
        cam.y += (t.y - cam.y) * ease;
        if (settleFrames > 90 &&
            Math.abs(cam.k - t.k) < 0.0005 &&
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
    view.setAttribute("transform",
      "translate(" + cam.x.toFixed(1) + "," + cam.y.toFixed(1) + ") scale(" + cam.k.toFixed(4) + ")");
    var usedLinks = {};
    var usedNodes = {};
    for (var k = 0; k < edges.length; k++) {
      var e = edges[k];
      var a = nodeMap[e.source], b = nodeMap[e.target];
      if (!a || !b) continue;
      var key = e.source + "|" + e.target;
      usedLinks[key] = true;
      // Hover takes priority over the pinned node so hovering any node (even
      // after pinning one) reveals its connections.
      var focus = hoveredId || (selected ? selected.id : null);
      var active = focus && (e.source === focus || e.target === focus);
      var ln = linkEls[key];
      if (!ln) {
        ln = document.createElementNS("http://www.w3.org/2000/svg", "line");
        ln.setAttribute("stroke-width", 1);
        linkEls[key] = ln;
        linkLayer.appendChild(ln);
      }
      ln.setAttribute("x1", a.x); ln.setAttribute("y1", a.y);
      ln.setAttribute("x2", b.x); ln.setAttribute("y2", b.y);
      ln.setAttribute("stroke", active ? "#8b949e" : "#21262d");
      ln.setAttribute("stroke-width", active ? 2 : 1);
    }
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
      var hitR = Math.max(18 / cam.k, 0);
      ht.setAttribute("cx", n.x); ht.setAttribute("cy", n.y); ht.setAttribute("r", hitR);
      ht.setAttribute("opacity", n.fade === undefined ? 1 : n.fade);

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
      var isSel = selected && selected.id === n.id;
      var isHov = hoveredId === n.id;
      c.setAttribute("cx", n.x); c.setAttribute("cy", n.y); c.setAttribute("r", visR);
      c.setAttribute("fill", color(n));
      c.setAttribute("stroke", isSel ? "#f0f6fc" : isHov ? "#ffd33d" : "#30363d");
      c.setAttribute("stroke-width", isSel || isHov ? 3 : 2);
      c.setAttribute("opacity", n.fade === undefined ? 1 : n.fade);
    }
    for (var key in linkEls) if (!usedLinks[key]) { linkEls[key].remove(); delete linkEls[key]; }
    for (var id in nodeEls) if (!usedNodes[id]) { nodeEls[id].remove(); delete nodeEls[id]; }
    for (var id in hitEls) if (!usedNodes[id]) { hitEls[id].remove(); delete hitEls[id]; }
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
