import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { hydrateRepos } from "./store";
import "./index.css";

void hydrateRepos();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
