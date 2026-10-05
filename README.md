# TPT Microgrid Sizer

Off-grid solar + battery sizing for cabins and small commercial sites —
hourly load/generation balance, solved locally.

Two editions ship from this one codebase:

| Edition | Build | Where it runs |
|---|---|---|
| **Free** | default (no features) | In-browser WASM at
[tptsolutions.co.nz/tools/microgrid-sizer](https://tptsolutions.co.nz/tools/microgrid-sizer):
single-day load/generation balance, one battery size |
| **Pro** | `--features pro` | Paid desktop exe (Gumroad, $149): full
seasonal (12-month) simulation (steady-state daily cycle per month), PV +
battery sizing optimization with optional backup generator and levelised
cost (LCOE), exportable system-design reports. |

## Layout

```
src/lib.rs       hub-contract glue: `mount(container)` export + standalone start
src/app.rs       the DOM UI (mounted-once inputs + reactive results)
src/chart.rs     canvas time-series chart (day and month views)
engine/          tpt-energy adapter — balance loop, dispatch, validation;
                 host-unit-testable, no UI deps (pro-gated: seasonal,
                 recommend, report)
desktop/         WebView2 shell for the Pro exe (embeds dist/ at compile time)
index.html       trunk page with the #tpt-appfront-root marker div
build.ps1        the pipeline (see below); build.sh is the unix mirror
```

The engine wraps [tpt-energy](../tpt-energy): `tpt-nrg-solar` for solar
position / clear-sky / PV output, `tpt-nrg-battery` for SoC-tracked
dispatch. The hourly balance loop, unmet/curtailed accounting, seasonal
aggregation, and sizing search live here.

## Hub contract

The web repo's `WasmAppRunner` dynamic-imports the wasm-bindgen
`--target web` glue, awaits its default export, then calls
`mount(container)`. `src/lib.rs` exports exactly that, plus a
`#[wasm_bindgen(start)]` that mounts into `#tpt-appfront-root` — a marker
div only `index.html` provides, so standalone dev (trunk serve) and the
embedded hub never double-mount.

## Development

```
just dev        # trunk serve on http://127.0.0.1:8080 (standalone mount)
just test       # engine tests (free + pro) + wasm type-checks
just check      # free / pro / desktop type-checks
```

Engine validation is hand-checkable: constant load vs. an empty-sky day
must empty the battery in exactly `capacity / load` hours, charge-side
losses are `sqrt(RTE)` per leg, the SoC floor caps discharge, and PV yield
must follow hemisphere seasonality.

## Build & ship

```
./build.ps1             # free wasm -> dist\hub\ (glue + wasm), gzip size report
./build.ps1 -WebRepo <path>   # ...and copy into <path>\public\apps\microgrid-sizer\
./build.ps1 -Pro        # pro trunk bundle -> dist\desktop\, exe, release\ zip
```

The `-Pro` pipeline never touches `dist\hub` — the website always gets the
free edition. The exe embeds `dist\desktop` (single-file Gumroad download;
resolve order: `TPT_MICROGRID_DIST` env → `./dist` → dist next to the exe →
embedded copy extracted to `%TEMP%`).

wasm-bindgen is pinned to the installed CLI version (currently `=0.2.128`)
— the CLI rejects wasm built against a different schema version; bump both
together.

Website registry entry lives in the web repo's
`src/lib/tpt-apps-registry.ts` (slug `microgrid-sizer`, category
`engineering`); listing copy: `GUMROAD.md` / apps.md §T1-9.
