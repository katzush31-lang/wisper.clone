import React from "react";
import ReactDOM from "react-dom/client";
import "../shared/theme.css";
import "./bar.css";
import Bar from "./Bar";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Bar />
  </React.StrictMode>,
);
