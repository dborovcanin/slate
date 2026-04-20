import { initApp } from "./app";
import { loadAndApplyTheme } from "./theme/theme";
import { startupFlush, startupMark } from "./perf/startup.ts";
import { getCurrentWindow } from "@tauri-apps/api/window";

async function bootstrap() {
  startupMark("ui_bootstrap_start");
  const configPromise = loadAndApplyTheme();
  startupMark("ui_theme_load_requested");
  await initApp(configPromise);
  startupMark("ui_app_ready");
  await getCurrentWindow().show();
  startupFlush("gui");
}

bootstrap().catch((e) => {
  console.error("Failed to initialize app:", e);
  document.body.textContent = `Failed to start: ${e}`;
});
