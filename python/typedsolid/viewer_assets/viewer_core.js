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
    // 製造案の絞り込み。空なら採用した製造案と、製造案に依らないcheckを示す。"*"ならすべての
    // 製造案、それ以外は指定した製造案と、製造案に依らないcheckを示す。
    plan: "",
    q: "",
    sort: "severity",
    yaw: 35,
    pitch: 25,
    zoom: 1,
    // 表示する分布の番号。nullなら部品色で描く。
    field: null,
    map: "sequential",
    scale: "limit",
    // scaleが"custom"のときの範囲 [下端, 上端]。
    range: null,
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

  function decodeUint16(text) {
    const bytes = bytesOfBase64(text);
    const view = new DataView(bytes.buffer);
    const out = new Uint16Array(bytes.length / 2);
    for (let i = 0; i < out.length; i += 1) {
      out[i] = view.getUint16(2 * i, true);
    }
    return out;
  }

  function decodeBytes(text) {
    return bytesOfBase64(text);
  }

  // 三角形ごとに頂点を複製し、面の法線を付ける。陰影で面の境界を見せるためである。
  // indicesがnullなら、positionsは三角形ごとに3頂点を並べたもの (STLの三角形) とする。
  function flatTriangles(positions, indices) {
    const count = indices ? indices.length : positions.length / 3;
    const outPositions = new Float32Array(count * 3);
    const outNormals = new Float32Array(count * 3);
    for (let t = 0; t < count; t += 3) {
      const p = [0, 1, 2].map((k) => {
        const i = (indices ? indices[t + k] : t + k) * 3;
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

  // flatTrianglesと同じ順に、頂点ごとの値を三角形の隅ごとに並べ直す。
  function expandPerVertex(values, indices) {
    if (!indices) {
      return values;
    }
    const out = new values.constructor(indices.length);
    for (let i = 0; i < indices.length; i += 1) {
      out[i] = values[indices[i]];
    }
    return out;
  }

  // ---- 分布の色 ----

  // 量子化した分布 (u16) のうち、表面の解析cellが近くに無い頂点を表す値。Python側と同じである。
  const NO_VALUE = 0xffff;
  const QUANTIZATION_STEPS = NO_VALUE - 1;

  function dequantize(field, codes) {
    const out = new Float32Array(codes.length);
    const span = field.maximum - field.minimum;
    for (let i = 0; i < codes.length; i += 1) {
      out[i] = codes[i] === NO_VALUE ? NaN : field.minimum + (codes[i] / QUANTIZATION_STEPS) * span;
    }
    return out;
  }

  function hexToRgb(hex) {
    const value = parseInt(hex.slice(1), 16);
    return [(value >> 16) & 255, (value >> 8) & 255, value & 255].map((c) => c / 255);
  }

  function toLinear(c) {
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  }

  function fromLinear(c) {
    const clamped = Math.min(1, Math.max(0, c));
    return clamped <= 0.0031308 ? 12.92 * clamped : 1.055 * clamped ** (1 / 2.4) - 0.055;
  }

  // sRGB (0〜1) とOKLabの変換。色は知覚的に一様なOKLabで補間する。
  function rgbToOklab(rgb) {
    const [r, g, b] = rgb.map(toLinear);
    const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
    const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
    const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
    return [
      0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
      1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
      0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    ];
  }

  function oklabToLinear(lab) {
    const [L, a, b] = lab;
    const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
    const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
    const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
    return [
      4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
      -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
      -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    ];
  }

  function oklabToRgb(lab) {
    return oklabToLinear(lab).map(fromLinear);
  }

  // OKLabの明度を変えた色。色相を保ち、sRGBに収まらなければ明度を保って彩度を下げる。
  function withLightness(hex, lightness) {
    const [, a, b] = rgbToOklab(hexToRgb(hex));
    const inside = (k) => oklabToLinear([lightness, a * k, b * k]).every((c) => c >= 0 && c <= 1);
    let low = 0;
    let high = 1;
    if (!inside(high)) {
      for (let i = 0; i < 30; i += 1) {
        const k = (low + high) / 2;
        if (inside(k)) {
          low = k;
        } else {
          high = k;
        }
      }
      high = low;
    }
    return oklabToRgb([lightness, a * high, b * high]);
  }

  function rgbToHex(rgb) {
    return `#${rgb.map((c) => Math.round(c * 255).toString(16).padStart(2, "0")).join("")}`;
  }

  // 連続色は青の1色相を明るい側から暗い側へ並べる。発散色は青と赤を灰の中点で結び、
  // 両端の明度を揃える。赤の暗い端は、赤の色相と彩度を保って青の暗い端の明度にした色である。
  const BLUE_DARK = "#0d366b";
  const RED = "#e34948";
  const COLOURMAPS = Object.freeze({
    sequential: Object.freeze({
      label: "sequential (blue)",
      stops: ["#86b6ef", "#3987e5", "#1c5cab", BLUE_DARK],
    }),
    diverging: Object.freeze({
      label: "diverging (blue – grey – red)",
      stops: [BLUE_DARK, "#3987e5", "#f0efec", RED, rgbToHex(withLightness(RED, rgbToOklab(hexToRgb(BLUE_DARK))[0]))],
    }),
  });
  // 上限に対する比の尺度で、上限を超えた値の色。連続色の青と区別する。
  const OVER_LIMIT = RED;

  // 範囲の決め方。limitは基準値から上限まで、dataは値の最小から最大まで、divergingは上限を
  // 中点とし基準値と最大値のうち上限から遠い方までを両側に取る、customは利用者が与える範囲である。
  const SCALES = Object.freeze({
    limit: "reference to limit",
    data: "minimum to maximum",
    diverging: "centred on the limit",
    custom: "custom range",
  });

  function colourAt(name, t) {
    const stops = (COLOURMAPS[name] || COLOURMAPS.sequential).stops;
    const x = Math.min(1, Math.max(0, t)) * (stops.length - 1);
    const k = Math.min(stops.length - 2, Math.floor(x));
    const a = rgbToOklab(hexToRgb(stops[k]));
    const b = rgbToOklab(hexToRgb(stops[k + 1]));
    const f = x - k;
    return oklabToRgb(a.map((v, i) => v + (b[i] - v) * f));
  }

  // 最小と最大の表記。局所的に細かくした格子の間隔などで、一方が無いか等しければ1つの値とする。
  function rangeLabel(low, high) {
    const has = (value) => value !== null && value !== undefined;
    if (has(low) && has(high) && low !== high) {
      return `${low}–${high}`;
    }
    return String(has(low) ? low : high);
  }

  // 分布の色の範囲。上限の無い量では、limitとdivergingは値の最小から最大までとする。
  function fieldScale(field, mode, range) {
    const hasLimit = field.limit !== null && field.limit !== undefined;
    if (mode === "custom" && range && range[1] > range[0]) {
      return { low: range[0], high: range[1], over: false };
    }
    if (mode === "limit" && hasLimit) {
      // 上限が基準値以下でも、上限を超える値は上限超過の色で示す。
      const low = Math.min(field.reference, field.minimum);
      return { low: low < field.limit ? low : field.limit - 1e-12, high: field.limit, over: true };
    }
    if (mode === "diverging" && hasLimit) {
      const half = Math.max(field.limit - Math.min(field.reference, field.minimum), field.maximum - field.limit, 1e-12);
      return { low: field.limit - half, high: field.limit + half, over: false };
    }
    const high = field.maximum > field.minimum ? field.maximum : field.minimum + 1e-12;
    return { low: field.minimum, high, over: false };
  }

  // 値の色。値の無い頂点はnullとし、部品色で描く。
  function fieldColour(value, scale, map) {
    if (Number.isNaN(value)) {
      return null;
    }
    if (scale.over && value > scale.high) {
      return hexToRgb(OVER_LIMIT);
    }
    return colourAt(map, (value - scale.low) / (scale.high - scale.low));
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

  // 縦と横のうち狭い方の視野角。縦長の画面では横の視野角が縦より狭い。
  function fittingFov(fovy, aspect) {
    return aspect < 1 ? 2 * Math.atan(Math.tan(fovy / 2) * aspect) : fovy;
  }

  // boxの外接球が視野角fovに収まる距離。minRadiusは小さい検出箇所に寄りすぎないための下限。
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

  // 断面の平面上で、boxの範囲をmarginだけ広げた四角形。2つの三角形の頂点を並べる。
  // 切り口 (cap) はこの四角形のうち、部品の内部にある画素だけを塗る。
  function clipQuad(clip, box, margin) {
    const axis = AXES.indexOf(clip.axis);
    const [u, v] = [0, 1, 2].filter((i) => i !== axis);
    const corner = (a, b) => {
      const point = [0, 0, 0];
      point[axis] = clip.value;
      point[u] = a ? box.max[u] + margin : box.min[u] - margin;
      point[v] = b ? box.max[v] + margin : box.min[v] - margin;
      return point;
    };
    const [p00, p10, p11, p01] = [corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1)];
    return new Float32Array([...p00, ...p10, ...p11, ...p00, ...p11, ...p01]);
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
    state.plan = params.get("plan") || "";
    state.q = params.get("q") || "";
    if (SORT_KEYS.includes(params.get("sort"))) {
      state.sort = params.get("sort");
    }
    if (params.has("field")) {
      const field = parseNumber(params.get("field"), null);
      state.field = Number.isInteger(field) && field >= 0 ? field : null;
    }
    if (Object.hasOwn(COLOURMAPS, params.get("map"))) {
      state.map = params.get("map");
    }
    if (Object.hasOwn(SCALES, params.get("scale"))) {
      state.scale = params.get("scale");
    }
    if (params.has("range")) {
      const [low, high] = params.get("range").split(",").map((v) => parseNumber(v, null));
      state.range = low !== null && high !== null && high > low ? [low, high] : null;
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

  // 視点の角度と倍率は3桁で足りる。断面の位置は検出箇所と同じ6桁 (1 nm) で書き、
  // 共有したURLで最小のfeatureを横切る位置へずれないようにする。
  function round(value, digits = 3) {
    return Number(value.toFixed(digits)).toString();
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
      params.set("clip", `${state.clip.axis}:${round(state.clip.value, 6)}${state.clip.flip ? ":flip" : ""}`);
    }
    if (state.hidden.length) {
      params.set("hidden", state.hidden.join(","));
    }
    if (state.ghost.length) {
      params.set("ghost", state.ghost.join(","));
    }
    if (state.field !== null) {
      params.set("field", String(state.field));
    }
    for (const key of ["map", "scale"]) {
      if (state[key] !== DEFAULT_STATE[key]) {
        params.set(key, state[key]);
      }
    }
    if (state.range) {
      params.set("range", state.range.map((v) => round(v, 6)).join(","));
    }
    for (const key of ["status", "rule", "plan", "q", "sort"]) {
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

  function matchesPlan(check, plan) {
    if (plan === "*") {
      return true;
    }
    if (!check.plan) {
      return true;
    }
    return plan ? check.plan === plan : check.adopted !== false;
  }

  // checkは書き出し時の順番 (index) を保つ。textは大文字と小文字を区別しない部分一致。
  function filterChecks(checks, filter) {
    const text = (filter.q || "").toLowerCase();
    return checks.filter(
      (check) =>
        matchesStatus(check, filter.status || "all") &&
        matchesPlan(check, filter.plan || "") &&
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
    COLOURMAPS,
    DEFAULT_STATE,
    NO_VALUE,
    OVER_LIMIT,
    SCALES,
    SORT_KEYS,
    STATUS_FILTERS,
    boxCentre,
    boxEdges,
    clipPlane,
    clipQuad,
    colourAt,
    decodeBytes,
    decodeFloat32,
    decodeUint16,
    decodeUint32,
    dequantize,
    expandPerVertex,
    fieldColour,
    fieldScale,
    hexToRgb,
    rgbToHex,
    rgbToOklab,
    filterChecks,
    fitDistance,
    fittingFov,
    flatTriangles,
    formatState,
    invert,
    lookAt,
    multiply,
    orbitEye,
    parseState,
    perspective,
    pickLocation,
    rangeLabel,
    rayBoxDistance,
    screenRay,
    sortChecks,
    transformPoint,
    unionBoxes,
  };
});
