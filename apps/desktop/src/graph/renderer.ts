/**
 * Bare WebGL2 renderer for massive static graphs.
 *
 * Draw architecture (3 draw calls total, O(1) regardless of graph size):
 *   1. edges  — one `drawElements(GL_LINES)` over a Uint32 index buffer that
 *               shares the node position VBO (2M vertices for 1M edges).
 *   2. nodes  — one `drawArraysInstanced(TRIANGLE_STRIP, 4, N)`; each
 *               instance is a quad, the fragment shader carves an
 *               anti-aliased circle with smoothstep.
 *   3. glows  — tiny instanced pass (≤64 instances) for hover / selection /
 *               beacon highlights with a soft halo.
 *
 * The camera transform lives entirely in shader uniforms (scale + translate),
 * so pan/zoom never touches vertex data.
 */

import type { Camera } from "./camera";
import type { GraphData } from "./format";
import type { ResolvedTheme } from "../theme";

/* ---- palette (light theme, low saturation) ---- */

const CLASS_COLORS: Record<string, string> = {
  Function: "#6b96e8",
  Method: "#62aebe",
  Class: "#a78bdb",
  Interface: "#d9a75e",
  File: "#9aa7b8",
  Struct: "#7fb89a",
  Enum: "#c893b4",
  Trait: "#a9a1d6",
  /* 合成节点(无源码位置)用区别于真实符号的色相:流程绿、社区灰紫 */
  Process: "#6fb287",
  Community: "#b3a8c9",
};
const FALLBACK_COLOR = "#8fa0b8";
export const ACCENT = "#2e7cf6";
export const BEACON = "#f6a623";

const MAX_PALETTE = 16;
const MAX_OVERLAY = 64;
const OVERLAY_STRIDE = 7; /* x, y, rPx, r, g, b, intensity */

const THEME_COLORS = {
  light: {
    bg: [246 / 255, 248 / 255, 251 / 255] as const,
    edge: [70 / 255, 90 / 255, 120 / 255] as const,
  },
  dark: {
    bg: [16 / 255, 21 / 255, 29 / 255] as const,
    edge: [116 / 255, 135 / 255, 160 / 255] as const,
  },
};

/* ---- shaders ---- */

const NODE_VS = `#version 300 es
layout(location=0) in vec2 aCorner;
layout(location=1) in vec2 aPos;
layout(location=2) in float aSize;
layout(location=3) in float aCls;
uniform vec2 uViewport;
uniform float uScale;
uniform vec2 uTranslate;
uniform float uMinPx;
uniform float uMaxPx;
uniform vec3 uPalette[${MAX_PALETTE}];
out vec2 vOffset;
out float vRadius;
out vec3 vColor;
void main() {
  vec2 screen = aPos * uScale + uTranslate;
  float rPx = clamp(aSize * uScale * 0.5, uMinPx, uMaxPx);
  vec2 cornerPx = aCorner * (rPx + 1.0);
  vec2 p = (screen + cornerPx) / uViewport * 2.0 - 1.0;
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  vOffset = cornerPx;
  vRadius = rPx;
  vColor = uPalette[int(aCls)];
}`;

const NODE_FS = `#version 300 es
precision mediump float;
in vec2 vOffset;
in float vRadius;
in vec3 vColor;
uniform float uAlpha;
out vec4 outColor;
void main() {
  float d = length(vOffset);
  float aa = smoothstep(vRadius + 0.7, vRadius - 0.7, d);
  if (aa < 0.004) discard;
  outColor = vec4(vColor, aa * uAlpha);
}`;

const EDGE_VS = `#version 300 es
layout(location=0) in vec2 aPos;
uniform vec2 uViewport;
uniform float uScale;
uniform vec2 uTranslate;
void main() {
  vec2 screen = aPos * uScale + uTranslate;
  vec2 p = screen / uViewport * 2.0 - 1.0;
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
}`;

const EDGE_FS = `#version 300 es
precision mediump float;
uniform vec4 uColor;
out vec4 outColor;
void main() {
  outColor = uColor;
}`;

const GLOW_VS = `#version 300 es
layout(location=0) in vec2 aCorner;
layout(location=1) in vec2 aPos;     /* world */
layout(location=2) in float aRadius; /* px */
layout(location=3) in vec3 aColor;
layout(location=4) in float aIntensity;
uniform vec2 uViewport;
uniform float uScale;
uniform vec2 uTranslate;
out vec2 vOffset;
out float vRadius;
out vec3 vColor;
out float vIntensity;
void main() {
  vec2 screen = aPos * uScale + uTranslate;
  float ext = aRadius * 3.2 + 8.0;
  vec2 cornerPx = aCorner * ext;
  vec2 p = (screen + cornerPx) / uViewport * 2.0 - 1.0;
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  vOffset = cornerPx;
  vRadius = aRadius;
  vColor = aColor;
  vIntensity = aIntensity;
}`;

