import { initApp } from "./app";
import { loadAndApplyTheme } from "./theme/theme";

async function bootstrap() {
  const configPromise = loadAndApplyTheme();
  await initApp(configPromise);
}

bootstrap().catch((e) => {
  console.error("Failed to initialize app:", e);
  document.body.textContent = `Failed to start: ${e}`;
});
