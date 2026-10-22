// 3D viewerのDOMとWebGLに依存しない処理の検査。`node --test tests/js/`で実行する。
"use strict";

const assert = require("node:assert/strict");
const { test } = require("node:test");
const path = require("node:path");

const core = require(path.join(__dirname, "..", "..", "python", "typedsolid", "viewer_assets", "viewer_core.js"));

function base64Of(values, Type) {
  return Buffer.from(new Type(values).buffer).toString("base64");
}

function close(actual, expected, tolerance = 1e-5) {
  assert.equal(actual.length, expected.length);
  for (let i = 0; i < expected.length; i += 1) {
    assert.ok(Math.abs(actual[i] - expected[i]) <= tolerance, `index ${i}: ${actual[i]} != ${expected[i]}`);
  }
}

test("decodes little endian arrays written by Python", () => {
  // Pythonの struct.pack("<3f", 1.5, -2, 3) と struct.pack("<2I", 7, 4294967295)。
  assert.deepEqual([...core.decodeFloat32(Buffer.from("AADAPwAAAMAAAEBA", "base64").toString("base64"))], [1.5, -2, 3]);
  assert.deepEqual([...core.decodeUint32("BwAAAP////8=")], [7, 4294967295]);
  assert.deepEqual([...core.decodeFloat32(base64Of([0.25, 8], Float32Array))], [0.25, 8]);
});

test("flat triangles repeat vertices and carry the face normal", () => {
  const positions = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]);
  const result = core.flatTriangles(positions, new Uint32Array([0, 1, 2]));
  close(result.positions, [0, 0, 0, 1, 0, 0, 0, 1, 0]);
  close(result.normals, [0, 0, 1, 0, 0, 1, 0, 0, 1]);
  const reversed = core.flatTriangles(positions, new Uint32Array([0, 2, 1]));
  close(reversed.normals.slice(0, 3), [0, 0, -1]);
});

test("triangles without indices are read three vertices at a time", () => {
  const positions = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1]);
  const result = core.flatTriangles(positions, null);
  close(result.positions, positions);
  close(result.normals.slice(0, 3), [0, 0, 1]);
  close(result.normals.slice(9, 12), [1, 0, 0]);
});

test("matrix product, inverse and projection agree", () => {
  const view = core.lookAt([10, -10, 10], [0, 0, 0], [0, 0, 1]);
  const projection = core.perspective(Math.PI / 4, 1.5, 0.1, 100);
  const viewProj = core.multiply(projection, view);
  const identity = core.multiply(viewProj, core.invert(viewProj));
  close(identity, [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1], 1e-4);
  // 注視点は画面の中央に写る。
  const centre = core.transformPoint(viewProj, [0, 0, 0]);
  close(centre.slice(0, 2), [0, 0]);
  assert.throws(() => core.invert(new Float32Array(16)), /singular/);
});

test("orbit eye keeps the distance and puts +y behind the target at yaw 0", () => {
  const eye = core.orbitEye([1, 2, 3], 10, 0, 0);
  close(eye, [1, -8, 3]);
  const tilted = core.orbitEye([0, 0, 0], 10, 35, 25);
  assert.ok(Math.abs(Math.hypot(...tilted) - 10) < 1e-9);
  assert.ok(tilted[2] > 0);
});

test("fit distance contains the bounding sphere and honours the minimum radius", () => {
  const box = { min: [0, 0, 0], max: [3, 4, 0] };
  const fovy = Math.PI / 3;
  assert.ok(Math.abs(core.fitDistance(box, fovy, 0) - 2.5 / Math.sin(fovy / 2)) < 1e-9);
  assert.ok(Math.abs(core.fitDistance(box, fovy, 10) - 10 / Math.sin(fovy / 2)) < 1e-9);
});

test("fitting field of view is the narrower of the vertical and horizontal", () => {
  const fovy = Math.PI / 3;
  assert.equal(core.fittingFov(fovy, 1.5), fovy);
  const narrow = core.fittingFov(fovy, 0.5);
  assert.ok(Math.abs(Math.tan(narrow / 2) - 0.5 * Math.tan(fovy / 2)) < 1e-12);
  // 縦長の画面では収める距離が長くなる。
  const box = { min: [0, 0, 0], max: [10, 10, 10] };
  assert.ok(core.fitDistance(box, narrow, 0) > core.fitDistance(box, fovy, 0));
});