const GLOW_FS = `#version 300 es
precision mediump float;
in vec2 vOffset;
in float vRadius;
in vec3 vColor;
in float vIntensity;
out vec4 outColor;
void main() {
  float d = length(vOffset);
  /* core disc */
  float core = smoothstep(vRadius + 0.7, vRadius - 0.7, d);
  /* ring just outside the disc */
  float ring = smoothstep(vRadius + 2.6, vRadius + 1.0, d) *
               (1.0 - smoothstep(vRadius + 1.0, vRadius - 0.4, d));
  /* soft halo */
  float halo = (1.0 - smoothstep(vRadius, vRadius * 3.2 + 8.0, d));
  float a = core * 0.92 + ring * 0.5 + halo * halo * 0.22;
  a *= vIntensity;
  if (a < 0.004) discard;
  outColor = vec4(vColor, a);
}`;

export interface LodParams {
  level: 0 | 1 | 2;
  z: number;
  minPx: number;
  maxPx: number;
  edgeAlpha: number;
  /**
   * Fraction of the edge index buffer drawn this frame (prefix sample).
   * Far zoom shows every edge as overlapping blended fragments anyway, so we
   * draw a random subset with compensated alpha — same nebula, far less fill.
   * Assumes edge order is unbiased; real snapshots are shuffled once at load.
   */
  edgeFraction: number;
}

export function computeLod(z: number): LodParams {
  const level: 0 | 1 | 2 = z < 3 ? 0 : z < 12 ? 1 : 2;
  const lz = Math.log2(Math.max(z, 1));
  const edgeFraction = 0.3 + 0.7 * smoothstep(1.5, 6, z);
  const baseAlpha = clamp(0.035 + 0.03 * lz, 0.03, 0.16);
  return {
    level,
    z,
    minPx: clamp(0.7 + 0.4 * lz, 0.7, 2.4),
    maxPx: clamp(2.2 * Math.max(z, 1), 2.2, 26),
    edgeAlpha: clamp(baseAlpha / edgeFraction, baseAlpha, 0.2),
    edgeFraction,
  };
}

export interface OverlayItem {
  /** node index */
  i: number;
  color: [number, number, number];
  intensity: number;
}

export class GraphRenderer {
  private gl: WebGL2RenderingContext;
  private canvas: HTMLCanvasElement;

  private nodeProgram: WebGLProgram;
  private edgeProgram: WebGLProgram;
  private glowProgram: WebGLProgram;

  private nodeVao: WebGLVertexArrayObject;
  private edgeVao: WebGLVertexArrayObject;
  private glowVao: WebGLVertexArrayObject;

  private glowBuffer: WebGLBuffer;
  private glowData = new Float32Array(MAX_OVERLAY * OVERLAY_STRIDE);

  private uniforms: Record<string, Record<string, WebGLUniformLocation | null>> = {};

  private data: GraphData | null = null;
  private palette = new Float32Array(MAX_PALETTE * 3);
  private ownedBuffers: WebGLBuffer[] = [];

  private dpr = 1;
  cssWidth = 1;
  cssHeight = 1;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    const gl = canvas.getContext("webgl2", {
      alpha: false,
      antialias: false,
      depth: false,
      stencil: false,
      powerPreference: "high-performance",
      preserveDrawingBuffer: false,
    });
    if (!gl) throw new Error("WebGL2 not available");
    this.gl = gl;

    this.nodeProgram = buildProgram(gl, NODE_VS, NODE_FS);
    this.edgeProgram = buildProgram(gl, EDGE_VS, EDGE_FS);
    this.glowProgram = buildProgram(gl, GLOW_VS, GLOW_FS);
    for (const [name, prog] of [
      ["node", this.nodeProgram],
      ["edge", this.edgeProgram],
      ["glow", this.glowProgram],
    ] as const) {
      const locs: Record<string, WebGLUniformLocation | null> = {};
      for (const u of [
        "uViewport",
        "uScale",
        "uTranslate",
        "uMinPx",
        "uMaxPx",
        "uPalette",
        "uAlpha",
        "uColor",
      ]) {
        locs[u] = gl.getUniformLocation(prog, u);
      }
      this.uniforms[name] = locs;
    }

    this.nodeVao = gl.createVertexArray()!;
    this.edgeVao = gl.createVertexArray()!;
    this.glowVao = gl.createVertexArray()!;
    this.glowBuffer = gl.createBuffer()!;

