import { getThemeConfigOrDefault, type ThemeConfig } from "../api";
import {
  BACKGROUND_PRESETS,
  clampFontSize,
  COLOR_SCHEMES,
  DEFAULT_BACKGROUND,
  DEFAULT_COLOR_SCHEME,
  DEFAULT_FONT,
  DEFAULT_FONT_SIZE,
  FONT_IDS,
  FONT_PRESETS,
} from "./presets";

export interface ThemeSelection {
  colorScheme: string;
  background: string;
  font: string;
  fontSize: number;
}

let currentSelection: ThemeSelection | null = null;
let reloadTimer: number | null = null;
let reloadInFlight = false;
const LIVE_RELOAD_INTERVAL_MS = 1200;

interface Rgb {
  r: number;
  g: number;
  b: number;
}

const FALLBACK_BG: Rgb = { r: 30, g: 30, b: 46 };
const FALLBACK_FG: Rgb = { r: 205, g: 214, b: 244 };
const FALLBACK_ACCENT: Rgb = { r: 137, g: 180, b: 250 };
const FALLBACK_FG_DIM: Rgb = { r: 127, g: 132, b: 156 };
const BASE_LINE_HEIGHT_RATIO = 1.65;

function clampChannel(value: number): number {
  return Math.max(0, Math.min(255, Math.round(value)));
}

function parseCssColor(value: string | undefined): Rgb | null {
  if (!value) return null;
  const text = value.trim();
  const hex = text.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (hex) {
    const raw = hex[1];
    if (raw.length === 3) {
      return {
        r: Number.parseInt(raw[0] + raw[0], 16),
        g: Number.parseInt(raw[1] + raw[1], 16),
        b: Number.parseInt(raw[2] + raw[2], 16),
      };
    }
    return {
      r: Number.parseInt(raw.slice(0, 2), 16),
      g: Number.parseInt(raw.slice(2, 4), 16),
      b: Number.parseInt(raw.slice(4, 6), 16),
    };
  }

  const rgb = text.match(/^rgba?\(([^)]+)\)$/i);
  if (rgb) {
    const parts = rgb[1]?.split(",").map((entry) => Number.parseFloat(entry.trim())) ?? [];
    if (parts.length >= 3 && parts.every((entry, idx) => idx < 3 && Number.isFinite(entry))) {
      return {
        r: clampChannel(parts[0] ?? 0),
        g: clampChannel(parts[1] ?? 0),
        b: clampChannel(parts[2] ?? 0),
      };
    }
  }

  return null;
}

function toHex(color: Rgb): string {
  const asHex = (value: number) => clampChannel(value).toString(16).padStart(2, "0");
  return `#${asHex(color.r)}${asHex(color.g)}${asHex(color.b)}`;
}

function mix(a: Rgb, b: Rgb, t: number): Rgb {
  return {
    r: a.r + (b.r - a.r) * t,
    g: a.g + (b.g - a.g) * t,
    b: a.b + (b.b - a.b) * t,
  };
}

