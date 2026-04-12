import { initApp } from "./app";
import { loadAndApplyTheme, startThemeLiveReload } from "./theme/theme";

async function bootstrap() {
  await loadAndApplyTheme();
  await initApp();
  startThemeLiveReload();
}

bootstrap().catch((e) => {
  console.error("Failed to initialize app:", e);
  document.body.textContent = `Failed to start: ${e}`;
});
