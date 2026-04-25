export interface ColorScheme {
  id: string;
  label: string;
  vars: Record<string, string>;
}

export interface BackgroundPreset {
  id: string;
  label: string;
  pattern: string;
  size: string;
}

export interface FontPreset {
  id: string;
  label: string;
  stack: string;
}

export interface DisplayFontPreset {
  id: string;
  label: string;
  stack: string;
}

export const COLOR_SCHEMES: Record<string, ColorScheme> = {
  "catppuccin-mocha": {
    id: "catppuccin-mocha",
    label: "Catppuccin Mocha",
    vars: {
      "--bg": "#1e1e2e",
      "--bg-surface": "#252536",
      "--bg-overlay": "#2a2a3c",
      "--fg": "#cdd6f4",
      "--fg-dim": "#7f849c",
      "--fg-muted": "#6c7086",
      "--accent": "#89b4fa",
      "--border": "#363647",
      "--selection-bg": "rgba(137, 180, 250, 0.25)",
      "--active-line-bg": "rgba(255, 255, 255, 0.03)",
      "--pattern-color": "rgba(166, 173, 200, 0.08)",
    },
  },
  "catppuccin-latte": {
    id: "catppuccin-latte",
    label: "Catppuccin Latte",
    vars: {
      "--bg": "#eff1f5",
      "--bg-surface": "#e6e9ef",
      "--bg-overlay": "#dce0e8",
      "--fg": "#4c4f69",
      "--fg-dim": "#6c6f85",
      "--fg-muted": "#7c7f93",
      "--accent": "#1e66f5",
      "--border": "#ccd0da",
      "--selection-bg": "rgba(30, 102, 245, 0.18)",
      "--active-line-bg": "rgba(76, 79, 105, 0.06)",
      "--pattern-color": "rgba(76, 79, 105, 0.08)",
    },
  },
  "gruvbox-dark": {
    id: "gruvbox-dark",
    label: "Gruvbox Dark",
    vars: {
      "--bg": "#282828",
      "--bg-surface": "#32302f",
      "--bg-overlay": "#3c3836",
      "--fg": "#ebdbb2",
      "--fg-dim": "#a89984",
      "--fg-muted": "#928374",
      "--accent": "#fabd2f",
      "--border": "#504945",
      "--selection-bg": "rgba(250, 189, 47, 0.2)",
      "--active-line-bg": "rgba(235, 219, 178, 0.05)",
      "--pattern-color": "rgba(235, 219, 178, 0.08)",
    },
  },
  "gruvbox-light": {
    id: "gruvbox-light",
    label: "Gruvbox Light",
    vars: {
      "--bg": "#fbf1c7",
      "--bg-surface": "#f2e5bc",
      "--bg-overlay": "#ebdbb2",
      "--fg": "#3c3836",
      "--fg-dim": "#665c54",
      "--fg-muted": "#7c6f64",
      "--accent": "#d65d0e",
      "--border": "#d5c4a1",
      "--selection-bg": "rgba(214, 93, 14, 0.2)",
      "--active-line-bg": "rgba(60, 56, 54, 0.05)",
      "--pattern-color": "rgba(60, 56, 54, 0.08)",
    },
  },
  dracula: {
    id: "dracula",
    label: "Dracula",
    vars: {
      "--bg": "#282a36",
      "--bg-surface": "#313442",
      "--bg-overlay": "#3b3f52",
      "--fg": "#f8f8f2",
      "--fg-dim": "#9ca0b5",
      "--fg-muted": "#7f8494",
      "--accent": "#bd93f9",
      "--border": "#44475a",
      "--selection-bg": "rgba(189, 147, 249, 0.22)",
      "--active-line-bg": "rgba(248, 248, 242, 0.04)",
      "--pattern-color": "rgba(248, 248, 242, 0.07)",
    },
  },
  dark: {
    id: "dark",
    label: "Dark",
    vars: {
      "--bg": "#14161a",
      "--bg-surface": "#1a1d23",
      "--bg-overlay": "#22262d",
      "--fg": "#e6edf3",
      "--fg-dim": "#9aa4af",
      "--fg-muted": "#7f8892",
      "--accent": "#4aa8ff",
      "--border": "#30363d",
      "--selection-bg": "rgba(74, 168, 255, 0.2)",
      "--active-line-bg": "rgba(230, 237, 243, 0.04)",
      "--pattern-color": "rgba(230, 237, 243, 0.07)",
    },
  },
  white: {
    id: "white",
    label: "White",
    vars: {
      "--bg": "#ffffff",
      "--bg-surface": "#f8f9fb",
      "--bg-overlay": "#eef1f4",
      "--fg": "#1f2933",
      "--fg-dim": "#6b7785",
      "--fg-muted": "#8793a0",
      "--accent": "#005cc5",
      "--border": "#d8dee4",
      "--selection-bg": "rgba(0, 92, 197, 0.18)",
      "--active-line-bg": "rgba(31, 41, 51, 0.05)",
      "--pattern-color": "rgba(31, 41, 51, 0.08)",
    },
  },
  "solarized-dark": {
    id: "solarized-dark",
    label: "Solarized Dark",
    vars: {
      "--bg": "#002b36",
      "--bg-surface": "#073642",
      "--bg-overlay": "#0a4452",
      "--fg": "#93a1a1",
      "--fg-dim": "#839496",
      "--fg-muted": "#657b83",
      "--accent": "#b58900",
      "--border": "#194754",
      "--selection-bg": "rgba(181, 137, 0, 0.2)",
      "--active-line-bg": "rgba(147, 161, 161, 0.06)",
      "--pattern-color": "rgba(147, 161, 161, 0.08)",
    },
  },
  "solarized-light": {
    id: "solarized-light",
    label: "Solarized Light",
    vars: {
      "--bg": "#fdf6e3",
      "--bg-surface": "#f5efdd",
      "--bg-overlay": "#eee8d5",
      "--fg": "#586e75",
      "--fg-dim": "#657b83",
      "--fg-muted": "#839496",
      "--accent": "#cb4b16",
      "--border": "#ddd6c0",
      "--selection-bg": "rgba(203, 75, 22, 0.18)",
      "--active-line-bg": "rgba(88, 110, 117, 0.06)",
      "--pattern-color": "rgba(88, 110, 117, 0.08)",
    },
  },
  nord: {
    id: "nord",
    label: "Nord",
    vars: {
      "--bg": "#2e3440",
      "--bg-surface": "#3b4252",
      "--bg-overlay": "#434c5e",
      "--fg": "#e5e9f0",
      "--fg-dim": "#aeb8c8",
      "--fg-muted": "#8f9db2",
      "--accent": "#88c0d0",
      "--border": "#4c566a",
      "--selection-bg": "rgba(136, 192, 208, 0.24)",
      "--active-line-bg": "rgba(229, 233, 240, 0.04)",
      "--pattern-color": "rgba(229, 233, 240, 0.07)",
    },
  },
  "tokyo-night": {
    id: "tokyo-night",
    label: "Tokyo Night",
    vars: {
      "--bg": "#1a1b26",
      "--bg-surface": "#212433",
      "--bg-overlay": "#2a2f45",
      "--fg": "#c0caf5",
      "--fg-dim": "#9aa5ce",
      "--fg-muted": "#7f89b0",
      "--accent": "#7aa2f7",
      "--border": "#2f354f",
      "--selection-bg": "rgba(122, 162, 247, 0.24)",
      "--active-line-bg": "rgba(192, 202, 245, 0.04)",
      "--pattern-color": "rgba(192, 202, 245, 0.07)",
    },
  },
  "one-dark": {
    id: "one-dark",
    label: "One Dark",
    vars: {
      "--bg": "#282c34",
      "--bg-surface": "#2f343f",
      "--bg-overlay": "#3a404d",
      "--fg": "#abb2bf",
      "--fg-dim": "#8a909c",
      "--fg-muted": "#6f7683",
      "--accent": "#61afef",
      "--border": "#464c59",
      "--selection-bg": "rgba(97, 175, 239, 0.22)",
      "--active-line-bg": "rgba(171, 178, 191, 0.04)",
      "--pattern-color": "rgba(171, 178, 191, 0.07)",
    },
  },
  slate: {
    id: "slate",
    label: "Slate",
    vars: {
      "--bg": "#f3efe8",
      "--bg-surface": "#fbf8f2",
      "--bg-overlay": "#ede9e1",
      "--fg": "#1e1c18",
      "--fg-dim": "rgba(30,28,24,0.62)",
      "--fg-muted": "rgba(30,28,24,0.38)",
      "--accent": "#7a5a3a",
      "--border": "rgba(30,28,24,0.08)",
      "--selection-bg": "rgba(122,90,58,0.12)",
      "--active-line-bg": "rgba(30,28,24,0.03)",
      "--pattern-color": "rgba(30,28,24,0.06)",
      "--ink": "#1e1c18",
      "--paper": "#fbf8f2",
      "--ink-soft": "rgba(30,28,24,0.62)",
      "--ink-faint": "rgba(30,28,24,0.38)",
      "--rule": "rgba(30,28,24,0.08)",
      "--rule-strong": "rgba(30,28,24,0.14)",
      "--accent-soft": "rgba(122,90,58,0.12)",
      "--display-font": "'Literata', Georgia, 'Times New Roman', serif",
      "--body-font": "'Geist', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
      "--window-radius": "14px",
      "--shadow-window": "0 0 0 0.5px rgba(0,0,0,0.20), 0 30px 80px rgba(40,30,15,0.28), 0 8px 24px rgba(40,30,15,0.12)",
      "--shadow-dialog": "0 30px 80px rgba(40,30,15,0.35), 0 8px 24px rgba(40,30,15,0.18)",
      "--shadow-popover": "0 12px 32px rgba(40,30,15,0.16)",
      "--bg-backdrop": "radial-gradient(1200px 800px at 20% 10%, #e8e2d6 0%, transparent 60%), radial-gradient(1200px 900px at 90% 90%, #ded5c6 0%, transparent 55%), #f3efe8",
    },
  },
  "slate-dark": {
    id: "slate-dark",
    label: "Slate Dark",
    vars: {
      "--bg": "#17161a",
      "--bg-surface": "#1d1c20",
      "--bg-overlay": "#252328",
      "--fg": "#ece8e0",
      "--fg-dim": "rgba(236,232,224,0.66)",
      "--fg-muted": "rgba(236,232,224,0.40)",
      "--accent": "#c8956a",
      "--border": "rgba(236,232,224,0.08)",
      "--selection-bg": "rgba(232,168,124,0.25)",
      "--active-line-bg": "rgba(255,255,255,0.02)",
      "--pattern-color": "rgba(236,232,224,0.07)",
      "--ink": "#ece8e0",
      "--paper": "#1d1c20",
      "--ink-soft": "rgba(236,232,224,0.66)",
      "--ink-faint": "rgba(236,232,224,0.40)",
      "--rule": "rgba(236,232,224,0.08)",
      "--rule-strong": "rgba(236,232,224,0.16)",
      "--accent-soft": "rgba(200,149,106,0.15)",
      "--display-font": "'Literata', Georgia, 'Times New Roman', serif",
      "--body-font": "'Geist', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
      "--window-radius": "14px",
      "--shadow-window": "0 0 0 0.5px rgba(0,0,0,0.50), 0 30px 80px rgba(0,0,0,0.55), 0 8px 24px rgba(0,0,0,0.30)",
      "--shadow-dialog": "0 30px 80px rgba(0,0,0,0.60), 0 8px 24px rgba(0,0,0,0.35)",
      "--shadow-popover": "0 12px 32px rgba(0,0,0,0.35)",
      "--bg-backdrop": "radial-gradient(900px 700px at 15% 10%, rgba(60,40,25,0.25) 0%, transparent 60%), radial-gradient(800px 700px at 88% 90%, rgba(30,20,40,0.30) 0%, transparent 55%), #17161a",
    },
  },
  "slate-light": {
    id: "slate-light",
    label: "Slate Light",
    vars: {
      "--bg": "#ececec",
      "--bg-surface": "#ffffff",
      "--bg-overlay": "#f2f2f2",
      "--fg": "#111111",
      "--fg-dim": "rgba(0,0,0,0.60)",
      "--fg-muted": "rgba(0,0,0,0.38)",
      "--accent": "#2c5282",
      "--border": "rgba(0,0,0,0.07)",
      "--selection-bg": "rgba(44,82,130,0.14)",
      "--active-line-bg": "rgba(0,0,0,0.02)",
      "--pattern-color": "rgba(0,0,0,0.06)",
      "--ink": "#111111",
      "--paper": "#ffffff",
      "--ink-soft": "rgba(0,0,0,0.60)",
      "--ink-faint": "rgba(0,0,0,0.38)",
      "--rule": "rgba(0,0,0,0.07)",
      "--rule-strong": "rgba(0,0,0,0.14)",
      "--accent-soft": "rgba(44,82,130,0.10)",
      "--display-font": "'Literata', Georgia, 'Times New Roman', serif",
      "--body-font": "'Geist', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
      "--window-radius": "14px",
      "--shadow-window": "0 0 0 0.5px rgba(0,0,0,0.12), 0 20px 60px rgba(0,0,0,0.16), 0 6px 20px rgba(0,0,0,0.08)",
      "--shadow-dialog": "0 20px 60px rgba(0,0,0,0.20), 0 6px 20px rgba(0,0,0,0.12)",
      "--shadow-popover": "0 10px 28px rgba(0,0,0,0.12)",
      "--bg-backdrop": "radial-gradient(1200px 800px at 20% 10%, #dcdcdc 0%, transparent 60%), radial-gradient(1200px 900px at 90% 90%, #d4d4d4 0%, transparent 55%), #ececec",
    },
  },
};

