import { initApp } from "./app";

initApp().catch((e) => {
  console.error("Failed to initialize app:", e);
  document.body.textContent = `Failed to start: ${e}`;
});
