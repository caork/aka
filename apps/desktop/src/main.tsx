import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { refreshRepos } from "./store";
import { applyTheme, readThemeMode } from "./theme";
import "./index.css";

applyTheme(readThemeMode());
void refreshRepos();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