function luminance(rgb: Rgb): number {
  const channel = (v: number) => {
    const n = v / 255;
    return n <= 0.03928 ? n / 12.92 : ((n + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b);
}

function rgbToHsl(rgb: Rgb): { h: number; s: number; l: number } {
  const r = rgb.r / 255;
  const g = rgb.g / 255;
  const b = rgb.b / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  if (d === 0) return { h: 0, s: 0, l };

  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h = 0;
  switch (max) {
    case r:
      h = (g - b) / d + (g < b ? 6 : 0);
      break;
    case g:
      h = (b - r) / d + 2;
      break;
    case b:
      h = (r - g) / d + 4;
      break;
  }
  h /= 6;
  return { h: h * 360, s, l };
}

function hslToRgb(h: number, s: number, l: number): Rgb {
  const hue = (((h % 360) + 360) % 360) / 360;
  if (s === 0) {
    const v = l * 255;
    return { r: v, g: v, b: v };
  }

  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  const hueToChannel = (t: number) => {
    let value = t;
    if (value < 0) value += 1;
    if (value > 1) value -= 1;
    if (value < 1 / 6) return p + (q - p) * 6 * value;
    if (value < 1 / 2) return q;
    if (value < 2 / 3) return p + (q - p) * (2 / 3 - value) * 6;
    return p;
  };

  return {
    r: hueToChannel(hue + 1 / 3) * 255,
    g: hueToChannel(hue) * 255,
    b: hueToChannel(hue - 1 / 3) * 255,
  };
}

function shiftHue(rgb: Rgb, degrees: number): Rgb {
  const hsl = rgbToHsl(rgb);
  return hslToRgb(hsl.h + degrees, hsl.s, hsl.l);
}

function deriveCodePalette(vars: Record<string, string>): Record<string, string> {
  const bg = parseCssColor(vars["--bg"]) ?? FALLBACK_BG;
  const bgSurface = parseCssColor(vars["--bg-surface"]) ?? mix(bg, FALLBACK_FG, 0.06);
  const fg = parseCssColor(vars["--fg"]) ?? FALLBACK_FG;
  const fgDim = parseCssColor(vars["--fg-dim"]) ?? FALLBACK_FG_DIM;
  const accent = parseCssColor(vars["--accent"]) ?? FALLBACK_ACCENT;
  const dark = luminance(bg) < 0.45;
  const pole: Rgb = dark ? { r: 255, g: 255, b: 255 } : { r: 0, g: 0, b: 0 };

  const adapt = (color: Rgb, amount: number) => mix(color, pole, amount);
  const keyword = adapt(accent, dark ? 0.12 : 0.28);
  const string = adapt(shiftHue(accent, dark ? 96 : 84), dark ? 0.08 : 0.32);
  const number = adapt(shiftHue(accent, dark ? -84 : -72), dark ? 0.05 : 0.28);
  const func = adapt(shiftHue(accent, dark ? 42 : 30), dark ? 0.08 : 0.25);
  const type = adapt(shiftHue(accent, dark ? -38 : -25), dark ? 0.08 : 0.26);
  const comment = mix(fgDim, bg, dark ? 0.12 : 0.04);
  const codeBg = mix(bgSurface, bg, dark ? 0.4 : 0.12);
  const codeBorder = mix(accent, bg, dark ? 0.7 : 0.58);

  return {
    "--code-token-keyword": vars["--code-token-keyword"] ?? toHex(keyword),
    "--code-token-string": vars["--code-token-string"] ?? toHex(string),
    "--code-token-number": vars["--code-token-number"] ?? toHex(number),
    "--code-token-function": vars["--code-token-function"] ?? toHex(func),
    "--code-token-type": vars["--code-token-type"] ?? toHex(type),
    "--code-token-comment": vars["--code-token-comment"] ?? toHex(comment),
    "--code-bg": vars["--code-bg"] ?? toHex(codeBg),
    "--code-border": vars["--code-border"] ?? toHex(codeBorder),
    "--code-fg": vars["--code-fg"] ?? toHex(fg),
  };
}

function normalizeName(value: string | null | undefined): string {
  return (value ?? "")
    .trim()
    .toLowerCase()
    .replace(/[_\s]+/g, "-");
}

function computeEditorLineHeightPx(fontSize: number): number {
  return Math.max(fontSize + 4, Math.round(fontSize * BASE_LINE_HEIGHT_RATIO));
}

function scaledPx(basePx: number, ratio: number, minPx = basePx): number {
  return Math.max(minPx, Math.round(basePx * ratio));
}

function computeHeadingMetrics(fontSize: number, lineHeightPx: number) {
  const heading1Size = scaledPx(fontSize, 1.75, fontSize + 4);
  const heading2Size = scaledPx(fontSize, 1.5, fontSize + 3);
  const heading3Size = scaledPx(fontSize, 1.3, fontSize + 2);
  const heading4Size = scaledPx(fontSize, 1.15, fontSize + 1);
  const heading5Size = scaledPx(fontSize, 1.05, fontSize + 1);
  const heading6Size = fontSize;

  return {
    heading1Size,
    heading2Size,
    heading3Size,
    heading4Size,
    heading5Size,
    heading6Size,
    heading1LineHeight: scaledPx(heading1Size, 1.3, heading1Size + 4),
    heading2LineHeight: scaledPx(heading2Size, 1.35, heading2Size + 3),
    headingLineHeight: lineHeightPx,
  };
}

export function resolveThemeSelection(input: Partial<ThemeConfig>): ThemeSelection {
  const colorSchemeKey = normalizeName(input.color_scheme);
  const backgroundKey = normalizeName(input.background);
  const fontKey = normalizeName(input.font);
  const fontSize =
    typeof input.font_size === "number" ? clampFontSize(input.font_size) : DEFAULT_FONT_SIZE;

  return {
    colorScheme: COLOR_SCHEMES[colorSchemeKey] ? colorSchemeKey : DEFAULT_COLOR_SCHEME,
    background: BACKGROUND_PRESETS[backgroundKey] ? backgroundKey : DEFAULT_BACKGROUND,
    font: FONT_PRESETS[fontKey] ? fontKey : DEFAULT_FONT,
    fontSize,
  };
}

export function applyTheme(selection: ThemeSelection) {
  const scheme = COLOR_SCHEMES[selection.colorScheme] ?? COLOR_SCHEMES[DEFAULT_COLOR_SCHEME];
  const background =
    BACKGROUND_PRESETS[selection.background] ?? BACKGROUND_PRESETS[DEFAULT_BACKGROUND];
  const font = FONT_PRESETS[selection.font] ?? FONT_PRESETS[DEFAULT_FONT];
  const fontSize = clampFontSize(selection.fontSize);
  const lineHeightPx = computeEditorLineHeightPx(fontSize);
  const heading = computeHeadingMetrics(fontSize, lineHeightPx);
  const root = document.documentElement;

  for (const [name, value] of Object.entries(scheme.vars)) {
    root.style.setProperty(name, value);
  }

  const codePalette = deriveCodePalette(scheme.vars);
  for (const [name, value] of Object.entries(codePalette)) {
    root.style.setProperty(name, value);
  }

  root.style.setProperty("--bg-pattern", background.pattern);
  root.style.setProperty("--bg-pattern-size", background.size);
  root.style.setProperty("--font-mono", font.stack);
  root.style.setProperty("--font-size", `${fontSize}px`);
  root.style.setProperty("--line-height", `${lineHeightPx}px`);
  root.style.setProperty("--heading-1-size", `${heading.heading1Size}px`);
  root.style.setProperty("--heading-2-size", `${heading.heading2Size}px`);
  root.style.setProperty("--heading-3-size", `${heading.heading3Size}px`);
  root.style.setProperty("--heading-4-size", `${heading.heading4Size}px`);
  root.style.setProperty("--heading-5-size", `${heading.heading5Size}px`);
  root.style.setProperty("--heading-6-size", `${heading.heading6Size}px`);
  root.style.setProperty("--heading-1-line-height", `${heading.heading1LineHeight}px`);
  root.style.setProperty("--heading-2-line-height", `${heading.heading2LineHeight}px`);
  root.style.setProperty("--heading-line-height", `${heading.headingLineHeight}px`);

  root.dataset.theme = scheme.id;
  root.dataset.background = background.id;
  root.dataset.font = font.id;

  const applied = {
    colorScheme: scheme.id,
    background: background.id,
    font: font.id,
    fontSize,
  };
  currentSelection = applied;
  return applied;
}

function sameSelection(a: ThemeSelection | null, b: ThemeSelection): boolean {
  return (
    !!a &&
    a.colorScheme === b.colorScheme &&
    a.background === b.background &&
    a.font === b.font &&
    a.fontSize === b.fontSize
  );
}

async function fetchSelection() {
  const config = await getThemeConfigOrDefault();
  return resolveThemeSelection(config);
}

async function reloadThemeIfChanged() {
  const selection = await fetchSelection();
  if (sameSelection(currentSelection, selection)) {
    return null;
  }
  return applyTheme(selection);
}

export function getCurrentThemeSelection(): ThemeSelection | null {
  return currentSelection;
}

export function getFontLabel(fontId: string): string {
  return FONT_PRESETS[fontId]?.label ?? fontId;
}

export function changeFontSize(delta: number): ThemeSelection {
  const base = currentSelection ?? resolveThemeSelection({});
  const next = { ...base, fontSize: clampFontSize(base.fontSize + delta) };
  if (sameSelection(currentSelection, next)) {
    return base;
  }
  return applyTheme(next);
}

export function cycleFont(direction: -1 | 1): ThemeSelection {
  const base = currentSelection ?? resolveThemeSelection({});
  const idx = FONT_IDS.indexOf(base.font);
  const currentIdx = idx >= 0 ? idx : FONT_IDS.indexOf(DEFAULT_FONT);
  const nextIdx = (currentIdx + direction + FONT_IDS.length) % FONT_IDS.length;
  const next = { ...base, font: FONT_IDS[nextIdx] };
  return applyTheme(next);
}

export async function loadAndApplyTheme() {
  const config = await getThemeConfigOrDefault();
  applyTheme(resolveThemeSelection(config));
  return config;
}
