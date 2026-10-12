import "./app.css";
import { mount } from "svelte";

// The web build (VITE_TARGET=web) runs outside a Tauri webview and needs the
// HTTP shim installed before any component calls `invoke`/`listen`. The flag is
// unset for the desktop build, so the dynamic import is statically dropped
// there. Started here rather than inside `start()` so the install is in flight
// at module evaluation, ahead of the root component's own imports.
const shimReady: Promise<unknown> =
  import.meta.env.VITE_TARGET === "web"
    ? import("./lib/tauri-web-shim")
    : Promise.resolve();

async function start() {
  await shimReady;
  const { default: Root } = new URLSearchParams(location.search).has("menubar")
    ? await import("./lib/Menubar.svelte")
    : await import("./App.svelte");
  return mount(Root, { target: document.getElementById("app")! });
}
export default start();
