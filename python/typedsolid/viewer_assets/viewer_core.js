// 3D viewerのDOMとWebGLに依存しない処理。行列、データの復号、表示状態のURL fragment、
// checkの絞り込みと並べ替え、検出箇所のpickingを持つ。node:testで検査する。
// ブラウザではglobalのViewerCore、Nodeではmodule.exportsとして公開する。
(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) {
    module.exports = api;
  } else {
    root.ViewerCore = api;
  }
})(globalThis, function () {
  "use strict";

  const AXES = ["x", "y", "z"];
  const STATUS_ORDER = { fail: 0, not_evaluated: 1, pass: 2 };
  const STATUS_FILTERS = ["problems", "fail", "not_evaluated", "pass", "all"];
  const SORT_KEYS = ["severity", "rule", "target", "locations"];
  const DEFAULT_STATE = Object.freeze({
    check: null,
    loc: null,
    clip: null,
    hidden: [],
    ghost: [],
    status: "problems",
    rule: "",
    q: "",
    sort: "severity",
    yaw: 35,
    pitch: 25,
    zoom: 1,
  });

  // ---- データの復号 ----

  function bytesOfBase64(text) {
    const binary = atob(text);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      bytes[i] = binary.charCodeAt(i);
    }
    return bytes;
  }

  // little endianの配列を読む。書き出し側 (Python) はlittle endianで書く。
  function decodeFloat32(text) {
    const bytes = bytesOfBase64(text);
    const view = new DataView(bytes.buffer);
    const out = new Float32Array(bytes.length / 4);
    for (let i = 0; i < out.length; i += 1) {
      out[i] = view.getFloat32(4 * i, true);
    }
    return out;
  }

  function decodeUint32(text) {
    const bytes = bytesOfBase64(text);
    const view = new DataView(bytes.buffer);
    const out = new Uint32Array(bytes.length / 4);
    for (let i = 0; i < out.length; i += 1) {
      out[i] = view.getUint32(4 * i, true);
    }
    return out;
  }

  // 三角形ごとに頂点を複製し、面の法線を付ける。陰影で面の境界を見せるためである。
  function flatTriangles(positions, indices) {
    const count = indices.length;
    const outPositions = new Float32Array(count * 3);
    const outNormals = new Float32Array(count * 3);
    for (let t = 0; t < count; t += 3) {
      const p = [0, 1, 2].map((k) => {
        const i = indices[t + k] * 3;
        return [positions[i], positions[i + 1], positions[i + 2]];
      });
      const n = normalize(cross(subtract(p[1], p[0]), subtract(p[2], p[0])));
      for (let k = 0; k < 3; k += 1) {
        outPositions.set(p[k], (t + k) * 3);
        outNormals.set(n, (t + k) * 3);
      }
    }
    return { positions: outPositions, normals: outNormals };
  }

  // ---- ベクトルと行列 (列優先の4×4) ----

  function subtract(a, b) {
    return [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
  }

  function cross(a, b) {
    return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  }

  function dot(a, b) {
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  }

  function normalize(a) {
    const length = Math.hypot(a[0], a[1], a[2]);
    return length > 0 ? [a[0] / length, a[1] / length, a[2] / length] : [0, 0, 0];
  }

  function multiply(a, b) {
    const out = new Float32Array(16);
    for (let column = 0; column < 4; column += 1) {
      for (let row = 0; row < 4; row += 1) {
        let sum = 0;
        for (let k = 0; k < 4; k += 1) {
          sum += a[k * 4 + row] * b[column * 4 + k];
        }
        out[column * 4 + row] = sum;
      }
    }
    return out;
  }

  function perspective(fovy, aspect, near, far) {
    const f = 1 / Math.tan(fovy / 2);
    const out = new Float32Array(16);
    out[0] = f / aspect;
    out[5] = f;
    out[10] = (far + near) / (near - far);
    out[11] = -1;
    out[14] = (2 * far * near) / (near - far);
    return out;
  }

  function lookAt(eye, target, up) {
    const z = normalize(subtract(eye, target));
    const x = normalize(cross(up, z));
    const y = cross(z, x);
    return new Float32Array([
      x[0], y[0], z[0], 0,
      x[1], y[1], z[1], 0,
      x[2], y[2], z[2], 0,
      -dot(x, eye), -dot(y, eye), -dot(z, eye), 1,
    ]);
  }

  function invert(m) {
    const inv = new Float32Array(16);
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
    const determinant = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if (determinant === 0) {
      throw new Error("singular matrix");
    }
    for (let i = 0; i < 16; i += 1) {
      inv[i] /= determinant;
    }
    return inv;
  }

  function transformPoint(m, p) {
    const x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
    const y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
    const z = m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14];
    const w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
    return [x / w, y / w, z / w];
  }

  // ---- 範囲とカメラ ----

  function boxCentre(box) {
    return [0, 1, 2].map((i) => (box.min[i] + box.max[i]) / 2);
  }

  function boxRadius(box) {
    return Math.hypot(...[0, 1, 2].map((i) => box.max[i] - box.min[i])) / 2;
  }

  function unionBoxes(boxes) {
    if (boxes.length === 0) {
      return { min: [0, 0, 0], max: [0, 0, 0] };
    }
    return {
      min: [0, 1, 2].map((i) => Math.min(...boxes.map((b) => b.min[i]))),
      max: [0, 1, 2].map((i) => Math.max(...boxes.map((b) => b.max[i]))),
    };
  }

  // boxの外接球が縦の視野角に収まる距離。minRadiusは小さい検出箇所に寄りすぎないための下限。
  function fitDistance(box, fovy, minRadius) {
    const radius = Math.max(boxRadius(box), minRadius || 0, 1e-6);
    return radius / Math.sin(fovy / 2);
  }

  // targetを中心に、yaw (z軸周り) とpitch (水平面からの仰角) で決まる方向から見る。単位は度。
  // zを上とする。
  function orbitEye(target, distance, yawDeg, pitchDeg) {
    const yaw = (yawDeg * Math.PI) / 180;
    const pitch = (pitchDeg * Math.PI) / 180;
    return [
      target[0] + distance * Math.cos(pitch) * Math.sin(yaw),
      target[1] - distance * Math.cos(pitch) * Math.cos(yaw),
      target[2] + distance * Math.sin(pitch),
    ];
  }

  // 12辺の線分の端点。gl.LINESで描く。
  function boxEdges(box) {
    const corner = (bits) => [0, 1, 2].map((i) => ((bits >> i) & 1 ? box.max[i] : box.min[i]));
    const out = [];
    for (let bits = 0; bits < 8; bits += 1) {
      for (const bit of [1, 2, 4]) {
        if (!(bits & bit)) {
          out.push(...corner(bits), ...corner(bits | bit));
        }
      }
    }
    return new Float32Array(out);
  }

  // ---- 断面 ----

  // clip {axis, value, flip} を平面 (a, b, c, d) にする。a·x + b·y + c·z + d < 0 の側を描かない。
  // flipが偽なら座標がvalue以下の側を残す。
  function clipPlane(clip) {
    if (!clip) {
      return [0, 0, 0, 1];
    }
    const sign = clip.flip ? 1 : -1;
    const plane = [0, 0, 0, -sign * clip.value];
    plane[AXES.indexOf(clip.axis)] = sign;
    return plane;
  }

  // ---- 表示状態とURL fragment ----

  function parseNumber(text, fallback) {
    const value = Number(text);
    return text !== "" && Number.isFinite(value) ? value : fallback;
  }

  function parseList(text) {
    return text ? text.split(",").filter((item) => item !== "") : [];
  }

  // `#check=3&loc=0&clip=z:8.5&hidden=lid` の形を読む。解釈できない値は既定値に戻す。
  function parseState(hash) {
    const state = { ...DEFAULT_STATE, hidden: [], ghost: [] };
    const params = new URLSearchParams((hash || "").replace(/^#/, ""));
    if (params.has("check")) {
      const check = parseNumber(params.get("check"), null);
      state.check = Number.isInteger(check) && check >= 0 ? check : null;
    }
    if (params.has("loc") && state.check !== null) {
      const loc = parseNumber(params.get("loc"), null);
      state.loc = Number.isInteger(loc) && loc >= 0 ? loc : null;
    }
    if (params.has("clip")) {
      const [axis, value, side] = params.get("clip").split(":");
      const coordinate = parseNumber(value || "", null);
      if (AXES.includes(axis) && coordinate !== null) {
        state.clip = { axis, value: coordinate, flip: side === "flip" };
      }
    }
    state.hidden = parseList(params.get("hidden"));
    state.ghost = parseList(params.get("ghost"));
    if (STATUS_FILTERS.includes(params.get("status"))) {
      state.status = params.get("status");
    }
    state.rule = params.get("rule") || "";
    state.q = params.get("q") || "";
    if (SORT_KEYS.includes(params.get("sort"))) {
      state.sort = params.get("sort");
    }
    if (params.has("view")) {
      const [yaw, pitch, zoom] = params.get("view").split(",");
      state.yaw = parseNumber(yaw || "", DEFAULT_STATE.yaw);
      state.pitch = Math.max(-89, Math.min(89, parseNumber(pitch || "", DEFAULT_STATE.pitch)));
      const factor = parseNumber(zoom || "", DEFAULT_STATE.zoom);
      state.zoom = factor > 0 ? factor : DEFAULT_STATE.zoom;
    }
    return state;
  }

  function round(value) {
    return Number(value.toFixed(3)).toString();
  }

  // 既定値と異なる項目だけを書く。同じ状態は同じ文字列になる。
  function formatState(state) {
    const params = new URLSearchParams();
    if (state.check !== null) {
      params.set("check", String(state.check));
      if (state.loc !== null) {
        params.set("loc", String(state.loc));
      }
    }
    if (state.clip) {
      params.set("clip", `${state.clip.axis}:${round(state.clip.value)}${state.clip.flip ? ":flip" : ""}`);
    }
    if (state.hidden.length) {
      params.set("hidden", state.hidden.join(","));
    }
    if (state.ghost.length) {
      params.set("ghost", state.ghost.join(","));
    }
    for (const key of ["status", "rule", "q", "sort"]) {
      if (state[key] !== DEFAULT_STATE[key]) {
        params.set(key, state[key]);
      }
    }
    if (state.yaw !== DEFAULT_STATE.yaw || state.pitch !== DEFAULT_STATE.pitch || state.zoom !== DEFAULT_STATE.zoom) {
      params.set("view", [state.yaw, state.pitch, state.zoom].map(round).join(","));
    }
    const text = params.toString();
    return text ? `#${text}` : "";
  }

  // ---- checkの絞り込みと並べ替え ----

  function matchesStatus(check, status) {
    if (status === "all") {
      return true;
    }
    if (status === "problems") {
      return check.status !== "pass";
    }
    return check.status === status;
  }

  // checkは書き出し時の順番 (index) を保つ。textは大文字と小文字を区別しない部分一致。
  function filterChecks(checks, filter) {
    const text = (filter.q || "").toLowerCase();
    return checks.filter(
      (check) =>
        matchesStatus(check, filter.status || "all") &&
        (!filter.rule || check.rule === filter.rule) &&
        (!text || [check.rule, check.target, check.message].some((field) => field.toLowerCase().includes(text))),
    );
  }

  function compareText(a, b) {
    return a < b ? -1 : a > b ? 1 : 0;
  }

  // 同じ順位のcheckはindexの順に並べ、並べ替えを決定的にする。
  function sortChecks(checks, key) {
    const comparators = {
      severity: (a, b) => STATUS_ORDER[a.status] - STATUS_ORDER[b.status],
      rule: (a, b) => compareText(a.rule, b.rule),
      target: (a, b) => compareText(a.target, b.target),
      locations: (a, b) => (b.locations || []).length - (a.locations || []).length,
    };
    const compare = comparators[key] || comparators.severity;
    return [...checks].sort((a, b) => compare(a, b) || a.index - b.index);
  }

  // ---- picking ----

  // 画面上の点 (CSS px、左上が原点) を通る視線。invViewProjはview·projectionの逆行列。
  function screenRay(x, y, width, height, invViewProj) {
    const ndcX = (2 * x) / width - 1;
    const ndcY = 1 - (2 * y) / height;
    const near = transformPoint(invViewProj, [ndcX, ndcY, -1]);
    const far = transformPoint(invViewProj, [ndcX, ndcY, 1]);
    return { origin: near, direction: normalize(subtract(far, near)) };
  }

  // 視線がboxに入る距離。交わらなければnull。slab法による。
  function rayBoxDistance(ray, box) {
    let enter = -Infinity;
    let exit = Infinity;
    for (let i = 0; i < 3; i += 1) {
      const origin = ray.origin[i];
      const direction = ray.direction[i];
      if (Math.abs(direction) < 1e-12) {
        if (origin < box.min[i] || origin > box.max[i]) {
          return null;
        }
        continue;
      }
      const t1 = (box.min[i] - origin) / direction;
      const t2 = (box.max[i] - origin) / direction;
      enter = Math.max(enter, Math.min(t1, t2));
      exit = Math.min(exit, Math.max(t1, t2));
    }
    if (exit < Math.max(enter, 0)) {
      return null;
    }
    return Math.max(enter, 0);
  }

  // 視線が最初に入る検出箇所。candidatesは {check, loc, box} の列。
  function pickLocation(ray, candidates) {
    let best = null;
    for (const candidate of candidates) {
      const distance = rayBoxDistance(ray, candidate.box);
      if (distance !== null && (best === null || distance < best.distance)) {
        best = { ...candidate, distance };
      }
    }
    return best;
  }

  return {
    AXES,
    DEFAULT_STATE,
    SORT_KEYS,
    STATUS_FILTERS,
    boxCentre,
    boxEdges,
    clipPlane,
    decodeFloat32,
    decodeUint32,
    filterChecks,
    fitDistance,
    flatTriangles,
    formatState,
    invert,
    lookAt,
    multiply,
    orbitEye,
    parseState,
    perspective,
    pickLocation,
    rayBoxDistance,
    screenRay,
    sortChecks,
    transformPoint,
    unionBoxes,
  };
});
