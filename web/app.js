// Dependency Analyzer page. Talks to the plugin's code through
// window.openidea.postMessage / onMessage; every request carries a nonce and
// replies meant for another pom.xml's analyzer are ignored.
"use strict";

(() => {
  const bridge = window.openidea;
  const $ = (id) => document.getElementById(id);
  const left = $("left");
  const right = $("right");
  const statusEl = $("status");
  const search = $("search");

  if (!bridge) {
    document.body.textContent = "This page runs inside OpenIDEA.";
    return;
  }

  // ------------------------------------------------------------------ state
  const state = {
    graph: null, // { module, version, nodes: [{g,a,v,s,st,rv,p}] }
    children: [], // index -> child indexes
    artifacts: new Map(), // "g:a" -> { g, a, occ: [index] }
    conflicts: new Set(), // "g:a"
    mode: "conflicts",
    query: "",
    showGroup: false,
    showScope: true,
    selected: null, // list modes: "g:a"; tree mode: node index
    expanded: new Set(),
    rows: [], // visible rows of the left pane, in order: { key, index }
  };

  // --------------------------------------------------------------- messages
  const pageId = Math.random().toString(36).slice(2);
  let seq = 0;
  const pending = new Map();

  function request(op, extra) {
    const nonce = `${pageId}-${++seq}`;
    return new Promise((resolve, reject) => {
      pending.set(nonce, { resolve, reject });
      bridge.postMessage(JSON.stringify({ op, nonce, ...extra }));
      setTimeout(() => {
        if (pending.delete(nonce)) reject(new Error("The plugin did not answer."));
      }, 15000);
    });
  }

  bridge.onMessage((text) => {
    let msg;
    try {
      msg = JSON.parse(text);
    } catch {
      return;
    }
    const p = pending.get(msg.nonce);
    if (!p) return; // another analyzer's reply
    pending.delete(msg.nonce);
    if (msg.op === "error") p.reject(new Error(msg.message));
    else p.resolve(msg);
  });

  function setStatus(text, error = false) {
    statusEl.textContent = text;
    statusEl.classList.toggle("error", error);
  }

  // ------------------------------------------------------------------ model
  const keyOf = (n) => `${n.g}:${n.a}`;
  /** The version this occurrence asked for. */
  const requested = (n) => (n.st === "conflict" && n.rv ? n.rv : n.v);

  function index(graph) {
    const nodes = graph.nodes;
    state.children = nodes.map(() => []);
    state.artifacts = new Map();
    state.conflicts = new Set();
    nodes.forEach((n, i) => {
      if (n.p != null && state.children[n.p]) state.children[n.p].push(i);
      if (i === 0) return; // the module itself
      const k = keyOf(n);
      let art = state.artifacts.get(k);
      if (!art) state.artifacts.set(k, (art = { g: n.g, a: n.a, occ: [] }));
      art.occ.push(i);
      if (n.st === "conflict") state.conflicts.add(k);
    });
    // Occurrences asking for different versions are a conflict (a version set
    // by dependency management is not).
    for (const [k, art] of state.artifacts) {
      const versions = new Set(art.occ.map((i) => requested(nodes[i])).filter(Boolean));
      if (versions.size > 1) state.conflicts.add(k);
    }
  }

  function pathToRoot(i) {
    const nodes = state.graph.nodes;
    const out = [];
    for (let cur = i; cur != null && cur !== 0; cur = nodes[cur].p) out.push(cur);
    return out; // occurrence first, direct dependency last
  }

  function matches(n) {
    if (!state.query) return true;
    return `${n.g}:${n.a}:${n.v}`.toLowerCase().includes(state.query);
  }

  // ------------------------------------------------------------- rendering
  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function highlight(parent, text) {
    const q = state.query;
    const at = q ? text.toLowerCase().indexOf(q) : -1;
    if (at < 0) {
      parent.append(text);
      return;
    }
    parent.append(text.slice(0, at));
    parent.append(el("mark", null, text.slice(at, at + q.length)));
    parent.append(text.slice(at + q.length));
  }

  /** The label of one occurrence: [group:]artifact : version [scope] (note). */
  function nodeLabel(row, n) {
    if (state.showGroup) highlight(row.appendChild(el("span", "group")), `${n.g}:`);
    highlight(row.appendChild(el("span", "name")), n.a);
    const version = row.appendChild(el("span", "version"));
    if (n.st === "conflict" && n.rv) version.textContent = `: ${n.rv}`;
    else version.textContent = n.v ? `: ${n.v}` : "";
    if (state.showScope && n.s) row.appendChild(el("span", "scope", `[${n.s}]`));
    const note =
      n.st === "conflict"
        ? `omitted for conflict with ${n.v}`
        : n.st === "duplicate"
          ? "omitted for duplicate"
          : n.st === "managed"
            ? `version managed${n.rv ? ` from ${n.rv}` : ""}`
            : "";
    if (note) row.appendChild(el("span", "note", `(${note})`));
    row.classList.add(n.st);
  }

  /** The label of one artifact in the list modes, with all its versions. */
  function artifactLabel(row, art) {
    const nodes = state.graph.nodes;
    if (state.showGroup) highlight(row.appendChild(el("span", "group")), `${art.g}:`);
    highlight(row.appendChild(el("span", "name")), art.a);
    const winner = nodes[art.occ[0]].v;
    const asked = [...new Set(art.occ.map((i) => requested(nodes[i])))].filter((v) => v && v !== winner);
    row.appendChild(el("span", "version", winner ? `: ${winner}` : ""));
    if (asked.length) row.appendChild(el("span", "note", `(also requested ${asked.join(", ")})`));
    if (state.showScope) {
      const scopes = [...new Set(art.occ.map((i) => nodes[i].s).filter(Boolean))];
      if (scopes.length) row.appendChild(el("span", "scope", `[${scopes.join(", ")}]`));
    }
    if (state.conflicts.has(`${art.g}:${art.a}`)) row.classList.add("conflict");
  }

  function renderLeft() {
    left.replaceChildren();
    state.rows = [];
    if (!state.graph) return;
    const frag = document.createDocumentFragment();
    if (state.mode === "tree") renderTree(frag);
    else renderList(frag);
    if (!state.rows.length) {
      frag.appendChild(
        el(
          "div",
          "empty",
          state.query
            ? "Nothing matches the search."
            : state.mode === "conflicts"
              ? "No conflicts."
              : "No dependencies.",
        ),
      );
    }
    left.appendChild(frag);
    left.querySelector(".row.selected")?.scrollIntoView({ block: "nearest" });
  }

  function renderList(frag) {
    const nodes = state.graph.nodes;
    const keys = [...state.artifacts.keys()]
      .filter((k) => state.mode !== "conflicts" || state.conflicts.has(k))
      .filter((k) => state.artifacts.get(k).occ.some((i) => matches(nodes[i])))
      .sort((x, y) => {
        const a = state.artifacts.get(x);
        const b = state.artifacts.get(y);
        return a.a.localeCompare(b.a) || a.g.localeCompare(b.g);
      });
    for (const k of keys) {
      const row = el("div", "row");
      row.setAttribute("role", "treeitem");
      artifactLabel(row, state.artifacts.get(k));
      if (state.selected === k) row.classList.add("selected");
      row.addEventListener("click", () => select(k));
      row.addEventListener("dblclick", () => jumpToFirstPath(k));
      state.rows.push({ key: k });
      frag.appendChild(row);
    }
  }

  function renderTree(frag) {
    const nodes = state.graph.nodes;
    // With a search, show matches and their ancestors, expanded.
    let visible = null;
    if (state.query) {
      visible = new Set();
      nodes.forEach((n, i) => {
        if (i !== 0 && matches(n)) for (const j of pathToRoot(i)) visible.add(j);
      });
    }
    const walk = (i, depth) => {
      for (const c of state.children[i]) {
        if (visible && !visible.has(c)) continue;
        const n = nodes[c];
        const row = el("div", "row");
        row.setAttribute("role", "treeitem");
        row.style.paddingLeft = `${8 + depth * 16}px`;
        const hasKids = state.children[c].length > 0;
        const open = visible ? true : state.expanded.has(c);
        const tw = row.appendChild(el("span", "twisty", hasKids ? (open ? "▾" : "▸") : ""));
        if (hasKids) {
          row.setAttribute("aria-expanded", String(open));
          tw.addEventListener("click", (e) => {
            e.stopPropagation();
            toggle(c);
          });
        }
        nodeLabel(row, n);
        if (state.selected === c) row.classList.add("selected");
        row.addEventListener("click", () => select(c));
        row.addEventListener("dblclick", () => (hasKids ? toggle(c) : jumpToDirect(c)));
        state.rows.push({ index: c });
        frag.appendChild(row);
        if (hasKids && open) walk(c, depth + 1);
      }
    };
    walk(0, 0);
  }

  function renderRight() {
    right.replaceChildren();
    if (!state.graph || state.selected == null) {
      right.appendChild(el("div", "empty", "Select an artifact to see what brings it in."));
      return;
    }
    const nodes = state.graph.nodes;
    const k = typeof state.selected === "number" ? keyOf(nodes[state.selected]) : state.selected;
    const art = state.artifacts.get(k);
    if (!art) return;
    right.appendChild(el("div", "heading", `${art.occ.length} path${art.occ.length === 1 ? "" : "s"} to ${k}`));
    for (const occ of art.occ) {
      const path = pathToRoot(occ);
      const box = el("div", "path");
      path.forEach((i, depth) => {
        const row = el("div", "row");
        row.style.paddingLeft = `${8 + depth * 16}px`;
        nodeLabel(row, nodes[i]);
        box.appendChild(row);
      });
      const direct = nodes[path[path.length - 1]];
      const actions = el("div", "actions");
      if (path.length > 1) {
        const ex = actions.appendChild(el("button", null, `Exclude from ${direct.a}`));
        ex.title = `Add <exclusion> ${art.g}:${art.a} to ${direct.g}:${direct.a} in this pom.xml`;
        ex.addEventListener("click", () => exclude(direct, art));
      }
      const jump = actions.appendChild(el("button", null, path.length > 1 ? `Jump to ${direct.a}` : "Jump to declaration"));
      jump.addEventListener("click", () => jumpTo(direct));
      box.appendChild(actions);
      right.appendChild(box);
    }
  }

  function renderStatus() {
    if (!state.graph) return;
    const deps = state.artifacts.size;
    const c = state.conflicts.size;
    setStatus(
      `${state.graph.module}${state.graph.version ? ` ${state.graph.version}` : ""} · ${deps} artifact${deps === 1 ? "" : "s"} · ` +
        `${c} conflict${c === 1 ? "" : "s"} · as resolved at the last Maven reload`,
    );
  }

  function render() {
    $("expand").hidden = $("collapse").hidden = state.mode !== "tree";
    renderLeft();
    renderRight();
    renderStatus();
  }

  // ---------------------------------------------------------------- actions
  function select(sel) {
    state.selected = sel;
    renderLeft();
    renderRight();
  }

  function toggle(i) {
    if (state.expanded.has(i)) state.expanded.delete(i);
    else state.expanded.add(i);
    renderLeft();
  }

  async function load() {
    try {
      const graph = await request("load");
      state.graph = graph;
      index(graph);
      if (!state.expanded.size) for (const c of state.children[0] || []) state.expanded.add(c);
      if (typeof state.selected === "number" && state.selected >= graph.nodes.length) state.selected = null;
      render();
    } catch (e) {
      state.graph = null;
      left.replaceChildren(el("div", "empty", e.message));
      right.replaceChildren();
      setStatus(e.message, true);
    }
  }

  async function exclude(direct, art) {
    try {
      const r = await request("exclude", { dep: { g: direct.g, a: direct.a }, target: { g: art.g, a: art.a } });
      setStatus(r.message || "Excluded.");
    } catch (e) {
      setStatus(e.message, true);
    }
  }

  async function jumpTo(direct) {
    try {
      await request("open", { g: direct.g, a: direct.a });
    } catch (e) {
      setStatus(e.message, true);
    }
  }

  function jumpToDirect(i) {
    const path = pathToRoot(i);
    if (path.length) jumpTo(state.graph.nodes[path[path.length - 1]]);
  }

  function jumpToFirstPath(k) {
    const art = state.artifacts.get(k);
    if (art) jumpToDirect(art.occ[0]);
  }

  // ------------------------------------------------------------- controls
  for (const r of document.querySelectorAll('input[name="mode"]')) {
    r.addEventListener("change", () => {
      state.mode = r.value;
      // Keep the selected artifact across modes.
      if (state.graph && state.selected != null) {
        const nodes = state.graph.nodes;
        if (state.mode === "tree" && typeof state.selected === "string") {
          state.selected = state.artifacts.get(state.selected)?.occ[0] ?? null;
          if (state.selected != null) for (const j of pathToRoot(state.selected).slice(1)) state.expanded.add(j);
        } else if (state.mode !== "tree" && typeof state.selected === "number") {
          state.selected = keyOf(nodes[state.selected]);
        }
      }
      render();
    });
  }

  let searchTimer = 0;
  search.addEventListener("input", () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      state.query = search.value.trim().toLowerCase();
      renderLeft();
    }, 120);
  });
  $("show-group").addEventListener("change", (e) => {
    state.showGroup = e.target.checked;
    renderLeft();
    renderRight();
  });
  $("show-scope").addEventListener("change", (e) => {
    state.showScope = e.target.checked;
    renderLeft();
    renderRight();
  });
  $("refresh").addEventListener("click", load);
  $("expand").addEventListener("click", () => {
    state.graph?.nodes.forEach((_, i) => state.children[i]?.length && state.expanded.add(i));
    renderLeft();
  });
  $("collapse").addEventListener("click", () => {
    state.expanded.clear();
    renderLeft();
  });

  left.addEventListener("keydown", (e) => {
    if (!state.rows.length) return;
    const cur = state.rows.findIndex((r) => (r.key ?? r.index) === state.selected);
    const pick = (i) => {
      const r = state.rows[Math.max(0, Math.min(state.rows.length - 1, i))];
      select(r.key ?? r.index);
    };
    if (e.key === "ArrowDown") pick(cur + 1);
    else if (e.key === "ArrowUp") pick(cur < 0 ? 0 : cur - 1);
    else if (e.key === "Home") pick(0);
    else if (e.key === "End") pick(state.rows.length - 1);
    else if (e.key === "ArrowRight" && typeof state.selected === "number") {
      if (!state.expanded.has(state.selected)) toggle(state.selected);
    } else if (e.key === "ArrowLeft" && typeof state.selected === "number") {
      if (state.expanded.has(state.selected)) toggle(state.selected);
      else {
        const p = state.graph.nodes[state.selected].p;
        if (p) select(p);
      }
    } else if (e.key === "Enter" && state.selected != null) {
      if (typeof state.selected === "number") jumpToDirect(state.selected);
      else jumpToFirstPath(state.selected);
    } else return;
    e.preventDefault();
  });

  // The POM changed (typed in the text editor, an exclusion, Undo): read again.
  let changeTimer = 0;
  bridge.document?.onChange(() => {
    clearTimeout(changeTimer);
    changeTimer = setTimeout(load, 800);
  });

  setStatus("Reading the dependency graph…");
  renderRight();
  load();
})();
