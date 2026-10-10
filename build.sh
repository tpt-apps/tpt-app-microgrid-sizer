# TPT Microgrid Sizer - build pipeline (unix mirror of build.ps1).
#
#   ./build.sh              free wasm bundle -> dist/hub
#   ./build.sh -Pro         pro wasm + trunk bundle + desktop binary
#   ./build.sh -SkipBuild   re-run wasm-bindgen only
#   TPT_WEB_REPO=<path>     also copy dist/hub/* into <repo>/public/apps/microgrid-sizer/

set -euo pipefail
cd "$(dirname "$0")"

PRO=0
SKIP_BUILD=0
WEB_REPO="${TPT_WEB_REPO:-}"
# Positional/`-WebRepo` paths are only accepted when they look like a web repo
# root: an existing directory that already carries the public/apps layout (or
# at least a public/ directory). Guessing wrong here would scatter the bundle
# into an unrelated tree, so a bad path is a hard error.
WEB_REPO_ARG_SET=0
for arg in "$@"; do
  case "$arg" in
    -Pro|--pro) PRO=1 ;;
    -SkipBuild|--skip-build) SKIP_BUILD=1 ;;
    -WebRepo|--web-repo) WEB_REPO_ARG_SET=1 ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *)
      if [ "$WEB_REPO_ARG_SET" = 1 ]; then
        WEB_REPO="$arg"
        WEB_REPO_ARG_SET=0
      elif [ -z "$WEB_REPO" ]; then
        WEB_REPO="$arg"
      else
        echo "unexpected extra argument: $arg" >&2
        exit 2
      fi
      ;;
  esac
done

if [ -n "$WEB_REPO" ]; then
  if [ ! -d "$WEB_REPO" ]; then
    echo "web repo path does not exist or is not a directory: $WEB_REPO" >&2
    exit 2
  fi
  if [ ! -d "$WEB_REPO/public" ]; then
    echo "web repo path has no public/ directory: $WEB_REPO" >&2
    exit 2
  fi
  WEB_REPO="$(cd "$WEB_REPO" && pwd)"
  echo "-- web repo: $WEB_REPO"
fi

CRATE="tpt-app-microgrid-sizer"
TARGET="wasm32-unknown-unknown"
OUT_NAME="microgrid-sizer"
HUB_DIR="dist/hub"
DESKTOP_DIR="dist/desktop"

echo "== TPT Microgrid Sizer build (edition: $([ "$PRO" = 1 ] && echo pro || echo free)) =="

if [ "$SKIP_BUILD" = 0 ]; then
  if [ "$PRO" = 1 ]; then
    cargo build --release --target "$TARGET" -p "$CRATE" --features pro
  else
    cargo build --release --target "$TARGET" -p "$CRATE"
  fi
fi

WASM="target/$TARGET/release/tpt_app_microgrid_sizer.wasm"
[ -f "$WASM" ] || { echo "missing $WASM - run the cargo build step first"; exit 1; }

if [ "$PRO" = 1 ]; then
  echo "-- hub bundle left untouched (the website always gets the free edition)"
else
  mkdir -p "$HUB_DIR"
  wasm-bindgen --target web --out-name "$OUT_NAME" --out-dir "$HUB_DIR" "$WASM"
  [ -f "$HUB_DIR/$OUT_NAME.js" ] || { echo "wasm-bindgen did not produce the glue"; exit 1; }

  if [ -n "$WEB_REPO" ]; then
    mkdir -p "$WEB_REPO/public/apps/microgrid-sizer"
    cp "$HUB_DIR/$OUT_NAME.js" "$HUB_DIR/${OUT_NAME}_bg.wasm" "$WEB_REPO/public/apps/microgrid-sizer/"
    echo "-- copied hub bundle into $WEB_REPO/public/apps/microgrid-sizer"
  fi

  RAW=$(wc -c < "$HUB_DIR/${OUT_NAME}_bg.wasm" | tr -d ' ')
  GZ=$(gzip -9 -c "$HUB_DIR/${OUT_NAME}_bg.wasm" | wc -c | tr -d ' ')
  echo "-- ${OUT_NAME}_bg.wasm: $RAW bytes raw, $GZ bytes gzipped"
  if [ "$GZ" -gt 614400 ]; then
    echo "WARNING: payload exceeds the ~600 KB gzip budget" >&2
  fi
fi

if [ "$PRO" = 1 ]; then
  trunk build --release --features pro --dist "$DESKTOP_DIR"
  cargo build --release -p tpt-microgrid-desktop
  echo "-- desktop binary: target/release/tpt-microgrid-sizer-pro (serves $DESKTOP_DIR)"
fi

echo "-- done"
