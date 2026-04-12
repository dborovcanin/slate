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

function normalizeName(value: string | null | undefined): string {
  return (value ?? "")
    .trim()
    .toLowerCase()
    .replace(/[_\s]+/g, "-");
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
  const root = document.documentElement;

  for (const [name, value] of Object.entries(scheme.vars)) {
    root.style.setProperty(name, value);
  }

  root.style.setProperty("--bg-pattern", background.pattern);
  root.style.setProperty("--bg-pattern-size", background.size);
  root.style.setProperty("--font-mono", font.stack);
  root.style.setProperty("--font-size", `${fontSize}px`);

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
