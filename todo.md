# TPT Microgrid Sizer — Build Todo

> Hub listing: `tptsolutions.co.nz/tools/microgrid-sizer` (registry slug
> `microgrid-sizer`, category `engineering`). Spec: `apps.md` §T1-9 in the
> tpt-electrician-nz-2 repo. Free WASM edition: single-day load/generation
> balance, one battery size. Paid offline edition ($149 USD, Gumroad): full
> seasonal simulation, battery/generation sizing optimization, and exportable
> system-design reports. Solver wraps `tpt-energy`. UI on `tpt-appfront`.
>
> Hub contract: `wasm-bindgen --target web` glue exposes `mount(container)`
> (called by `WasmAppRunner` after `default()` init). Standalone `trunk serve`
> mounts into the `#tpt-appfront-root` marker div instead — never
> `document.body` — so the two hosts never double-mount.

## Phase 0 — Scaffold & pipeline proof
- [x] Scaffold the crate on the hub contract (scaffolded in-repo following
      `tpt-app-fea-lite`'s refined pattern — the stock `tpt-appfront init`
      template predates the `mount()`/marker-div contract) — done
- [x] Restructure scaffold to hub contract: `#[wasm_bindgen] pub fn mount(el)` + `#[wasm_bindgen(start)]` no-op unless `#tpt-appfront-root` present
- [x] `index.html` carries the `#tpt-appfront-root` marker div
- [x] Build pipeline script mirroring fea-lite/cutlist-optimizer's (`build.ps1`/
      `build.sh`/`justfile`: `--target web` glue → web repo `public/apps/microgrid-sizer/`)
- [x] Hello-world build loads through the standalone dev page (`trunk serve`)
- [x] git init + first commit

## Phase 1 — Solver integration (wrap tpt-energy)
- [x] Survey `tpt-energy`: `tpt-nrg-solar` (SolarModel/PvPlant — irradiance
      models are crate-private, so cloud cover goes through the public
      `output_clearsky` → `output_from_poa` path), `tpt-nrg-battery`
      (BatteryStorage — mutates SoC *before* erroring, so the engine
      pre-clamps requests against available/headroom)
- [x] Adapter crate `engine/`: load profile + solar/battery spec in →
      generation/storage balance, unmet-load hours, sizing recommendation out
      as plain structs — no UI deps, host-unit-testable
- [x] Validation against a simple hand-checkable load/generation scenario
      (battery-only dispatch empties in exactly capacity/load hours; charge
      losses = sqrt(RTE) per leg; SoC floor; power-rating curtailment;
      hemisphere seasonality) — 10 free + 14 pro tests green

## Phase 2 — Free WASM UI
- [x] Single-day load profile input (24 hourly fields + presets) + one
      solar/battery configuration
- [x] Load-vs-generation balance chart (canvas: solar fill, load line, SoC
      right axis, unmet-hour shading), single battery size
- [x] Free-tier upsell copy pointing at seasonal simulation and sizing
      optimization ($149)
- [x] Browser-verified: mount, simulate, chart renders correctly (headless
      Chromium over CDP — free day, commercial preset with unmet bands,
      winter month; zero console errors)

## Phase 3 — Paid desktop edition (cargo feature `pro`)
- [x] `pro` gates: full seasonal simulation (12 representative days scaled by
      month length, monthly load/cloud/temperature factors, SoC carried
      between months), battery/generation sizing optimization (grid search
      over PV × battery for cheapest design meeting a served-energy target),
      exportable system-design reports (Markdown download)
- [x] Pro UI: seasonal factor grid, sizing-optimization inputs + recommendation
      with apply-to-inputs, export button
- [x] Desktop shell via `tpt-appfront-webview`; Windows exe build + smoke test
      (exe boots, serves the pro bundle, single-instance + open_external ACL)
- [ ] Gumroad product from apps.md listing copy ($149), exe upload, URL into
      registry — manual step, listing copy at apps.md §T1-9 and `GUMROAD.md`
      (the package is built: `release\TPT-Microgrid-Sizer-Pro.zip`)

## Phase 4 — Ship & measure
- [x] Registry entry (`engineering` category, `wasm: { entry: '/apps/microgrid-sizer/microgrid-sizer.js' }`,
      `status: 'coming-soon'` per the geotech-calc pattern) added to
      `tpt-apps-registry.ts` — flip to `'live'` at the Gumroad step
- [x] Verify hub card/detail/runner in dev (`/tools` card present,
      `/tools/microgrid-sizer` renders and mounts the free app, zero console
      errors); prod check after the web repo deploy (manual)
- [x] WASM size budget check on the shipped build — **99.3 KB gzipped** against
      the ~600 KB budget