test("box edges are the 12 axis-aligned edges", () => {
  const edges = core.boxEdges({ min: [0, 0, 0], max: [1, 2, 3] });
  assert.equal(edges.length, 12 * 2 * 3);
  const lengths = [];
  for (let i = 0; i < edges.length; i += 6) {
    lengths.push(Math.hypot(edges[i + 3] - edges[i], edges[i + 4] - edges[i + 1], edges[i + 5] - edges[i + 2]));
  }
  assert.deepEqual(lengths.sort(), [1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3]);
});

test("clip plane keeps the side at or below the value unless flipped", () => {
  const keep = (plane, point) => plane[0] * point[0] + plane[1] * point[1] + plane[2] * point[2] + plane[3] >= 0;
  const plane = core.clipPlane({ axis: "z", value: 5, flip: false });
  assert.ok(keep(plane, [0, 0, 4]));
  assert.ok(!keep(plane, [0, 0, 6]));
  const flipped = core.clipPlane({ axis: "z", value: 5, flip: true });
  assert.ok(!keep(flipped, [0, 0, 4]));
  assert.ok(keep(flipped, [0, 0, 6]));
  assert.ok(keep(core.clipPlane(null), [1e6, -1e6, 1e6]));
});

test("clip quad lies on the section plane and covers the box", () => {
  const quad = core.clipQuad({ axis: "y", value: 4, flip: false }, { min: [0, 0, 0], max: [10, 8, 6] }, 1);
  assert.equal(quad.length, 18);
  for (let i = 0; i < 18; i += 3) {
    assert.equal(quad[i + 1], 4);
    assert.ok([-1, 11].includes(quad[i]) && [-1, 7].includes(quad[i + 2]));
  }
});

test("state round-trips through the URL fragment", () => {
  const hash = "#check=3&loc=1&clip=y%3A12.5%3Aflip&hidden=lid&ghost=tray%2Cboard&status=all&rule=snap_fit&q=hook&sort=rule&view=10%2C-20%2C2";
  const state = core.parseState(hash);
  assert.equal(state.check, 3);
  assert.equal(state.loc, 1);
  assert.deepEqual(state.clip, { axis: "y", value: 12.5, flip: true });
  assert.deepEqual(state.hidden, ["lid"]);
  assert.deepEqual(state.ghost, ["tray", "board"]);
  assert.equal(state.status, "all");
  assert.equal(state.rule, "snap_fit");
  assert.equal(state.q, "hook");
  assert.equal(state.sort, "rule");
  assert.deepEqual([state.yaw, state.pitch, state.zoom], [10, -20, 2]);
  assert.equal(core.formatState(state), hash);
});

test("section coordinates keep sub-micrometre precision in the fragment", () => {
  const state = { ...core.parseState(""), clip: { axis: "x", value: 0.0005, flip: false } };
  const hash = core.formatState(state);
  assert.equal(core.parseState(hash).clip.value, 0.0005);
  assert.equal(core.parseState(core.formatState({ ...state, clip: { axis: "x", value: 12.3456789, flip: false } })).clip.value, 12.345679);
});

test("default state formats to an empty fragment", () => {
  assert.equal(core.formatState(core.parseState("")), "");
  assert.deepEqual(core.parseState(""), { ...core.DEFAULT_STATE, hidden: [], ghost: [] });
});

test("invalid fragment values fall back to the defaults", () => {
  const state = core.parseState("#check=-1&loc=2&clip=w:3&status=bogus&sort=size&view=a,200,-1");
  assert.equal(state.check, null);
  assert.equal(state.loc, null);
  assert.equal(state.clip, null);
  assert.equal(state.status, "problems");
  assert.equal(state.sort, "severity");
  assert.equal(state.yaw, core.DEFAULT_STATE.yaw);
  assert.equal(state.pitch, 89);
  assert.equal(state.zoom, 1);
  // locはcheckを選んでいる場合だけ意味を持つ。
  assert.equal(core.parseState("#loc=0").loc, null);
});

const CHECKS = [
  { index: 0, rule: "valid_solid", target: "tray", status: "pass", message: "ok", locations: [] },
  { index: 1, rule: "part_interference", target: "tray/lid", status: "fail", message: "overlap 264", locations: [{}, {}] },
  { index: 2, rule: "mesh_volume", target: "model", status: "not_evaluated", message: "export only", locations: [] },
  { index: 3, rule: "access_clearance", target: "cable/tray", status: "fail", message: "Overlap 100", locations: [{}] },
];

