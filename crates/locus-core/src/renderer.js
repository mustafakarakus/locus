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
  var svg, view, hullLayer, linkLayer, nodeLayer, labelLayer;
  var nodeEls = {}, hitEls = {}, hullEls = {}, labelEls = {};
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
  // decluster runs a bounded position-based relaxation over just the pinned
  // cloud's nodes once the camera arrives, pushing overlapping dots apart so
  // every memory is visible and hoverable. It never touches other clouds and
  // never re-runs the global force sim, so a heavy graph stays cheap. The
  // gentle centroid pull keeps the cloud from drifting while spreading.
  var decluster = null, declusteredNs = null;
  // savedCloud remembers the pinned cloud's pre-stage/pre-spread home positions
  // so unpin restores the overview constellation without a blown-up cloud.
  var savedCloud = null;
  // Target on-screen gap between adjacent dot edges (in CSS pixels) after a
  // pinned cloud finishes spreading. The spread is re-fitted to the grown
  // bounds, so this is the visible separation the user actually sees.
  var DECLUSTER_GAP = 24;
  // World-space gap between other clouds and the staged (pinned) cloud so the
  // expanded detail has empty room instead of expanding on top of the pile.
  var STAGE_GAP = 280;
  // Non-pinned clouds dim to this opacity while a namespace is staged.
  // Kept high enough that landmarks stay readable, low enough that the
  // staged cloud clearly owns attention (was 0.18 — too ghostly/bright).
  var DIM_OPACITY = 0.42;
  // Nodes are translucent (not solid stickers) so the sphere stays soft.
  var NODE_FILL_OPACITY = 0.72;
  var pinChip = null;
  var drag = null, dragMoved = false;
  // Incremental render flags. Links and nodes are heavy (57k+ links, 2k dots on
  // a big graph), so draw() only touches the DOM when something actually moved:
  // linkDirty/nodeDirty while the force sim runs, camDirty while the camera
  // eases, and focusDirty when the hovered/pinned node changes.
  // posDirty = sphere orbit only: update cx/cy/opacity, never re-sort or
  // re-append the DOM (that was making idle spin crawl).
  var linkDirty = true, nodeDirty = true, camDirty = true, focusDirty = true, hullDirty = true;
  var posDirty = false;
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
  // Free-layout forces only run for a pinned/staged cloud. Overview is a
  // fixed sphere packing (Fibonacci lattice → 2D projection) — never
  // re-simulated — so hubs cannot collapse each namespace into a camp.
  var REPULSE_ACCEL = 0.9;
  var REPULSE_DIST = 220;
  // World scale of the projected sphere (unit sphere × this).
  var overviewRingR = 320;
  // Sphere orientation (radians). Drag on empty overview canvas orbits;
  // a gentle idle spin runs until the user takes over.
  var sphereRotY = 0;
  var sphereRotX = 0.35;
  var SPHERE_SPIN = 0.12;
  // Radians per CSS pixel of drag — tuned so a half-swipe turns ~half a turn.
  var SPHERE_DRAG = 0.008;

  // Absolute, per-visit dot growth: base radius with a logarithmic ramp so a
  // node keeps getting bigger every time it is visited, but never beyond
  // MAX_DOT_RADIUS. Growth is independent of the busiest node, so one very hot
  // node can not shrink its neighbours. The draw() screen clamp guarantees a
  // heavily visited node can never cover the view even fully zoomed in.
  // Tuned so cold (0) / warm (~20) / hot (~200+) are clearly different sizes
  // on the overview sphere without hot dots swallowing the globe.
  var BASE_DOT_RADIUS = 7;
  var DOT_GROWTH = 14;
  var MAX_DOT_RADIUS = 48;
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
    // Clear, readable dots on dark bg — saturated enough to tell namespaces
    // apart, not so neon that the disk glows.
    var sat = Math.round(62 - 20 * t);
    var lit = Math.round(56 + 10 * t);
    return "hsl(" + hue + "," + sat + "%," + lit + "%)";
  }
  function nsColor(ns) {
    var hue = nsHue(ns);
    if (hue < 0) return "hsl(0,0%,62%)";
    return "hsl(" + hue + ",58%,55%)";
  }
  function buildLegend() {
    if (!legendEl) return;
    var seen = {};
    var counts = {};
    for (var i = 0; i < nodes.length; i++) {
      var ns = nodes[i].ns || "global";
      seen[ns] = true;
      counts[ns] = (counts[ns] || 0) + 1;
    }
    var lines = [];
    Object.keys(seen).sort().forEach(function (ns) {
      lines.push('<div class="lrow" data-ns="' + esc(ns) + '"><span class="dot" style="background:' +
        nsColor(ns) + '"></span>' + esc(ns) +
        '<span class="ncount">' + (counts[ns] || 0) + '</span></div>');
    });
    lines.push('<div class="hint">node size = retrieval frequency</div>');
    lines.push('<div class="hint">brighter = more recently touched</div>');
    lines.push('<div class="hint">drag to rotate · click a node to stage</div>');
    legendEl.innerHTML = lines.join("");
    ensurePinChip();
    updatePinChrome();
  }
  function ensurePinChip() {
    if (pinChip) return;
    pinChip = document.getElementById("pinchip");
    if (!pinChip) {
      pinChip = document.createElement("div");
      pinChip.id = "pinchip";
      document.body.appendChild(pinChip);
    }
    pinChip.addEventListener("click", function () {
      if (pinnedNs) unpinNamespace();
    });
  }
  function updatePinChrome() {
    ensurePinChip();
    if (!pinChip) return;
    if (!pinnedNs) {
      pinChip.style.display = "none";
      pinChip.textContent = "";
      return;
    }
    var n = 0;
    for (var i = 0; i < nodes.length; i++) if (nodes[i].ns === pinnedNs) n++;
    pinChip.style.display = "block";
    pinChip.innerHTML = "Pinned <b>" + esc(pinnedNs) + "</b> · " + n +
      " memories <span class=\"unpin\">click to unpin</span>";
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
  function nsCentroid(ns) {
    var sx = 0, sy = 0, c = 0;
    for (var i = 0; i < nodes.length; i++) {
      if (nodes[i].ns !== ns) continue;
      sx += nodes[i].x; sy += nodes[i].y; c++;
    }
    return c ? { x: sx / c, y: sy / c, n: c } : null;
  }
  function fitToBounds(b, pad) {
    if (!b) return null;
    var bw = Math.max(1, b.maxX - b.minX), bh = Math.max(1, b.maxY - b.minY);
    var k = Math.max(0.05, Math.min(3, Math.min((W - pad * 2) / bw, (H - pad * 2) / bh) * 0.9));
    // Stage a pinned cloud slightly left of center so the detail panel and
    // legend don't fight the expanded cloud for the same screen real estate.
    var cx = pinnedNs ? W * 0.42 : W / 2;
    return { x: cx - (b.minX + b.maxX) / 2 * k, y: H / 2 - (b.minY + b.maxY) / 2 * k, k: k };
  }
  function fitCamera() {
    // Extra pad so overview clouds sit inside the viewport with breathing room
    // instead of edge-to-edge after fit.
    return fitToBounds(nsBounds(), 100);
  }
  function focusNamespace(ns) {
    var b = nsBounds(ns);
    var t = fitToBounds(b, 90);
    if (t) camTarget = t;
  }
  function listNsNodes(ns) {
    var list = [];
    for (var i = 0; i < nodes.length; i++) {
      if (nodes[i].ns === ns) list.push(nodes[i]);
    }
    return list;
  }
  function saveCloudHome(ns) {
    var list = listNsNodes(ns);
    savedCloud = { ns: ns, pos: {} };
    for (var i = 0; i < list.length; i++) {
      savedCloud.pos[list[i].id] = { x: list[i].x, y: list[i].y };
    }
  }
  // Slide the pinned cloud into empty world space so decluster expands into
  // clear stage room instead of on top of the overview constellation.
  function stageCloud(ns) {
    var list = listNsNodes(ns);
    if (!list.length) return;
    var c = nsCentroid(ns);
    if (!c) return;
    // Place the stage to the right of every other cloud's bounds (bounds
    // computed excluding the pinned namespace below).
    var minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    var any = false;
    for (var i = 0; i < nodes.length; i++) {
      if (nodes[i].ns === ns) continue;
      any = true;
      if (nodes[i].x < minX) minX = nodes[i].x;
      if (nodes[i].x > maxX) maxX = nodes[i].x;
      if (nodes[i].y < minY) minY = nodes[i].y;
      if (nodes[i].y > maxY) maxY = nodes[i].y;
    }
    var stageX, stageY;
    if (any) {
      stageX = maxX + STAGE_GAP + Math.sqrt(list.length) * 8;
      stageY = (minY + maxY) / 2;
    } else {
      stageX = c.x + STAGE_GAP;
      stageY = c.y;
    }
    var dx = stageX - c.x, dy = stageY - c.y;
    if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return;
    for (var i = 0; i < list.length; i++) {
      list[i].x += dx;
      list[i].y += dy;
    }
    linkDirty = true;
    nodeDirty = true;
    hullDirty = true;
  }
  // Bounded spread of a pinned cloud so every dot is visible and hoverable.
  // Only the pinned namespace's nodes move; other clouds and the global layout
  // are untouched. The pass runs a synchronous fixed-point solve: it pushes
  // overlapping pairs apart toward a target gap that is recomputed from the fit
  // zoom of the *current* bounds each round, so the on-screen separation after
  // the final re-fit is bounded below by DECLUSTER_GAP regardless of how much
  // the cloud grows. Because it converges inside a few frames instead of
  // spreading work across rAF, a heavy graph never re-enters the O(n^2) global
  // sim and the user sees the result immediately.
  //
  // NOTE: bounded to 80 rounds x O(list.length^2) per round, run synchronously
  // on the calling (click/legend) event. This is fine for typical per-project
  // namespace sizes (tens to low hundreds of memories); if a single namespace
  // can grow into the thousands, this pass should move off the main thread or
  // gain an explicit list.length cap.
  function startDecluster(ns) {
    if (decluster || declusteredNs === ns) return;
    var list = listNsNodes(ns);
    if (list.length < 2) {
      declusteredNs = ns;
      focusNamespace(ns);
      return;
    }
    // Home positions are saved by pinNamespace before staging; only fall back
    // to a fresh snapshot if nothing was saved (e.g. camera-arrive path).
    if (!savedCloud || savedCloud.ns !== ns) saveCloudHome(ns);
    decluster = { ns: ns, nodes: list };
    // Run to convergence immediately. Bounded to a fixed round count so a huge
    // cloud can never stall the frame; anything left overlapping after the
    // budget is far more spread than before and the re-fit keeps it readable.
    for (var r = 0; r < 80; r++) {
      var b = nsBounds(ns);
      var fit = b ? fitToBounds(b, 60) : null;
      var k = fit && fit.k > 0 ? fit.k : cam.k;
      var gap = DECLUSTER_GAP / k;
      var moved = false;
      for (var i = 0; i < list.length; i++) {
        var a = list[i];
        for (var j = i + 1; j < list.length; j++) {
          var b2 = list[j];
          var minD = (radius(a) + radius(b2)) / k + gap;
          var dx = a.x - b2.x, dy = a.y - b2.y;
          var d2 = dx * dx + dy * dy;
          if (d2 >= minD * minD || d2 === 0) continue;
          var d = Math.sqrt(d2);
          var push = (minD - d) * 0.5;
          var ux = dx / d, uy = dy / d;
          a.x += ux * push * 0.5;
          a.y += uy * push * 0.5;
          b2.x -= ux * push * 0.5;
          b2.y -= uy * push * 0.5;
          moved = true;
        }
      }
      if (!moved) break;
    }
    declusteredNs = ns;
    decluster = null;
    linkDirty = true;
    nodeDirty = true;
    hullDirty = true;
    focusNamespace(ns);
  }
  // Galaxy pin: save home positions, slide the cloud into empty stage space,
  // decluster for readability, then ease the camera onto the staged cloud.
  function pinNamespace(ns) {
    collapseCloud();
    pinnedNs = ns;
    declusteredNs = null;
    decluster = null;
    saveCloudHome(ns);
    stageCloud(ns);
    startDecluster(ns);
    autoFit = false;
    userControlled = false;
    updatePinChrome();
    updateLegendPins();
    linkDirty = true;
    nodeDirty = true;
    hullDirty = true;
    focusDirty = true;
  }
  function unpinNamespace() {
    pinnedNs = null;
    decluster = null;
    declusteredNs = null;
    collapseCloud();
    // Prefer polar homes over saved xy in case of any drift.
    lockOverviewHomes();
    settled = true;
    camTarget = fitCamera();
    autoFit = false;
    userControlled = false;
    updatePinChrome();
    updateLegendPins();
    linkDirty = true;
    nodeDirty = true;
    hullDirty = true;
    focusDirty = true;
  }
  function updateLegendPins() {
    if (!legendEl) return;
    for (var i = 0; i < legendEl.children.length; i++) {
      var ch = legendEl.children[i];
      if (!ch.classList || !ch.classList.contains("lrow")) continue;
      ch.classList.toggle("pin", !!pinnedNs && ch.getAttribute("data-ns") === pinnedNs);
    }
  }
  // Restore the previously staged/spread cloud to its overview home positions
  // so the constellation fit never has to accommodate a blown-up cloud.
  function collapseCloud() {
    if (!savedCloud) return;
    var pos = savedCloud.pos;
    for (var i = 0; i < nodes.length; i++) {
      var p = pos[nodes[i].id];
      if (p) { nodes[i].x = p.x; nodes[i].y = p.y; }
    }
    savedCloud = null;
    linkDirty = true;
    nodeDirty = true;
    hullDirty = true;
  }
  function fit() {
    var t = fitCamera();
    if (t) { cam.x = t.x; cam.y = t.y; cam.k = t.k; }
  }
  function buildSvg() {
    svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    view = document.createElementNS("http://www.w3.org/2000/svg", "g");
    // Layer order: hulls (under) → links → nodes → labels (over).
    hullLayer = document.createElementNS("http://www.w3.org/2000/svg", "g");
    linkLayer = document.createElementNS("http://www.w3.org/2000/svg", "g");
    nodeLayer = document.createElementNS("http://www.w3.org/2000/svg", "g");
    labelLayer = document.createElementNS("http://www.w3.org/2000/svg", "g");
    nodeEls = {};
    hitEls = {};
    hullEls = {};
    labelEls = {};
    // All links render as one <path> (a single d attribute instead of one
    // <line> element per edge) so large graphs stay cheap to draw. A second
    // path holds the connections of the hovered/pinned node on top.
    linkPath = document.createElementNS("http://www.w3.org/2000/svg", "path");
    linkPath.setAttribute("fill", "none");
    linkPath.setAttribute("stroke", "#1a1f26");
    linkPath.setAttribute("stroke-width", 1);
    linkPath.setAttribute("stroke-opacity", "0.85");
    activePath = document.createElementNS("http://www.w3.org/2000/svg", "path");
    activePath.setAttribute("fill", "none");
    activePath.setAttribute("stroke", "#8b949e");
    activePath.setAttribute("stroke-width", 2);
    linkLayer.appendChild(linkPath);
    linkLayer.appendChild(activePath);
    svg.appendChild(view);
    view.appendChild(hullLayer);
    view.appendChild(linkLayer);
    view.appendChild(nodeLayer);
    view.appendChild(labelLayer);
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
    hullDirty = true;
    nodeLayer.addEventListener("click", function (e) {
      if (dragMoved) return;
      var c = e.target.closest && e.target.closest("circle");
      if (!c) return;
      var n = nodeMap[c.getAttribute("data-id")];
      if (!n) return;
      e.stopPropagation();
      // Clicking a memory stages its namespace: the cloud moves to the detail
      // stage, gets a hull, and other clouds dim. Legend pin does the same.
      if (n.ns && pinnedNs !== n.ns) {
        pinNamespace(n.ns);
      }
      if (selected && selected.id === n.id) {
        // Clicking the selected memory again clears the memory pin (panel),
        // not the namespace stage — empty-canvas click unpins the project.
        selected = null;
        focusDirty = true;
        hidePanel();
      } else {
        select(n);
      }
    });
    // Click empty canvas (not a node) unpins the staged project and clears
    // the memory selection — same affordance as many graph UIs.
    svg.addEventListener("click", function (e) {
      if (dragMoved) return;
      var c = e.target.closest && e.target.closest("circle[data-id]");
      if (c) return;
      if (pinnedNs) unpinNamespace();
      if (selected) {
        selected = null;
        focusDirty = true;
        hidePanel();
      }
    });
    // Hover reveals the panel immediately; clicking selects it (the panel stays
    // open even after the pointer leaves) and stages its namespace.
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
    // Drag works even when the pointer starts on a node: the sphere is fully
    // covered by hit-target circles, so ignoring those targets made orbit
    // impossible. Click-without-move still selects via the click handler
    // (which checks dragMoved).
    svg.addEventListener("pointerdown", function (e) {
      if (e.button !== undefined && e.button !== 0) return;
      dragMoved = false;
      camTarget = null;
      decluster = null;
      drag = {
        x: e.clientX,
        y: e.clientY,
        camX: cam.x,
        camY: cam.y,
        rotY: sphereRotY,
        rotX: sphereRotX,
        // Overview → orbit globe; pinned detail → pan camera.
        mode: pinnedNs ? "pan" : "orbit"
      };
      try { svg.setPointerCapture(e.pointerId); } catch (err) { /* older browsers */ }
    });
    svg.addEventListener("pointermove", function (e) {
      if (!drag) return;
      var dx = e.clientX - drag.x, dy = e.clientY - drag.y;
      if (!dragMoved && Math.abs(dx) + Math.abs(dy) < 4) return;
      dragMoved = true;
      if (drag.mode === "orbit") {
        // Horizontal drag → yaw, vertical → pitch (clamped so the globe
        // doesn't flip upside-down).
        sphereRotY = drag.rotY + dx * SPHERE_DRAG;
        sphereRotX = Math.max(-1.2, Math.min(1.2, drag.rotX + dy * SPHERE_DRAG));
        lockOverviewHomes();
        posDirty = true;
        userControlled = true;
        autoFit = false;
      } else {
        cam.x = drag.camX + dx;
        cam.y = drag.camY + dy;
        camDirty = true;
        userControlled = true;
        autoFit = false;
      }
    });
    function endDrag(e) {
      drag = null;
      if (e && e.pointerId !== undefined) {
        try { svg.releasePointerCapture(e.pointerId); } catch (err) { /* not capturing */ }
      }
    }
    svg.addEventListener("pointerup", endDrag);
    svg.addEventListener("pointercancel", endDrag);
    svg.addEventListener("lostpointercapture", function () { drag = null; });
    svg.addEventListener("wheel", function (e) {
      e.preventDefault();
      camTarget = null;
      decluster = null;
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
          unpinNamespace();
        } else {
          pinNamespace(ns);
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
    // Fresh data replaces the layout; drop any staged pin so overview is clean.
    pinnedNs = null;
    decluster = null;
    declusteredNs = null;
    savedCloud = null;
    buildSvg();
    scatter();
    fit();
    autoFit = true;
    settled = false;
    stillFrames = 0;
    simStart = Date.now();
    buildNsHues();
    buildLegend();
    hullDirty = true;
  }
  function scatter() {
    // Overview = ONE sphere. Points are a Fibonacci lattice on the unit
    // sphere, then sliced into longitude bands per namespace so colors form
    // contiguous regions on a single globe (not flat pie wedges / camps).
    var names = [];
    var byNs = {};
    for (var i = 0; i < nodes.length; i++) {
      var ns = nodes[i].ns || "global";
      if (!byNs[ns]) { names.push(ns); byNs[ns] = []; }
      byNs[ns].push(nodes[i]);
    }
    names.sort();
    var ordered = [];
    for (var s = 0; s < names.length; s++) {
      var list = byNs[names[s]];
      list.sort(function (a, b) { return a.id < b.id ? -1 : a.id > b.id ? 1 : 0; });
      for (var i = 0; i < list.length; i++) ordered.push(list[i]);
    }
    var total = Math.max(1, ordered.length);
    overviewRingR = 200 + Math.sqrt(total) * 11;
    sphereRotY = 0;
    sphereRotX = 0.35;
    // Fibonacci sphere (even surface distribution).
    var pts = [];
    var golden = Math.PI * (3 - Math.sqrt(5));
    for (var i = 0; i < total; i++) {
      var fy = total === 1 ? 0 : 1 - (i / (total - 1)) * 2;
      var fr = Math.sqrt(Math.max(0, 1 - fy * fy));
      var theta = golden * i;
      var fx = Math.cos(theta) * fr;
      var fz = Math.sin(theta) * fr;
      pts.push({ x: fx, y: fy, z: fz, lon: Math.atan2(fz, fx) });
    }
    // Sort by longitude so namespace bands wrap the globe as orange slices.
    pts.sort(function (a, b) { return a.lon - b.lon; });
    for (var i = 0; i < ordered.length; i++) {
      var n = ordered[i];
      var p = pts[i];
      n.sx = p.x;
      n.sy = p.y;
      n.sz = p.z;
      n.vx = 0;
      n.vy = 0;
      n.fade = 1;
      n.fresh = false;
    }
    lockOverviewHomes();
  }
  // Project unit-sphere homes → 2D with current orbit (pitch X, yaw Y).
  // Depth is stored for size/opacity cues so the silhouette reads as a sphere.
  // Cos/sin are cached on the module locals and refreshed by lockOverviewHomes.
  var _cy = 1, _sy = 0, _cx = 1, _sx = 0;
  function projectSphereNode(n) {
    var x0 = n.sx, y0 = n.sy, z0 = n.sz;
    var x1 = x0 * _cy + z0 * _sy;
    var z1 = -x0 * _sy + z0 * _cy;
    var y2 = y0 * _cx - z1 * _sx;
    var z2 = y0 * _sx + z1 * _cx;
    var persp = 1 / (1.55 - z2 * 0.42);
    var s = overviewRingR * persp;
    n.x = x1 * s;
    n.y = y2 * s;
    n.depth = z2;
  }
  // Snap every non-pinned node to its sphere projection.
  function lockOverviewHomes() {
    _cy = Math.cos(sphereRotY); _sy = Math.sin(sphereRotY);
    _cx = Math.cos(sphereRotX); _sx = Math.sin(sphereRotX);
    for (var i = 0; i < nodes.length; i++) {
      var n = nodes[i];
      if (pinnedNs && n.ns === pinnedNs) continue;
      if (n.sx === undefined) continue;
      projectSphereNode(n);
      n.vx = 0;
      n.vy = 0;
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
    // Overview: sphere projection (optional idle spin). No force sim — that
    // was what collapsed each namespace into a separate camp. Pin: free layout
    // only for the staged cloud; other projects stay on the globe.
    if (!pinnedNs) {
      var nowSpin = Date.now();
      var dtSpin = Math.min(0.1, (nowSpin - lastTick) / 1000);
      // Gentle idle rotation when the user isn't dragging/zooming.
      // Only mark posDirty (cheap cx/cy updates) — never nodeDirty, which
      // used to re-sort + re-append every SVG circle every frame.
      if (!userControlled && !drag) {
        sphereRotY += dtSpin * SPHERE_SPIN;
        lockOverviewHomes();
        posDirty = true;
      } else if (!settled) {
        lockOverviewHomes();
      }
      if (!settled) {
        for (var i = 0; i < nodes.length; i++) nodes[i].fade = 1;
        settled = true;
        linkDirty = true;
        nodeDirty = true;
        hullDirty = true;
      }
    } else if (!settled) {
      var elapsed = simStart ? (Date.now() - simStart) : 0;
      var cool = elapsed < SIM_MS ? 1 - elapsed / SIM_MS : 0;
      // Only the pinned cloud participates in free force layout.
      var pinned = [];
      for (var i = 0; i < nodes.length; i++) {
        if (nodes[i].ns === pinnedNs) pinned.push(nodes[i]);
      }
      for (var i = 0; i < pinned.length; i++) {
        var a = pinned[i];
        for (var j = i + 1; j < pinned.length; j++) {
          var b = pinned[j];
          var dx = a.x - b.x, dy = a.y - b.y;
          var d2 = dx * dx + dy * dy + 1;
          if (d2 > REPULSE_DIST * REPULSE_DIST) continue;
          var d = Math.sqrt(d2);
          var f = Math.min(2600 / d2, REPULSE_ACCEL / d) * cool;
          a.vx += f * dx; a.vy += f * dy;
          b.vx -= f * dx; b.vy -= f * dy;
        }
      }
      for (var k = 0; k < edges.length; k++) {
        var e = edges[k];
        var a = nodes[e.source], b = nodes[e.target];
        if (!a || !b || a.ns !== pinnedNs || b.ns !== pinnedNs) continue;
        var dx = b.x - a.x, dy = b.y - a.y;
        var d = Math.sqrt(dx * dx + dy * dy) || 1;
        var f = 0.012 * (d - 110) / d * cool;
        a.vx += f * dx; a.vy += f * dy;
        b.vx -= f * dx; b.vy -= f * dy;
      }
      for (var i = 0; i < nodes.length; i++) {
        var n = nodes[i];
        if (n.ns === pinnedNs) {
          n.x += n.vx; n.y += n.vy;
          n.vx *= 0.82; n.vy *= 0.82;
        } else if (n.sx !== undefined) {
          projectSphereNode(n);
          n.vx = 0; n.vy = 0;
        }
        if (n.fade < 1) n.fade = Math.min(1, n.fade + 0.05);
        if (n.pulse > 0) n.pulse *= 0.92;
      }
      var maxV = 0;
      for (var i = 0; i < pinned.length; i++) {
        var n = pinned[i];
        var v = Math.abs(n.vx) > Math.abs(n.vy) ? Math.abs(n.vx) : Math.abs(n.vy);
        if (v > maxV) maxV = v;
      }
      if (maxV < SETTLE_EPS || elapsed >= SIM_MS) {
        if (++stillFrames >= SETTLE_FRAMES || elapsed >= SIM_MS) {
          for (var i = 0; i < pinned.length; i++) { pinned[i].vx = 0; pinned[i].vy = 0; }
          settled = true;
        }
      } else {
        stillFrames = 0;
      }
      linkDirty = true;
      nodeDirty = true;
    } else if (pinnedNs) {
      // Keep non-pinned projects glued to the sphere while the staged cloud is up.
      lockOverviewHomes();
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
        if (pinnedNs) startDecluster(pinnedNs);
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
    // Snapshot camDirty before clearing — radius/label sizing still need it.
    var camChanged = camDirty;
    if (camDirty) {
      view.setAttribute("transform",
        "translate(" + cam.x.toFixed(1) + "," + cam.y.toFixed(1) + ") scale(" + cam.k.toFixed(4) + ")");
      camDirty = false;
    }
    var focus = hoveredId || (selected ? selected.id : null);
    var posChanged = linkDirty;
    var focusChanged = focusDirty;
    // Links: one <path> for edges. Overview keeps the mesh quiet (subsampled +
    // dark stroke). While a namespace is pinned, only that cloud's edges draw
    // at full density so the stage stays readable and cheap.
    if (linkDirty) {
      var d = "";
      // Overview: hide the mesh. Within-ns hub edges were drawing dark fans
      // that made each color look like a separate camp. Edges return on pin.
      if (pinnedNs) {
        for (var k = 0; k < edges.length; k++) {
          var e = edges[k];
          var a = nodes[e.source], b = nodes[e.target];
          if (!a || !b) continue;
          if (a.ns !== pinnedNs || b.ns !== pinnedNs) continue;
          d += "M" + a.x.toFixed(1) + " " + a.y.toFixed(1) + "L" + b.x.toFixed(1) + " " + b.y.toFixed(1);
        }
        linkPath.setAttribute("stroke", "#30363d");
        linkPath.setAttribute("stroke-opacity", "0.9");
      }
      linkPath.setAttribute("d", d);
      linkDirty = false;
    }
    // Focus links: the connections of the hovered/pinned node, drawn on top.
    // Rebuilt when focus changes or when the layout moved (node positions).
    if (focusChanged || posChanged) {
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
    }
    // Hull + label only for the staged (pinned) namespace. Overview stays a
    // clean multi-cloud scatter — no rings until the user pins a project by
    // clicking a node or a legend row.
    if (hullLayer && (hullDirty || posChanged || camChanged)) {
      var usedHull = {};
      if (pinnedNs) {
        var hcx = 0, hcy = 0, hn = 0, hr = 0;
        for (var i = 0; i < nodes.length; i++) {
          if (nodes[i].ns !== pinnedNs) continue;
          hcx += nodes[i].x; hcy += nodes[i].y; hn++;
        }
        if (hn > 0) {
          hcx /= hn; hcy /= hn;
          for (var i = 0; i < nodes.length; i++) {
            if (nodes[i].ns !== pinnedNs) continue;
            var dx = nodes[i].x - hcx, dy = nodes[i].y - hcy;
            var dist = Math.sqrt(dx * dx + dy * dy) + radius(nodes[i]);
            if (dist > hr) hr = dist;
          }
          hr += 28;
          usedHull[pinnedNs] = true;
          var hue = nsHue(pinnedNs);
          var hull = hullEls[pinnedNs];
          if (!hull) {
            hull = document.createElementNS("http://www.w3.org/2000/svg", "circle");
            hull.setAttribute("pointer-events", "none");
            hullEls[pinnedNs] = hull;
            hullLayer.appendChild(hull);
          }
          hull.setAttribute("cx", hcx);
          hull.setAttribute("cy", hcy);
          hull.setAttribute("r", hr);
          // Dark, solid stage ring — only visible while a project is pinned.
          if (hue < 0) {
            hull.setAttribute("fill", "rgba(22,27,34,0.72)");
            hull.setAttribute("stroke", "rgba(48,54,61,0.7)");
          } else {
            hull.setAttribute("fill", "hsla(" + hue + ",22%,14%,0.78)");
            hull.setAttribute("stroke", "hsla(" + hue + ",28%,32%,0.55)");
          }
          hull.setAttribute("stroke-width", 1.6 / Math.max(cam.k, 0.05));
          var lab = labelEls[pinnedNs];
          if (!lab) {
            lab = document.createElementNS("http://www.w3.org/2000/svg", "text");
            lab.setAttribute("text-anchor", "middle");
            lab.setAttribute("dominant-baseline", "middle");
            lab.setAttribute("pointer-events", "none");
            lab.setAttribute("font-family", "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace");
            labelEls[pinnedNs] = lab;
            labelLayer.appendChild(lab);
          }
          lab.textContent = pinnedNs;
          lab.setAttribute("x", hcx);
          lab.setAttribute("y", hcy - hr - 10 / Math.max(cam.k, 0.05));
          lab.setAttribute("font-size", Math.max(11, 13 / Math.max(cam.k, 0.05)));
          lab.setAttribute("fill", "#8b949e");
          lab.setAttribute("opacity", "0.88");
        }
      }
      for (var hid in hullEls) {
        if (!usedHull[hid]) { hullEls[hid].remove(); delete hullEls[hid]; }
      }
      for (var lid in labelEls) {
        if (!usedHull[lid]) { labelEls[lid].remove(); delete labelEls[lid]; }
      }
      hullDirty = false;
    }
    // Fast path: sphere orbit / idle spin only moves existing circles.
    // Avoids sort + appendChild of ~2k DOM nodes per frame (was the slowdown).
    if (posDirty && !nodeDirty) {
      for (var pi = 0; pi < nodes.length; pi++) {
        var pn = nodes[pi];
        if (pinnedNs && pn.ns === pinnedNs) continue;
        var pc = nodeEls[pn.id], pht = hitEls[pn.id];
        if (!pc) continue;
        var px = pn.x, py = pn.y;
        pc.setAttribute("cx", px); pc.setAttribute("cy", py);
        if (pht) { pht.setAttribute("cx", px); pht.setAttribute("cy", py); }
        var pdt = pn.depth === undefined ? 1 : (pn.depth + 1) / 2;
        var pop = (pn.fade === undefined ? 1 : pn.fade) *
          (0.55 + 0.45 * pdt) * NODE_FILL_OPACITY;
        if (selected && selected.id === pn.id) pop = Math.min(0.92, pop + 0.12);
        else if (hoveredId === pn.id) pop = Math.min(0.92, pop + 0.12);
        pc.setAttribute("opacity", pop);
        // Depth size cue without full style recompute.
        var pr = radius(pn) * (0.7 + 0.4 * pdt);
        var pvr = Math.min(Math.max(pr, 5 / cam.k), 120 / cam.k);
        pc.setAttribute("r", pvr);
      }
      posDirty = false;
      if (!focusDirty) {
        // Nothing else structural to do this frame.
        return;
      }
    }
    // Full path: create/update nodes (data load, pin, focus, camera zoom).
    var usedNodes = {};
    for (var di = 0; di < nodes.length; di++) {
      var n = nodes[di];
      usedNodes[n.id] = true;
      var isDim = pinnedNs && n.ns !== pinnedNs;
      var baseOp = n.fade === undefined ? 1 : n.fade;
      var depthT = n.depth === undefined ? 1 : (n.depth + 1) / 2;
      if (pinnedNs && n.ns === pinnedNs) depthT = 1;
      var op = (isDim ? baseOp * DIM_OPACITY : baseOp) *
        (0.55 + 0.45 * depthT) * NODE_FILL_OPACITY;
      var isSel = selected && selected.id === n.id;
      var isHov = hoveredId === n.id;
      if (isSel || isHov) op = Math.min(0.92, op + 0.12);
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
      var c = nodeEls[n.id];
      if (!c) {
        c = document.createElementNS("http://www.w3.org/2000/svg", "circle");
        c.setAttribute("data-id", n.id);
        c.setAttribute("stroke-width", 1.5);
        c.setAttribute("pointer-events", "none");
        nodeEls[n.id] = c;
        nodeLayer.appendChild(c);
      }
      var r = radius(n) * (1 + (n.pulse || 0) * 0.5) * (0.7 + 0.4 * depthT);
      var visR = Math.min(Math.max(r, 5 / cam.k), 120 / cam.k);
      if (nodeDirty || posDirty) {
        ht.setAttribute("cx", n.x); ht.setAttribute("cy", n.y);
        c.setAttribute("cx", n.x); c.setAttribute("cy", n.y);
      }
      if (nodeDirty || camChanged || posDirty) {
        c.setAttribute("r", visR);
        ht.setAttribute("r", Math.max(18 / cam.k, 0));
      }
      if (nodeDirty || camChanged || focusChanged) {
        if (isDim) {
          var hueD = nsHue(n.ns);
          c.setAttribute("fill", hueD < 0 ? "hsl(0,0%,32%)" : "hsl(" + hueD + ",22%,36%)");
        } else {
          c.setAttribute("fill", color(n));
        }
      }
      if (focusChanged || nodeDirty) {
        c.setAttribute("stroke", isSel ? "rgba(240,246,252,0.65)" : isHov ? "rgba(255,211,61,0.7)" : "rgba(48,54,61,0.4)");
        c.setAttribute("stroke-width", isSel || isHov ? 2 : 1.25);
      }
      if (nodeDirty || focusChanged || posDirty) { c.setAttribute("opacity", op); }
    }
    if (nodeDirty) {
      for (var id in nodeEls) if (!usedNodes[id]) { nodeEls[id].remove(); delete nodeEls[id]; }
      for (var id in hitEls) if (!usedNodes[id]) { hitEls[id].remove(); delete hitEls[id]; }
    }
    nodeDirty = false;
    posDirty = false;
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
