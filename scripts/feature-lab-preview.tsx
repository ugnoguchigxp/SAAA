import { createRoot } from "react-dom/client";
import "../src/features/media/mediaGeneration.css";
import { FeatureLabPreview } from "../src/features/media/FeatureLabPreview";

const params = new URLSearchParams(window.location.search);
const transport = params.get("transport") === "http" ? "http" : "react-mock";
const provider = params.get("provider") === "larm" ? "larm" : "fixture";
const root = document.getElementById("root");
if (root) createRoot(root).render(<FeatureLabPreview transport={transport} provider={provider} />);