test("filter by status, rule and case-insensitive text", () => {
  const ids = (checks) => checks.map((c) => c.index);
  assert.deepEqual(ids(core.filterChecks(CHECKS, { status: "problems" })), [1, 2, 3]);
  assert.deepEqual(ids(core.filterChecks(CHECKS, { status: "fail" })), [1, 3]);
  assert.deepEqual(ids(core.filterChecks(CHECKS, { status: "pass" })), [0]);
  assert.deepEqual(ids(core.filterChecks(CHECKS, { status: "all", rule: "access_clearance" })), [3]);
  assert.deepEqual(ids(core.filterChecks(CHECKS, { status: "all", q: "OVERLAP" })), [1, 3]);
  assert.deepEqual(ids(core.filterChecks(CHECKS, { status: "all", q: "tray" })), [0, 1, 3]);
});

test("plan filter shows adopted plans by default", () => {
  const planned = [
    { index: 0, rule: "neck_section", target: "lid", status: "pass", message: "", locations: [] },
    { index: 1, rule: "support_free", target: "lid", status: "pass", message: "", locations: [], plan: "fdm", adopted: true },
    { index: 2, rule: "support_free", target: "lid", status: "fail", message: "", locations: [], plan: "resin", adopted: false },
    { index: 3, rule: "resin_suction", target: "lid", status: "fail", message: "", locations: [], plan: "resin", adopted: false },
  ];
  const ids = (filter) => core.filterChecks(planned, { status: "all", ...filter }).map((c) => c.index);
  assert.deepEqual(ids({}), [0, 1]);
  assert.deepEqual(ids({ plan: "resin" }), [0, 2, 3]);
  assert.deepEqual(ids({ plan: "*" }), [0, 1, 2, 3]);
  const state = { ...core.parseState(""), plan: "resin" };
  assert.equal(core.formatState(state), "#plan=resin");
  assert.equal(core.parseState("#plan=resin").plan, "resin");
});

test("sorting is stable on the report order", () => {
  const ids = (checks) => checks.map((c) => c.index);
  assert.deepEqual(ids(core.sortChecks(CHECKS, "severity")), [1, 3, 2, 0]);
  assert.deepEqual(ids(core.sortChecks(CHECKS, "rule")), [3, 2, 1, 0]);
  assert.deepEqual(ids(core.sortChecks(CHECKS, "target")), [3, 2, 0, 1]);
  assert.deepEqual(ids(core.sortChecks(CHECKS, "locations")), [1, 3, 0, 2]);
  assert.deepEqual(ids(core.sortChecks(CHECKS, "unknown")), [1, 3, 2, 0]);
});

test("ray picks the nearest box it enters", () => {
  const ray = { origin: [0, -10, 0], direction: [0, 1, 0] };
  const near = { check: 1, loc: 0, box: { min: [-1, -2, -1], max: [1, 0, 1] } };
  const far = { check: 3, loc: 0, box: { min: [-1, 4, -1], max: [1, 6, 1] } };
  const aside = { check: 5, loc: 0, box: { min: [5, -2, 5], max: [6, 0, 6] } };
  const hit = core.pickLocation(ray, [far, aside, near]);
  assert.equal(hit.check, 1);
  assert.equal(hit.distance, 8);
  assert.equal(core.pickLocation(ray, [aside]), null);
  // 箱の内側から出る視線は距離0で当たる。
  assert.equal(core.rayBoxDistance({ origin: [0, 5, 0], direction: [0, 1, 0] }, far.box), 0);
  assert.equal(core.rayBoxDistance({ origin: [0, 10, 0], direction: [0, 1, 0] }, far.box), null);
});

test("screen ray through the centre passes through the target", () => {
  const view = core.lookAt([0, -20, 0], [0, 0, 0], [0, 0, 1]);
  const viewProj = core.multiply(core.perspective(Math.PI / 4, 2, 0.1, 100), view);
  const ray = core.screenRay(200, 100, 400, 200, core.invert(viewProj));
  close(ray.direction, [0, 1, 0], 1e-4);
  const box = { min: [-1, -1, -1], max: [1, 1, 1] };
  assert.ok(core.rayBoxDistance(ray, box) !== null);
});