export const BACKGROUND_PRESETS: Record<string, BackgroundPreset> = {
  plain: {
    id: "plain",
    label: "Plain",
    pattern: "none",
    size: "auto",
  },
  lines: {
    id: "lines",
    label: "Lines",
    pattern:
      "repeating-linear-gradient(0deg, transparent 0 27px, var(--pattern-color) 27px 28px)",
    size: "100% 28px",
  },
  squares: {
    id: "squares",
    label: "Squares",
    pattern:
      "linear-gradient(var(--pattern-color) 1px, transparent 1px), linear-gradient(90deg, var(--pattern-color) 1px, transparent 1px)",
    size: "24px 24px",
  },
  dots: {
    id: "dots",
    label: "Dots",
    pattern: "radial-gradient(var(--pattern-color) 1px, transparent 1px)",
    size: "20px 20px",
  },
  diagonal: {
    id: "diagonal",
    label: "Diagonal",
    pattern:
      "repeating-linear-gradient(45deg, transparent 0 12px, var(--pattern-color) 12px 13px)",
    size: "18px 18px",
  },
};

export const DEFAULT_COLOR_SCHEME = "slate";
export const DEFAULT_BACKGROUND = "plain";
export const DEFAULT_FONT = "jetbrains-mono";
export const DEFAULT_FONT_SIZE = 14;
export const MIN_FONT_SIZE = 11;
export const MAX_FONT_SIZE = 28;

