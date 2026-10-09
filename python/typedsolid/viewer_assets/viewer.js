// 3D viewerの描画とUI。ViewerCoreの関数とWebGL 1で、埋め込んだ部品と検査結果を描く。
// 表示状態はURL fragmentに書き、同じURLで同じ表示を再現できるようにする。
(function () {
  "use strict";

  const core = globalThis.ViewerCore;
  const FOVY = (40 * Math.PI) / 180;
  const PART_COLOURS = [
    [0.55, 0.62, 0.72], [0.80, 0.68, 0.48], [0.56, 0.72, 0.60], [0.70, 0.58, 0.74],
    [0.52, 0.70, 0.76], [0.62, 0.66, 0.78], [0.66, 0.66, 0.52], [0.60, 0.60, 0.64],
  ];
  // 部品色は赤みを持たせない。検出箇所の赤と区別するためである。
  const LOCATION_COLOUR = [0.84, 0.15, 0.16];
  const KEEPOUT_COLOUR = [0.13, 0.56, 0.35];
  const CUT_COLOUR = [0.35, 0.37, 0.40];
  const GHOST_ALPHA = 0.25;
  // 切り口は部品色を暗くした地に、画面上で一定間隔の45°の斜線を入れる。値は部品色に掛ける比で、
  // 斜線の比は陰影の下限 (0.5) より小さく、表面の色と重ならない。scripts/check-viewer.pyが読む。
  const CAP_FILL = 0.6;
  const CAP_STRIPE = 0.35;
  const CAP_PERIOD_PX = 8.0;

  const data = JSON.parse(document.getElementById("viewer-data").textContent);
  const checks = data.checks.map((check, index) => ({ ...check, index, locations: check.locations || [] }));
  const modelBox = core.unionBoxes(data.parts.map((part) => part.bounds).concat(data.keepouts));
  const modelRadius = Math.max(Math.hypot(...[0, 1, 2].map((i) => modelBox.max[i] - modelBox.min[i])) / 2, 1);

  let state = core.parseState(location.hash);
  let pan = [0, 0, 0];

  // ---- WebGL ----

  const canvas = document.getElementById("view");
  // canvasを不透明にする。半透明の部品を描くと描画先のalphaが下がり、pageの背景と合成されて消えるためである。
  // 切り口はstencilで部品の内部を判定して塗る。stencilを得られない環境では切り口を描かない。
  const gl = canvas.getContext("webgl", { alpha: false, antialias: true, stencil: true, preserveDrawingBuffer: true });
  const hasStencil = Boolean(gl && gl.getContextAttributes().stencil);

  const SURFACE_VERTEX = `
    attribute vec3 aPosition;
    attribute vec3 aNormal;
    uniform mat4 uViewProj;
    varying vec3 vPosition;
    varying vec3 vNormal;
    void main() {
      vPosition = aPosition;
      vNormal = aNormal;
      gl_Position = uViewProj * vec4(aPosition, 1.0);
    }`;
  // 断面の向こう側を捨て、切り口から見える裏面は灰色で描く。
  const SURFACE_FRAGMENT = `
    precision mediump float;
    uniform vec4 uColour;
    uniform vec4 uClip;
    uniform vec3 uLight;
    uniform vec3 uCut;
    varying vec3 vPosition;
    varying vec3 vNormal;
    void main() {
      if (dot(vec4(vPosition, 1.0), uClip) < 0.0) discard;
      if (!gl_FrontFacing) {
        gl_FragColor = vec4(uCut, uColour.a);
        return;
      }
      float light = 0.5 + 0.5 * abs(dot(normalize(vNormal), uLight));
      gl_FragColor = vec4(uColour.rgb * light, uColour.a);
    }`;
  const FLAT_VERTEX = `
    attribute vec3 aPosition;
    uniform mat4 uViewProj;
    void main() {
      gl_Position = uViewProj * vec4(aPosition, 1.0);
    }`;
  const FLAT_FRAGMENT = `
    precision mediump float;
    uniform vec4 uColour;
    void main() {
      gl_FragColor = uColour;
    }`;
  const CAP_FRAGMENT = `
    precision mediump float;
    uniform vec4 uColour;
    uniform vec3 uCut;
    void main() {
      float phase = mod(gl_FragCoord.x + gl_FragCoord.y, ${CAP_PERIOD_PX.toFixed(1)});
      gl_FragColor = phase < ${(CAP_PERIOD_PX / 3).toFixed(1)} ? vec4(uCut, uColour.a) : uColour;
    }`;

  function compile(type, source) {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      throw new Error(gl.getShaderInfoLog(shader));
    }
    return shader;
  }

  function program(vertex, fragment) {
    const handle = gl.createProgram();
    gl.attachShader(handle, compile(gl.VERTEX_SHADER, vertex));
    gl.attachShader(handle, compile(gl.FRAGMENT_SHADER, fragment));
    gl.linkProgram(handle);
    if (!gl.getProgramParameter(handle, gl.LINK_STATUS)) {
      throw new Error(gl.getProgramInfoLog(handle));
    }
    const uniforms = {};
    for (const name of ["uViewProj", "uColour", "uClip", "uLight", "uCut"]) {
      uniforms[name] = gl.getUniformLocation(handle, name);
    }
    return {
      handle,
      uniforms,
      position: gl.getAttribLocation(handle, "aPosition"),
      normal: gl.getAttribLocation(handle, "aNormal"),
    };
  }

  function buffer(array) {
    const handle = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, handle);
    gl.bufferData(gl.ARRAY_BUFFER, array, gl.STATIC_DRAW);
    return handle;
  }

  function boxTriangles(box) {
    const c = (bits) => [0, 1, 2].map((i) => ((bits >> i) & 1 ? box.max[i] : box.min[i]));
    const faces = [[0, 2, 6, 4], [1, 5, 7, 3], [0, 4, 5, 1], [2, 3, 7, 6], [0, 1, 3, 2], [4, 6, 7, 5]];
    const out = [];
    for (const [a, b, d, e] of faces) {
      out.push(...c(a), ...c(b), ...c(d), ...c(a), ...c(d), ...c(e));
    }
    return new Float32Array(out);
  }

  let surface = null;
  let flat = null;
  let cap = null;
  let parts = [];
  let keepoutLines = null;

  function setUpScene() {
    surface = program(SURFACE_VERTEX, SURFACE_FRAGMENT);
    flat = program(FLAT_VERTEX, FLAT_FRAGMENT);
    cap = program(FLAT_VERTEX, CAP_FRAGMENT);
    parts = data.parts.map((part, index) => {
      const indices = part.indices ? core.decodeUint32(part.indices) : null;
      const triangles = core.flatTriangles(core.decodeFloat32(part.positions), indices);
      return {
        id: part.id,
        colour: PART_COLOURS[index % PART_COLOURS.length],
        count: triangles.positions.length / 3,
        positions: buffer(triangles.positions),
        normals: buffer(triangles.normals),
      };
    });
    const edges = [];
    for (const keepout of data.keepouts) {
      edges.push(...core.boxEdges(keepout));
    }
    keepoutLines = edges.length ? { buffer: buffer(new Float32Array(edges)), count: edges.length / 3 } : null;
  }

  // ---- カメラ ----

  function selectedCheck() {
    return state.check !== null && state.check < checks.length ? checks[state.check] : null;
  }

  function focusedBox() {
    const check = selectedCheck();
    if (check && state.loc !== null && state.loc < check.locations.length) {
      return check.locations[state.loc];
    }
    return null;
  }

  function camera(width, height) {
    const focus = focusedBox();
    const box = focus || modelBox;
    const target = core.boxCentre(box).map((value, i) => value + pan[i]);
    // 注視では検出箇所の周囲も入るよう、箇所の外接球の2倍の範囲を収める。
    // 縦長の画面では横の視野角で収める。
    const fov = core.fittingFov(FOVY, width / height);
    const distance = (focus ? 2 * core.fitDistance(box, fov, modelRadius * 0.15) : core.fitDistance(box, fov, 0)) * state.zoom;
    const eye = core.orbitEye(target, distance, state.yaw, state.pitch);
    const near = Math.max(distance - 2 * modelRadius, distance * 0.01);
    const far = distance + 2 * modelRadius;
    const view = core.lookAt(eye, target, [0, 0, 1]);
    const projection = core.perspective(FOVY, width / height, near, far);
    return { target, distance, eye, viewProj: core.multiply(projection, view) };
  }

  // ---- 描画 ----

  let pending = false;

  function requestDraw() {
    if (!pending) {
      pending = true;
      requestAnimationFrame(() => {
        pending = false;
        draw();
      });
    }
  }

  function bind(programInfo, positions, normals) {
    gl.bindBuffer(gl.ARRAY_BUFFER, positions);
    gl.enableVertexAttribArray(programInfo.position);
    gl.vertexAttribPointer(programInfo.position, 3, gl.FLOAT, false, 0, 0);
    if (programInfo.normal >= 0) {
      if (normals) {
        gl.bindBuffer(gl.ARRAY_BUFFER, normals);
        gl.enableVertexAttribArray(programInfo.normal);
        gl.vertexAttribPointer(programInfo.normal, 3, gl.FLOAT, false, 0, 0);
      } else {
        gl.disableVertexAttribArray(programInfo.normal);
      }
    }
  }

  function drawParts(view, ghosts) {
    gl.useProgram(surface.handle);
    gl.uniformMatrix4fv(surface.uniforms.uViewProj, false, view.viewProj);
    gl.uniform4fv(surface.uniforms.uClip, core.clipPlane(state.clip));
    // 光源は視点から少し左上に置き、向きの異なる面の明るさを分ける。
    const toEye = normalize3([0, 1, 2].map((i) => view.eye[i] - view.target[i]));
    const right = normalize3(cross3([0, 0, 1], toEye));
    const up = cross3(toEye, right);
    gl.uniform3fv(surface.uniforms.uLight, normalize3([0, 1, 2].map((i) => toEye[i] + 0.7 * up[i] - 0.4 * right[i])));
    gl.uniform3fv(surface.uniforms.uCut, CUT_COLOUR);
    for (const part of parts) {
      if (state.hidden.includes(part.id) || state.ghost.includes(part.id) !== ghosts) {
        continue;
      }
      gl.uniform4fv(surface.uniforms.uColour, [...part.colour, ghosts ? GHOST_ALPHA : 1]);
      bind(surface, part.positions, part.normals);
      gl.drawArrays(gl.TRIANGLES, 0, part.count);
    }
  }

  // 断面の切り口。部品ごとに、断面より残す側の面を深度によらず数えてstencilの偶奇を反転する。
  // 断面の平面上の点から視線上に奇数個の面があれば、その点は部品の内部にある。その画素だけに
  // 平面の四角形を塗る。四角形は深度検査を行い、手前の部品に隠れる。meshが閉じていることを前提とする。
  function drawCaps(view, ghosts) {
    if (!state.clip || !hasStencil) {
      return;
    }
    const quad = buffer(core.clipQuad(state.clip, modelBox, modelRadius * 0.05));
    gl.enable(gl.STENCIL_TEST);
    gl.stencilMask(0xff);
    for (const part of parts) {
      if (state.hidden.includes(part.id) || state.ghost.includes(part.id) !== ghosts) {
        continue;
      }
      gl.clear(gl.STENCIL_BUFFER_BIT);
      gl.colorMask(false, false, false, false);
      gl.depthMask(false);
      gl.disable(gl.DEPTH_TEST);
      gl.stencilFunc(gl.ALWAYS, 0, 0xff);
      gl.stencilOp(gl.KEEP, gl.KEEP, gl.INVERT);
      gl.useProgram(surface.handle);
      bind(surface, part.positions, part.normals);
      gl.drawArrays(gl.TRIANGLES, 0, part.count);

      gl.colorMask(true, true, true, true);
      gl.enable(gl.DEPTH_TEST);
      gl.depthMask(!ghosts);
      gl.stencilFunc(gl.NOTEQUAL, 0, 0xff);
      gl.stencilOp(gl.KEEP, gl.KEEP, gl.KEEP);
      gl.useProgram(cap.handle);
      gl.uniformMatrix4fv(cap.uniforms.uViewProj, false, view.viewProj);
      const alpha = ghosts ? GHOST_ALPHA : 1;
      gl.uniform4fv(cap.uniforms.uColour, [...part.colour.map((c) => c * CAP_FILL), alpha]);
      gl.uniform3fv(cap.uniforms.uCut, part.colour.map((c) => c * CAP_STRIPE));
      bind(cap, quad, null);
      gl.drawArrays(gl.TRIANGLES, 0, 6);
    }
    gl.disable(gl.STENCIL_TEST);
    gl.deleteBuffer(quad);
  }

  function drawFlat(view, array, mode, colour) {
    gl.useProgram(flat.handle);
    gl.uniformMatrix4fv(flat.uniforms.uViewProj, false, view.viewProj);
    gl.uniform4fv(flat.uniforms.uColour, colour);
    const handle = buffer(array);
    bind(flat, handle, null);
    gl.drawArrays(mode, 0, array.length / 3);
    gl.deleteBuffer(handle);
  }

  // 選択中のcheckがあればその検出箇所だけを、なければfailしたcheckのすべての検出箇所を描く。
  function visibleLocations() {
    const check = selectedCheck();
    const source = check ? [check] : checks.filter((c) => c.status === "fail");
    const out = [];
    for (const item of source) {
      item.locations.forEach((box, loc) => out.push({ check: item.index, loc, box }));
    }
    return out;
  }

  function draw() {
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;
    const ratio = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
    }
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.clearColor(1, 1, 1, 1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    const view = camera(width, height);

    gl.enable(gl.DEPTH_TEST);
    gl.disable(gl.BLEND);
    gl.depthMask(true);
    drawParts(view, false);
    drawCaps(view, false);
    gl.depthMask(true);
    if (keepoutLines) {
      gl.useProgram(flat.handle);
      gl.uniformMatrix4fv(flat.uniforms.uViewProj, false, view.viewProj);
      gl.uniform4fv(flat.uniforms.uColour, [...KEEPOUT_COLOUR, 1]);
      bind(flat, keepoutLines.buffer, null);
      gl.drawArrays(gl.LINES, 0, keepoutLines.count);
    }

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.depthMask(false);
    drawParts(view, true);
    // drawPartsが断面の平面とuniformを設定した後に描く。
    drawCaps(view, true);

    // 検出箇所は部品の内部にあることが多いため、深度によらず手前に描く。
    gl.disable(gl.DEPTH_TEST);
    const focus = focusedBox();
    if (focus) {
      drawFlat(view, boxTriangles(focus), gl.TRIANGLES, [...LOCATION_COLOUR, 0.25]);
    }
    for (const item of visibleLocations()) {
      drawFlat(view, core.boxEdges(item.box), gl.LINES, [...LOCATION_COLOUR, 1]);
    }
    gl.depthMask(true);
    document.body.dataset.drawn = String(Number(document.body.dataset.drawn || 0) + 1);
  }

  // ---- 状態の反映 ----

  function setState(changes, options) {
    state = { ...state, ...changes };
    const hash = core.formatState(state);
    if (hash !== location.hash && !(hash === "" && location.hash === "")) {
      history.replaceState(null, "", hash || location.pathname + location.search);
    }
    if (!options || options.panel !== false) {
      renderPanel();
    }
    requestDraw();
  }

  // ---- UI ----

  function element(tag, attributes, children) {
    const node = document.createElement(tag);
    for (const [key, value] of Object.entries(attributes || {})) {
      if (key === "text") {
        node.textContent = value;
      } else if (key.startsWith("on")) {
        node.addEventListener(key.slice(2), value);
      } else if (value !== false && value !== null && value !== undefined) {
        node.setAttribute(key, value === true ? "" : String(value));
      }
    }
    for (const child of children || []) {
      node.append(child);
    }
    return node;
  }

  function formatBox(box) {
    const text = (values) => values.map((v) => v.toFixed(2)).join(", ");
    return `(${text(box.min)}) – (${text(box.max)}) mm`;
  }

  function renderControls() {
    const status = document.getElementById("filter-status");
    status.value = state.status;
    const rule = document.getElementById("filter-rule");
    if (!rule.options.length) {
      rule.append(element("option", { value: "", text: "all rules" }));
      for (const name of [...new Set(checks.map((c) => c.rule))].sort()) {
        rule.append(element("option", { value: name, text: name }));
      }
    }
    rule.value = state.rule;
    const query = document.getElementById("filter-text");
    if (document.activeElement !== query) {
      query.value = state.q;
    }
    document.getElementById("sort").value = state.sort;
    document.getElementById("clip-axis").value = state.clip ? state.clip.axis : "";
    const slider = document.getElementById("clip-value");
    const flip = document.getElementById("clip-flip");
    slider.disabled = flip.disabled = !state.clip;
    if (state.clip) {
      const axis = core.AXES.indexOf(state.clip.axis);
      slider.min = String(modelBox.min[axis]);
      slider.max = String(modelBox.max[axis]);
      slider.step = String((modelBox.max[axis] - modelBox.min[axis]) / 200 || 0.01);
      slider.value = String(state.clip.value);
      flip.checked = state.clip.flip;
    }
    document.getElementById("clip-label").textContent = state.clip ? `${state.clip.axis} = ${state.clip.value.toFixed(2)} mm` : "off";
  }

  function renderParts() {
    const list = document.getElementById("parts");
    list.replaceChildren();
    parts.forEach((part) => {
      const swatch = element("span", { class: "swatch" });
      swatch.style.background = `rgb(${part.colour.map((v) => Math.round(v * 255)).join(",")})`;
      const toggle = (key) => () => {
        const values = state[key].includes(part.id) ? state[key].filter((id) => id !== part.id) : [...state[key], part.id];
        setState({ [key]: values });
      };
      list.append(element("li", {}, [
        swatch,
        element("span", { class: "part-id", text: part.id }),
        element("label", {}, [element("input", { type: "checkbox", checked: !state.hidden.includes(part.id), onchange: toggle("hidden") }), "shown"]),
        element("label", {}, [element("input", { type: "checkbox", checked: state.ghost.includes(part.id), onchange: toggle("ghost") }), "translucent"]),
      ]));
    });
  }

  function renderChecks() {
    const shown = core.sortChecks(core.filterChecks(checks, state), state.sort);
    document.getElementById("check-count").textContent = `${shown.length} of ${checks.length} checks`;
    const list = document.getElementById("checks");
    list.replaceChildren();
    for (const check of shown) {
      const selected = state.check === check.index;
      const row = element("li", { class: `check ${check.status}${selected ? " selected" : ""}` }, [
        element("button", {
          class: "check-head",
          "aria-pressed": selected,
          onclick: () => setState(selected ? { check: null, loc: null } : { check: check.index, loc: null }),
        }, [
          element("span", { class: "status", text: check.status.replace("_", " ") }),
          element("span", { class: "rule", text: check.rule }),
          element("span", { class: "target", text: check.target }),
          element("span", { class: "count", text: check.locations.length ? String(check.locations.length) : "" }),
        ]),
      ]);
      if (selected) {
        row.append(element("p", { class: "message", text: check.message }));
        const locations = element("ol", { class: "locations" });
        check.locations.forEach((box, loc) => {
          const focused = state.loc === loc;
          locations.append(element("li", {}, [
            element("button", {
              class: focused ? "focus active" : "focus",
              text: focused ? "unfocus" : "focus",
              onclick: () => {
                pan = [0, 0, 0];
                setState({ loc: focused ? null : loc, zoom: 1 });
              },
            }),
            element("span", { text: formatBox(box) }),
          ]));
        });
        row.append(locations);
      }
      list.append(row);
    }
  }

  function renderPanel() {
    renderControls();
    renderParts();
    renderChecks();
  }

  function bindControls() {
    document.getElementById("filter-status").addEventListener("change", (event) => setState({ status: event.target.value }));
    document.getElementById("filter-rule").addEventListener("change", (event) => setState({ rule: event.target.value }));
    document.getElementById("filter-text").addEventListener("input", (event) => setState({ q: event.target.value }));
    document.getElementById("sort").addEventListener("change", (event) => setState({ sort: event.target.value }));
    document.getElementById("clip-axis").addEventListener("change", (event) => {
      const axis = event.target.value;
      if (!axis) {
        setState({ clip: null });
        return;
      }
      const index = core.AXES.indexOf(axis);
      setState({ clip: { axis, value: (modelBox.min[index] + modelBox.max[index]) / 2, flip: false } });
    });
    // dragの間はpanel全体を作り直さず、断面の位置の表示だけを更新する。
    document.getElementById("clip-value").addEventListener("input", (event) => {
      setState({ clip: { ...state.clip, value: Number(event.target.value) } }, { panel: false });
      document.getElementById("clip-label").textContent = `${state.clip.axis} = ${state.clip.value.toFixed(2)} mm`;
    });
    document.getElementById("clip-value").addEventListener("change", () => renderControls());
    document.getElementById("clip-flip").addEventListener("change", (event) => setState({ clip: { ...state.clip, flip: event.target.checked } }));
    document.getElementById("reset-view").addEventListener("click", () => {
      pan = [0, 0, 0];
      setState({ loc: null, yaw: core.DEFAULT_STATE.yaw, pitch: core.DEFAULT_STATE.pitch, zoom: 1 });
    });
    window.addEventListener("hashchange", () => {
      state = core.parseState(location.hash);
      renderPanel();
      requestDraw();
    });
    window.addEventListener("resize", requestDraw);
  }

  // ---- 操作: 左dragで回転、右dragかshift+dragで平行移動、wheelで拡大、clickで検出箇所の選択 ----

  function bindPointer() {
    const pointers = new Map();
    let moved = 0;
    let pinch = null;

    canvas.addEventListener("contextmenu", (event) => event.preventDefault());
    canvas.addEventListener("pointerdown", (event) => {
      canvas.setPointerCapture(event.pointerId);
      pointers.set(event.pointerId, { x: event.clientX, y: event.clientY, button: event.button, shift: event.shiftKey });
      moved = 0;
      pinch = null;
    });
    canvas.addEventListener("pointermove", (event) => {
      const previous = pointers.get(event.pointerId);
      if (!previous) {
        return;
      }
      const dx = event.clientX - previous.x;
      const dy = event.clientY - previous.y;
      previous.x = event.clientX;
      previous.y = event.clientY;
      moved += Math.abs(dx) + Math.abs(dy);
      if (pointers.size === 2) {
        const [a, b] = [...pointers.values()];
        const spread = Math.hypot(a.x - b.x, a.y - b.y);
        if (pinch) {
          state.zoom = Math.min(20, Math.max(0.05, state.zoom * (pinch / spread)));
        }
        pinch = spread;
      } else if (previous.button === 2 || previous.shift) {
        const view = camera(canvas.clientWidth, canvas.clientHeight);
        const scale = (2 * view.distance * Math.tan(FOVY / 2)) / canvas.clientHeight;
        const forward = [0, 1, 2].map((i) => view.target[i] - view.eye[i]);
        const right = normalize3(cross3(forward, [0, 0, 1]));
        const up = normalize3(cross3(right, forward));
        pan = pan.map((value, i) => value - (dx * right[i] - dy * up[i]) * scale);
      } else {
        state.yaw -= dx * 0.4;
        state.pitch = Math.max(-89, Math.min(89, state.pitch + dy * 0.4));
      }
      requestDraw();
    });
    const release = (event) => {
      const previous = pointers.get(event.pointerId);
      pointers.delete(event.pointerId);
      if (previous && moved < 4 && event.type === "pointerup" && previous.button === 0) {
        pickAt(event);
      }
      setState({}, { panel: false });
    };
    canvas.addEventListener("pointerup", release);
    canvas.addEventListener("pointercancel", release);
    canvas.addEventListener("wheel", (event) => {
      event.preventDefault();
      state.zoom = Math.min(20, Math.max(0.05, state.zoom * Math.exp(event.deltaY * 0.001)));
      setState({}, { panel: false });
    }, { passive: false });
  }

  function cross3(a, b) {
    return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  }

  function normalize3(a) {
    const length = Math.hypot(...a) || 1;
    return a.map((v) => v / length);
  }

  function pickAt(event) {
    const rect = canvas.getBoundingClientRect();
    const view = camera(rect.width, rect.height);
    const ray = core.screenRay(event.clientX - rect.left, event.clientY - rect.top, rect.width, rect.height, core.invert(view.viewProj));
    const hit = core.pickLocation(ray, visibleLocations());
    if (hit) {
      setState({ check: hit.check, loc: null });
      const row = document.querySelector("#checks .selected");
      if (row) {
        row.scrollIntoView({ block: "nearest" });
      }
    }
  }

  // ---- 起動 ----

  document.getElementById("title").textContent = data.title;
  bindControls();
  if (!gl) {
    document.getElementById("no-webgl").hidden = false;
    renderPanel();
    return;
  }
  setUpScene();
  bindPointer();
  renderPanel();
  // 読み込み直後に1回描く。headlessの撮影がload直後の画面を写すためである。
  draw();
})();