test("union of boxes and box centre", () => {
  const union = core.unionBoxes([{ min: [0, 0, 0], max: [1, 1, 1] }, { min: [-1, 2, 0], max: [0, 3, 4] }]);
  assert.deepEqual(union, { min: [-1, 0, 0], max: [1, 3, 4] });
  assert.deepEqual(core.boxCentre(union), [0, 1.5, 2]);
  assert.deepEqual(core.unionBoxes([]), { min: [0, 0, 0], max: [0, 0, 0] });
});

test("analysis fields are dequantized and expanded like the triangles", () => {
  const field = { minimum: 10, maximum: 20 };
  const codes = base64Of([0, 65534, 32767, 65535], Uint16Array);
  const values = core.dequantize(field, core.decodeUint16(codes));
  close(values.slice(0, 3), [10, 20, 15], 1e-3);
  assert.ok(Number.isNaN(values[3]));
  assert.deepEqual([...core.expandPerVertex(new Uint8Array([5, 6, 7]), new Uint32Array([2, 0, 1, 1]))], [7, 5, 6, 6]);
  assert.deepEqual([...core.expandPerVertex(new Uint8Array([5, 6]), null)], [5, 6]);
});

test("field scales follow the chosen range", () => {
  const field = { minimum: 2, maximum: 30, limit: 20, reference: 0 };
  assert.deepEqual(core.fieldScale(field, "limit", null), { low: 0, high: 20, over: true });
  assert.deepEqual(core.fieldScale(field, "data", null), { low: 2, high: 30, over: false });
  // 上限を中点とし、上限から遠い方 (基準値の0、20離れる) までを両側に取る。
  assert.deepEqual(core.fieldScale(field, "diverging", null), { low: 0, high: 40, over: false });
  assert.deepEqual(core.fieldScale(field, "custom", [5, 8]), { low: 5, high: 8, over: false });
  // 上限の無い量と、不正な数値の範囲は、値の最小から最大までとする。
  assert.deepEqual(core.fieldScale({ ...field, limit: null }, "limit", null), { low: 2, high: 30, over: false });
  assert.deepEqual(core.fieldScale(field, "custom", [8, 5]), { low: 2, high: 30, over: false });
});

test("field colours mark values over the limit and leave missing values to the part", () => {
  const scale = { low: 0, high: 20, over: true };
  assert.equal(core.fieldColour(Number.NaN, scale, "sequential"), null);
  assert.equal(core.rgbToHex(core.fieldColour(25, scale, "sequential")), core.OVER_LIMIT);
  assert.equal(core.rgbToHex(core.fieldColour(0, scale, "sequential")), core.COLOURMAPS.sequential.stops[0]);
  assert.equal(core.rgbToHex(core.fieldColour(20, scale, "sequential")), core.COLOURMAPS.sequential.stops.at(-1));
  // 発散色の中点は灰。
  assert.equal(core.rgbToHex(core.colourAt("diverging", 0.5)), "#f0efec");
});

test("colour maps keep the lightness monotone on each arm", () => {
  const lightness = (map, t) => core.rgbToOklab(core.colourAt(map, t))[0];
  for (let i = 0; i < 20; i += 1) {
    assert.ok(lightness("sequential", i / 20) > lightness("sequential", (i + 1) / 20));
  }
  for (let i = 0; i < 10; i += 1) {
    // 中点から両端へ暗くなる。
    assert.ok(lightness("diverging", 0.5 - i / 20) > lightness("diverging", 0.5 - (i + 1) / 20));
    assert.ok(lightness("diverging", 0.5 + i / 20) > lightness("diverging", 0.5 + (i + 1) / 20));
  }
  // 発散色の両端は同じ明度に揃える。
  assert.ok(Math.abs(lightness("diverging", 0) - lightness("diverging", 1)) < 0.01);
});

test("the field view round-trips through the URL fragment", () => {
  const state = { ...core.parseState(""), field: 2, map: "diverging", scale: "custom", range: [1.5, 9] };
  const parsed = core.parseState(core.formatState(state));
  assert.equal(parsed.field, 2);
  assert.equal(parsed.map, "diverging");
  assert.equal(parsed.scale, "custom");
  assert.deepEqual(parsed.range, [1.5, 9]);
  const invalid = core.parseState("#field=-1&map=rainbow&scale=log&range=9,1");
  assert.equal(invalid.field, null);
  assert.equal(invalid.map, core.DEFAULT_STATE.map);
  assert.equal(invalid.scale, core.DEFAULT_STATE.scale);
  assert.equal(invalid.range, null);
  assert.equal(core.formatState(core.parseState("")), "");
});
