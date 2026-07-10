import React from "react";
import ReactDOM from "react-dom/client";
import { SettingsWindow } from "./index";
import "../../index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsWindow />
  </React.StrictMode>,
);
