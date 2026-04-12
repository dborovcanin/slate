import { initApp } from "./app";
import { loadAndApplyTheme, startThemeLiveReload } from "./theme/theme";

async function bootstrap() {
  const configPromise = loadAndApplyTheme();
  await initApp(configPromise);
  startThemeLiveReload();
}

bootstrap().catch((e) => {
  console.error("Failed to initialize app:", e);
  document.body.textContent = `Failed to start: ${e}`;
});
