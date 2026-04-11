import React from "react";
import ReactDOM from "react-dom/client";
import { ThemeProvider } from "./components/theme-provider";
import './index.css'

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider><Button/></ThemeProvider>
  </React.StrictMode>,
);


function Button() {
  return <button >Continue</button>
}