export const FONT_PRESETS: Record<string, FontPreset> = {
  "jetbrains-mono": {
    id: "jetbrains-mono",
    label: "JetBrains Mono",
    stack:
      '"JetBrains Mono", "Fira Code", "Cascadia Code", "Source Code Pro", ui-monospace, monospace',
  },
  "fira-code": {
    id: "fira-code",
    label: "Fira Code",
    stack:
      '"Fira Code", "JetBrains Mono", "Cascadia Code", "Source Code Pro", ui-monospace, monospace',
  },
  "cascadia-code": {
    id: "cascadia-code",
    label: "Cascadia Code",
    stack:
      '"Cascadia Code", "JetBrains Mono", "Fira Code", "Source Code Pro", ui-monospace, monospace',
  },
  iosevka: {
    id: "iosevka",
    label: "Iosevka",
    stack:
      '"Iosevka", "JetBrains Mono", "Fira Code", "Cascadia Code", ui-monospace, monospace',
  },
  hack: {
    id: "hack",
    label: "Hack",
    stack:
      '"Hack", "JetBrains Mono", "Fira Code", "Cascadia Code", "Source Code Pro", ui-monospace, monospace',
  },
  "source-code-pro": {
    id: "source-code-pro",
    label: "Source Code Pro",
    stack:
      '"Source Code Pro", "JetBrains Mono", "Fira Code", "Cascadia Code", ui-monospace, monospace',
  },
};

export const FONT_IDS = Object.keys(FONT_PRESETS);

export const DISPLAY_FONT_PRESETS: Record<string, DisplayFontPreset> = {
  literata: {
    id: "literata",
    label: "Literata",
    stack: "'Literata', Georgia, 'Times New Roman', serif",
  },
  fraunces: {
    id: "fraunces",
    label: "Fraunces",
    stack: "'Fraunces', Georgia, 'Times New Roman', serif",
  },
  "source-serif-4": {
    id: "source-serif-4",
    label: "Source Serif 4",
    stack: "'Source Serif 4', Georgia, 'Times New Roman', serif",
  },
  "eb-garamond": {
    id: "eb-garamond",
    label: "EB Garamond",
    stack: "'EB Garamond', Georgia, 'Times New Roman', serif",
  },
};

export const DEFAULT_DISPLAY_FONT = "literata";
export const DISPLAY_FONT_IDS = Object.keys(DISPLAY_FONT_PRESETS);

export function clampFontSize(size: number) {
  if (!Number.isFinite(size)) return DEFAULT_FONT_SIZE;
  return Math.max(MIN_FONT_SIZE, Math.min(MAX_FONT_SIZE, Math.round(size)));
}
