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
      hemisphere seasonality) — now 13 free + 25 pro tests green

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
      month length, monthly load/cloud/temperature factors, each day run to a
      steady-state SoC cycle), battery/generation sizing optimization (grid search
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

## Phase 5 — Review follow-ups (platform review, 2026-10)
Done:
- [x] kWh/MWh unit bug in formatters (engine + UI); `format_kwh` test
- [x] Optimizer honours the user's battery settings (`recommend_size` takes a `BatterySpec`)
- [x] Desktop `open_external` host allow-list (exact host match) + test
- [x] Cargo path deps made relative (`../tpt-appfront`, `../../tpt-energy`)
- [x] Seasonal model: each month's day iterated to a steady-state SoC cycle
- [x] Solar position sampled at hour midpoint
- [x] Desktop dist extraction keyed by app version
- [x] Battery cycles use usable capacity; initial SoC below min SoC rejected
- [x] Report: monthly factors table, initial SoC, generator line
- [x] Free edition: clear-sky / 15 °C best-case caveat in the site card
- [x] LCOE + annual cost (discount rate, project life, O&M, battery replacement)
- [x] Optional backup generator in seasonal sim, optimizer and report
- [x] Engine tests: NaN/range validation, zero-power battery, negative load,
      infeasible optimizer target, generator, annual cost

Open — bugs / robustness:
- [x] Multi-day cloudy-spell / autonomy check (`simulate_cloudy_spell`: back-to-back
      worst-month days from a full battery, SoC carried across days; surfaced in
      the seasonal results and the report, spell length is a UI input)
- [x] Error when solar and load profile lengths differ (`ProfileError` —
      `dispatch_profile` now returns `Result`, so a short solar array is a
      data-entry error rather than a sunless day)
- [x] Add optimizer cost inputs (and generator settings) to the report (new
      "Cost basis" section, so the LCOE is reproducible)
- [x] Optimizer objective: minimise annual cost / LCOE rather than capex
      (`OptimizationObjective` — capex / annual cost / LCOE, UI select)
- [x] Decide Pro licensing: **honour system** — no key/activation check. The
      $149 zip ships the plaintext pro wasm by design (offline desktop app);
      Gumroad checkout + the README/GUMROAD notice carry the "personal
      licence, don't redistribute" ask. No code to maintain, no key servers,
      no false sense of security from an obfuscation that trivially unwraps
- [x] Add a LICENSE file — **dual MIT/Apache-2.0** (owner choice):
      `LICENSE-MIT` + `LICENSE-APACHE`, `license = "MIT OR Apache-2.0"` in all
      three Cargo.toml manifests (code only — the TPT Solutions branding,
      listing copy and `cover.png` artwork stay proprietary)
- [ ] Browser/desktop verification of the new Pro inputs (objective select,
      overcast spell, cost basis, steady-state seasonal) — headless browser
      passes now run for both editions (`chart-check` in the session scratchpad,
      built with trunk and driven over CDP in Edge): free 14/14 and Pro 20/20
      checks — hover tooltip, keyboard cursor, resize without page overflow,
      the text alternative, the appliance builder, the seasonal chart, and the
      inverter, reliability and grid cards, with no console errors. Still open:
      the desktop shell (the Pro exe, run with WebView2 remote debugging) passes
      the same 28 checks as the browser, which also cover saved scenarios
      (save, load, delete, survive a reload), the overcast spell, and the
      optimizer's objective select. The report (Markdown, with its cost-basis,
      inverter, reliability and grid sections), the PDF and both CSVs were
      captured from the page's downloads and checked. Saving under an existing
      scenario name now replaces it. Dark mode:
      the app follows the OS colour scheme (`color-scheme` declared, and the
      buttons and form controls themed); both themes were checked by screenshot.
- [x] `build.sh` (and `build.ps1`) validate their web-repo path argument

Open — missing tests:
- [x] Report content beyond section headings; leap years, DST / fractional UTC
      offsets, panel orientation
- [ ] UI, chart and desktop shell tests
- [x] Test that the free build excludes Pro features

Open — features (by value):
- [x] PV and battery degradation — `SolarArray::annual_degradation` (0.5%/yr)
      and `BatterySpec::annual_fade` (2%/yr), `aged()` on both; the optimizer
      judges every candidate at end of life (PV over project life, battery at
      its last point before replacement). Caveat: LCOE still divides by the
      end-of-life served energy, not a lifetime-averaged figure, so it is
      conservative; seasonal results and the day view stay as-new.
- [x] Load CSV and TMY / measured weather import — paste-in CSV (`engine/src/import.rs`):
      load = 24 hourly kW (one day) or 8760 (a year → mean day + monthly load
      factors); weather = 8760 rows of GHI, DNI, DHI → monthly cloud factors
      calibrated to the measured AC yield vs clear sky. Caveats: pasted text
      only (no file picker yet); calibration is an energy-level match, not
      hour-by-hour; the cloudy-spell check still uses the calibrated monthly
      factors, not the measured day sequence; the weather import is Pro-only.
      Browser pass not yet run.
