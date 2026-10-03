# Gumroad listing

Everything needed to create/update the Gumroad product once this app is
built. Source copy is `apps.md` §T1-9 in the `tpt-electrician-nz-2` repo — if
that changes, update here too (and vice versa).

## Step-by-step: publish this listing

1. Build the Pro edition (`./build.ps1 -Pro`, once the build pipeline exists
   — see `todo.md` and mirror `tpt-app-fea-lite`/`tpt-app-cutlist-optimizer`'s
   pattern) — produces the packaged exe/zip in `release/`.
2. Log in to Gumroad → **New product** → **Digital product**.
3. Set the product name, price, and upload the packaged file (all below).
4. Paste the short description, bullets, and full description (below) into
   their respective fields.
5. Set category/tags (below) and add a cover image (see note below).
6. Publish, then copy the resulting product URL (`https://gum.co/...`).
7. Paste that URL into the `microgrid-sizer` entry's `gumroadUrl` field in
   `tpt-electrician-nz-2\src\lib\tpt-apps-registry.ts`, and set `price: '$149'`
   on the same entry.
8. Flip that entry's `status` to `'live'` once both the free demo and this
   Gumroad listing exist.
9. Commit and push the registry repo.

## File to upload

`release\TPT-Microgrid-Sizer-Pro.zip` — produced by `./build.ps1 -Pro`
(self-contained exe: the web bundle is embedded and extracts on first run;
see `README.txt` inside the zip).

## Product name

```
TPT Microgrid Sizer — Off-Grid & Microgrid Battery Sizing Tool
```

## Price

**$149 USD, one-time purchase** (no subscription).

## Short description / summary field

```
Size solar + battery systems correctly with full seasonal load simulation — built for renewable energy consultants and design engineers, not installers.
```

## Bullets / feature list

```
Full seasonal simulation
Battery/generation sizing optimization
Exportable design reports
One-time purchase
No subscription
```

## Full description (long-form field)

```
TPT Microgrid Sizer models load profiles against solar generation and
battery storage to size a microgrid system correctly before installation,
including seasonal variation.

Who it's for: renewable-energy consultants and design engineers sizing
off-grid or microgrid battery/solar systems (off-grid cabins, small
commercial sites) — a design-stage engineering tool, deliberately separate
from the fieldservice solar-install audience.

Pro edition (this download):
- Full seasonal simulation (not just a single day)
- Battery/generation sizing optimization
- Exportable system-design reports

Try the free single-day balance edition first, right in your browser, no
install: tptsolutions.co.nz/tools/microgrid-sizer

Requirements: Windows 10 (20H2+) or Windows 11. Uses the WebView2 runtime,
preinstalled on both — nothing extra to download.
```

## Category / tags

- Category: **Software > Design & Engineering** (or Gumroad's closest equivalent)
- Tags: `microgrid-design`, `solar-engineering`, `battery-sizing`,
  `renewable-energy`, `engineering-software`, `windows`

## Cover image / thumbnail

Not created yet. A screenshot of a full seasonal load/generation chart with
a real system profile loaded is the most convincing cover once built. No
stock/AI art. (`target\verify\pro-full.png` from the verification pass is a
reasonable starting point; re-shoot at a nicer window size for the real
thing.)

## After publishing

1. Copy the resulting product URL (`https://gum.co/...`).
2. Paste it into the `gumroadUrl` field of the `microgrid-sizer` entry in
   `tpt-electrician-nz-2\src\lib\tpt-apps-registry.ts`, and set `price: '$149'`.
3. Set `status` to `'live'`.
4. Commit and push the registry repo.