    gl.disable(gl.DEPTH_TEST);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
  }

  setData(data: GraphData) {
    const gl = this.gl;
    this.data = data;

    /* palette from class names */
    this.palette.fill(0.6);
    data.classNames.slice(0, MAX_PALETTE).forEach((cls, idx) => {
      const [r, g, b] = hexToRgb(CLASS_COLORS[cls] ?? FALLBACK_COLOR);
      this.palette[idx * 3] = r;
      this.palette[idx * 3 + 1] = g;
      this.palette[idx * 3 + 2] = b;
    });

    for (const b of this.ownedBuffers) gl.deleteBuffer(b);
    this.ownedBuffers.length = 0;
    const own = <T extends WebGLBuffer>(b: T): T => {
      this.ownedBuffers.push(b);
      return b;
    };

    const quad = new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]);
    const quadBuffer = own(gl.createBuffer()!);
    gl.bindBuffer(gl.ARRAY_BUFFER, quadBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, quad, gl.STATIC_DRAW);

    const posBuffer = own(gl.createBuffer()!);
    gl.bindBuffer(gl.ARRAY_BUFFER, posBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, data.positions, gl.STATIC_DRAW);

    const sizeBuffer = own(gl.createBuffer()!);
    gl.bindBuffer(gl.ARRAY_BUFFER, sizeBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, data.sizes, gl.STATIC_DRAW);

    const clsBuffer = own(gl.createBuffer()!);
    gl.bindBuffer(gl.ARRAY_BUFFER, clsBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, data.classes, gl.STATIC_DRAW);

    /* ---- node VAO: quad corner + per-instance pos/size/class ---- */
    gl.bindVertexArray(this.nodeVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, quadBuffer);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.bindBuffer(gl.ARRAY_BUFFER, posBuffer);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 0, 0);
    gl.vertexAttribDivisor(1, 1);
    gl.bindBuffer(gl.ARRAY_BUFFER, sizeBuffer);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 1, gl.FLOAT, false, 0, 0);
    gl.vertexAttribDivisor(2, 1);
    gl.bindBuffer(gl.ARRAY_BUFFER, clsBuffer);
    gl.enableVertexAttribArray(3);
    gl.vertexAttribPointer(3, 1, gl.UNSIGNED_BYTE, false, 0, 0);
    gl.vertexAttribDivisor(3, 1);

    /* ---- edge VAO: node positions + element index buffer ---- */
    gl.bindVertexArray(this.edgeVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, posBuffer);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    const edgeIndexBuffer = own(gl.createBuffer()!);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, edgeIndexBuffer);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, data.edges, gl.STATIC_DRAW);

    /* ---- glow VAO: quad corner + dynamic per-instance data ---- */
    gl.bindVertexArray(this.glowVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, quadBuffer);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.glowBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, this.glowData.byteLength, gl.DYNAMIC_DRAW);
    const stride = OVERLAY_STRIDE * 4;
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, stride, 0);
    gl.vertexAttribDivisor(1, 1);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 1, gl.FLOAT, false, stride, 8);
    gl.vertexAttribDivisor(2, 1);
    gl.enableVertexAttribArray(3);
    gl.vertexAttribPointer(3, 3, gl.FLOAT, false, stride, 12);
    gl.vertexAttribDivisor(3, 1);
    gl.enableVertexAttribArray(4);
    gl.vertexAttribPointer(4, 1, gl.FLOAT, false, stride, 24);
    gl.vertexAttribDivisor(4, 1);

    gl.bindVertexArray(null);
  }

  resize(cssWidth: number, cssHeight: number, dpr: number) {
    this.cssWidth = Math.max(1, cssWidth);
    this.cssHeight = Math.max(1, cssHeight);
    this.dpr = dpr;
    const w = Math.round(this.cssWidth * dpr);
    const h = Math.round(this.cssHeight * dpr);
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
  }

  render(
    camera: Camera,
    lod: LodParams,
    overlay: OverlayItem[],
    theme: ResolvedTheme = "light",
  ) {
    const gl = this.gl;
    const data = this.data;
    const themeColors = THEME_COLORS[theme];
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(themeColors.bg[0], themeColors.bg[1], themeColors.bg[2], 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    if (!data) return;

    const vw = this.cssWidth;
    const vh = this.cssHeight;

    /* ---- edges ---- */
    if (lod.edgeAlpha > 0.002) {
      gl.useProgram(this.edgeProgram);
      const u = this.uniforms.edge;
      gl.uniform2f(u.uViewport, vw, vh);
      gl.uniform1f(u.uScale, camera.k);
      gl.uniform2f(u.uTranslate, camera.tx, camera.ty);
      gl.uniform4f(
        u.uColor,
        themeColors.edge[0],
        themeColors.edge[1],
        themeColors.edge[2],
        theme === "dark" ? lod.edgeAlpha * 0.72 : lod.edgeAlpha,
      );
      gl.bindVertexArray(this.edgeVao);
      const drawnEdges = Math.min(
        data.edgeCount,
        Math.ceil(data.edgeCount * lod.edgeFraction),
      );
      gl.drawElements(gl.LINES, drawnEdges * 2, gl.UNSIGNED_INT, 0);
    }

    /* ---- nodes ---- */
    gl.useProgram(this.nodeProgram);
    const un = this.uniforms.node;
    gl.uniform2f(un.uViewport, vw, vh);
    gl.uniform1f(un.uScale, camera.k);
    gl.uniform2f(un.uTranslate, camera.tx, camera.ty);
    gl.uniform1f(un.uMinPx, lod.minPx);
    gl.uniform1f(un.uMaxPx, lod.maxPx);
    gl.uniform1f(un.uAlpha, 0.92);
    gl.uniform3fv(un.uPalette, this.palette);
    gl.bindVertexArray(this.nodeVao);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, data.count);

    /* ---- glow overlay (hover / selection / beacons) ---- */
    const n = Math.min(overlay.length, MAX_OVERLAY);
    if (n > 0) {
      for (let j = 0; j < n; j++) {
        const item = overlay[j];
        const i = item.i;
        const base = j * OVERLAY_STRIDE;
        const rPx = clamp(
          data.sizes[i] * camera.k * 0.5,
          lod.minPx,
          lod.maxPx,
        );
        this.glowData[base] = data.positions[i * 2];
        this.glowData[base + 1] = data.positions[i * 2 + 1];
        this.glowData[base + 2] = rPx;
        this.glowData[base + 3] = item.color[0];
        this.glowData[base + 4] = item.color[1];
        this.glowData[base + 5] = item.color[2];
        this.glowData[base + 6] = item.intensity;
      }
      gl.useProgram(this.glowProgram);
      const ug = this.uniforms.glow;
      gl.uniform2f(ug.uViewport, vw, vh);
      gl.uniform1f(ug.uScale, camera.k);
      gl.uniform2f(ug.uTranslate, camera.tx, camera.ty);
      gl.bindVertexArray(this.glowVao);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.glowBuffer);
      gl.bufferSubData(
        gl.ARRAY_BUFFER,
        0,
        this.glowData.subarray(0, n * OVERLAY_STRIDE),
      );
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, n);
    }

    gl.bindVertexArray(null);
  }

  clearData() {
    const gl = this.gl;
    this.data = null;
    for (const b of this.ownedBuffers) gl.deleteBuffer(b);
    this.ownedBuffers.length = 0;
  }

  get devicePixelRatio() {
    return this.dpr;
  }

  destroy() {
    /* NOTE: do NOT loseContext() here — under React StrictMode the effect
       re-mounts on the same canvas and a lost context cannot be re-created.
       Just release the GPU objects we own. */
    const gl = this.gl;
    gl.deleteProgram(this.nodeProgram);
    gl.deleteProgram(this.edgeProgram);
    gl.deleteProgram(this.glowProgram);
    gl.deleteVertexArray(this.nodeVao);
    gl.deleteVertexArray(this.edgeVao);
    gl.deleteVertexArray(this.glowVao);
    gl.deleteBuffer(this.glowBuffer);
    for (const b of this.ownedBuffers) gl.deleteBuffer(b);
    this.ownedBuffers.length = 0;
  }
}

/* ---- helpers ---- */

function buildProgram(
  gl: WebGL2RenderingContext,
  vsSource: string,
  fsSource: string,
): WebGLProgram {
  const vs = compile(gl, gl.VERTEX_SHADER, vsSource);
  const fs = compile(gl, gl.FRAGMENT_SHADER, fsSource);
  const prog = gl.createProgram()!;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    throw new Error(`program link failed: ${gl.getProgramInfoLog(prog)}`);
  }
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  return prog;
}

function compile(
  gl: WebGL2RenderingContext,
  type: number,
  source: string,
): WebGLShader {
  const shader = gl.createShader(type)!;
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(`shader compile failed: ${gl.getShaderInfoLog(shader)}`);
  }
  return shader;
}

export function hexToRgb(hex: string): [number, number, number] {
  const v = parseInt(hex.slice(1), 16);
  return [((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255];
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

function smoothstep(lo: number, hi: number, v: number): number {
  const t = clamp((v - lo) / (hi - lo), 0, 1);
  return t * t * (3 - 2 * t);
}
