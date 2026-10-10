// TPT Microgrid Sizer — free in-browser edition.
//
// Hub contract (web repo `WasmAppRunner`):
//   1. the runner dynamic-imports the wasm-bindgen `--target web` glue,
//   2. awaits its default export (wasm init),
//   3. calls the named export `mount(container)` with the hub's container div.
//
// Standalone dev (trunk serve) uses the same `mount_app` via the
// `#tpt-appfront-root` marker div that only index.html provides, so the two
// hosts never double-mount.

// The DOM app compiles only for wasm32 (tpt-appfront-dom is an empty crate
// on other targets); the view layer and chart data compile anywhere so the
// UI structure is host-unit-testable, and the engine stays fully
// target-independent.
#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(any(target_arch = "wasm32", test))]
mod chart;
#[cfg(any(target_arch = "wasm32", test))]
mod view;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Hub entry point: mounted into the /tools/microgrid-sizer page's container.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn mount(container: web_sys::Element) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    app::mount_app(&container)
}

/// Standalone entry (trunk serve): index.html provides `#tpt-appfront-root`;
/// the hub page does not, so this is a no-op when embedded.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let window = web_sys::window().expect("no window");
    let document = window.document().expect("no document");
    if let Some(root) = document.get_element_by_id("tpt-appfront-root") {
        app::mount_app(&root)?;
    }
    Ok(())
}