- [x] CSV and PDF export; saved scenarios and side-by-side comparison —
      `engine/src/export.rs`: hourly CSV (free), monthly CSV and a PDF of the
      design report (Pro; minimal dependency-free writer, Helvetica/Courier,
      tables in monospace). Saved scenarios live in this browser's
      localStorage: inputs plus the metrics from the last runs at save time,
      compared side by side. Caveats: a scenario restores by input position,
      so it only loads in the edition that saved it; metrics are not re-run
      on load; the PDF is checked structurally with poppler (`pdftotext`) but
      not visually; no browser pass yet.
- [x] Inverter / peak-power sizing check and configurable DC/AC ratio —
      `SolarArray::dc_ac_ratio` (default 1.2, the PV model's old default, so
      existing results are unchanged) sets the inverter rating
      (DC ÷ ratio). `engine/src/inverter.rs` reports whether that rating
      covers the peak demand and the energy clipped per year from the same
      hourly model as the seasonal run. Caveats: the check is advisory and
      does not cap the dispatch; only the PV inverter counts against the
      peak (a separate battery inverter is ignored); clipping is estimated
      on representative days; no browser pass yet.
- [x] Reliability metrics: LOLP, days of autonomy — `engine/src/reliability.rs`
      (Pro): loss-of-load probability (share of hours unserved), loss-of-load
      hours and expectation (days a year with any unserved hour), all from the
      steady representative days with any backup generator applied; and
      battery-only autonomy, the days the battery alone carries the worst
      month from full with no sun (capped at 60). Shown in a Pro card and the
      design report. Caveats: the year is the typical year from the monthly
      factors, not a weather sequence, so LOLP is an expectation and does not
      capture a bad run of days (the cloudy-spell check does); autonomy ignores
      sun entirely, so it is a floor, not the spell result; no browser pass yet.
- [x] Tariffs, grid connection and export / net metering — `engine/src/grid.rs`
      (Pro): a flat import price, an export credit (equal to the import price
      models net metering), a daily charge, and import and export limits. The
      grid imports unmet load and exports surplus the battery cannot store; it
      never charges the battery. Reports the year's bills with and without the
      system, the saving and simple payback, and goes in the design report.
      Caveats: flat rates only (no time-of-use, demand charges or monthly
      netting); the year is the typical year from the monthly factors; the
      reliability figures stay islanded and ignore the grid; no browser pass yet.
- [x] Chart: resizable canvas, hover tooltips, accessibility text — both charts
      (`src/chart.rs`, `mount_chart` in `src/app.rs`) fill their container and
      repaint on window resize, sized for the device pixel ratio. Hovering
      shows a crosshair and a tooltip for the nearest hour or month; the arrow
      keys, Home, End and Escape move the same cursor for keyboard users. Each
      canvas has a role and an aria-label, and a visually hidden list gives every
      slot's values. Caveats: touch has no tooltip (the text list covers it);
      the tooltip is pure text, not a styled panel; resizing relies on the
      window event, so a container resized without a window change will not
      repaint. Checked in headless Edge: hover, keyboard, resize and the text list.
- [x] Per-appliance load builder; shading and multi-orientation arrays —
      `engine/src/appliances.rs`: up to six appliances (power, start hour,
      hours per day, wrapping past midnight) summed into the hourly load.
      `SolarArray::obstructions`: up to two shading windows (whole hours, loss
      fraction, overlapping windows combine). `SolarArray::orientations`: up
      to four planes, each a share of capacity with its own tilt and azimuth,
      and each with its own inverter share so clipping is per plane. Empty
      lists keep the single-plane behaviour, so existing results are unchanged.
      Caveats: appliance power is constant during a run; the profile repeats
      every day; shading is whole hours only and uniform across the array;
      the weather calibration uses the open sky, so shading is applied once,
      in the simulation; scenarios saved before this change have fewer inputs
      and will not load. The builder ran in headless Edge.

## Phase 6 — Ease of use and theming (2026-10)
- [x] Places: `engine/src/sites.rs` has ten offline NZ and AU presets (name,
      latitude, longitude, standard UTC offset). "Start from a place" fills
      the site inputs; altitude is left alone. Tests check every preset is a
      valid site and that its offset agrees with its longitude.
- [x] "Use my location": the browser's geolocation (with its permission prompt)
      fills latitude, longitude and the UTC offset. The position is used only
      to fill those inputs. New Zealand is special-cased to +12, because
      longitude alone gives +11 at Queenstown. Caveat: the desktop shell and
      other browsers may not offer location; the status line then says to enter
      the coordinates by hand.
- [x] Finding coordinates by hand: short instructions in the site card, and the
      UTC offset is explained as standard time (no daylight saving).
- [x] Colour theme: a "Theme" control in the header (match system, light, dark),
      remembered in this browser. The OS setting still applies by default, and
      both explicit themes override it. Buttons and form controls are themed
      and `color-scheme` is declared for both themes.
- [x] Saving under an existing scenario name replaces that scenario.
- Caveats: the presets use standard time, so a summer-time site should keep its
  standard offset; the offset suggestion is only a guess outside New Zealand
  (time-zone borders do not follow longitude); location needs permission and a
  location provider, which desktop WebView2 may not have.
