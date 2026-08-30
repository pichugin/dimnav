#!/usr/bin/env bash
#
# Builds the demo file tree that the website video and screenshots are recorded
# against (scripts/record-demo.sh). Committed rather than made by hand because
# the demo has to be re-recorded every time the UI moves, and a screenshot of a
# tree nobody can reproduce is a screenshot nobody can refresh.
#
#   ./scripts/demo-tree.sh [target]          # default /Users/Shared/dimnav-demo
#   ./scripts/demo-tree.sh [target] --force  # rebuild in place
#
# The default deliberately sits under /Users/Shared rather than $HOME: dimnav
# prints the absolute path in each panel header, and these frames end up on a
# public web page. /Users/Shared keeps the maintainer's account name out of
# every screenshot without needing a second user account.
#
# Three things make the tree read as real rather than as fixture data:
#
#   * sizes    - bulk files are made with `mkfile -n`, which is sparse: instant
#                to create, costs no disk, and still reports the intended size
#                to `ls` and to the detailed panel view.
#   * dates    - `touch -t` spreads mtimes over the past year. Without this
#                every row in the detailed view reads today, and the whole tree
#                looks generated the moment you sort by date.
#   * content  - the handful of files the video actually opens carry real text,
#                a real image and a real binary, so F3/F4 have something to show.

set -euo pipefail

TARGET="${1:-/Users/Shared/dimnav-demo}"
FORCE="${2:-}"
[[ "$TARGET" == --force ]] && { TARGET="/Users/Shared/dimnav-demo"; FORCE=--force; }

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MARKER=".dimnav-demo"

# A tree this script did not create is never touched. The marker is the only
# thing that authorises the rm -rf below, so --force cannot be pointed at a real
# directory by a slip of the shell.
if [[ -e "$TARGET" ]]; then
  if [[ "$FORCE" != "--force" ]]; then
    echo "error: $TARGET already exists. Pass --force to rebuild it." >&2
    exit 1
  fi
  if [[ ! -f "$TARGET/$MARKER" ]]; then
    echo "error: $TARGET exists but has no $MARKER marker." >&2
    echo "       Refusing to delete a directory this script did not create." >&2
    exit 1
  fi
  rm -rf "$TARGET"
fi

mkdir -p "$TARGET"
: > "$TARGET/$MARKER"

# --- helpers ---------------------------------------------------------------

# sized <path> <size>  - sparse file of a given size, e.g. 4m, 512k, 1200
# mkfile creates 0600, which reads oddly for holiday photos in the detailed
# view, so everything lands on the usual 0644 and the few executables are
# chmod'd back up explicitly.
sized() { mkdir -p "$(dirname "$1")"; mkfile -n "$2" "$1"; chmod 644 "$1"; }

# A deterministic spread of plausible dates, walked round-robin with a minute
# offset per file, so a listing sorted by date shows a believable scatter.
#
# NOTE: the counter is advanced by a plain function call, never by $(...).
# Command substitution runs in a subshell, so an `echo`-style stamp() would
# lose every increment and stamp the entire tree with one identical date -
# which is precisely what makes a generated tree look generated.
DATES=(
  202509120914 202510031642 202510221108 202511051530 202511190803
  202512011245 202512241959 202601140736 202602081422 202603021017
  202603271853 202604111134 202605090621 202606021508 202606211043
  202607041625 202607190912 202608051347 202608221104 202608281839
)
di=0
STAMP=""
next_stamp() {
  local base="${DATES[$((di % ${#DATES[@]}))]}"
  local bump=$(( (di / ${#DATES[@]}) * 7 % 50 ))          # minutes, 0..49
  STAMP="${base:0:10}$(printf '%02d' $(( 10#${base:10:2} % 10 + bump )))"
  di=$((di + 1))
}

# dated <path> - stamp with the next date in the walk
dated() { next_stamp; touch -t "$STAMP" "$1"; }

# mk <path> <size> - sized file plus a rotating timestamp
mk() { sized "$1" "$2"; dated "$1"; }

# --- Projects/aurora-cms ---------------------------------------------------

A="$TARGET/Projects/aurora-cms"
mkdir -p "$A"/{src,migrations,assets,docs,target/release}

cat > "$A/Cargo.toml" <<'EOF'
[package]
name = "aurora-cms"
version = "2.4.1"
edition = "2021"
authors = ["Aurora Team <team@aurora.example>"]
license = "MIT"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
sqlx = { version = "0.7", features = ["postgres", "runtime-tokio"] }
serde = { version = "1", features = ["derive"] }
tracing = "0.1"
EOF

cat > "$A/README.md" <<'EOF'
# Aurora CMS

Content service behind the marketing site and the docs portal.

## Running locally

    cp .env.example .env
    docker compose up -d postgres
    cargo run

The server listens on :8080. Migrations run automatically at boot unless
AURORA_SKIP_MIGRATIONS is set.

## Layout

    src/main.rs      binary entry point, tracing setup
    src/routes.rs    HTTP surface
    src/db.rs        connection pool and query helpers
    src/config.rs    environment parsing
    migrations/      sqlx migrations, applied in order

## Deploying

Tagged builds go out through scripts/deploy.sh. Staging deploys on every
merge to main; production is a manual promotion.
EOF

cat > "$A/src/config.rs" <<'EOF'
use std::env;
use std::time::Duration;

/// Runtime configuration, read once at boot.
///
/// Every field has a working default so a fresh checkout starts without a
/// .env file; only the database URL is required in production.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    pub pool_size: u32,
    pub request_timeout: Duration,
    pub asset_root: String,
    pub skip_migrations: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            database_url: require("DATABASE_URL")?,
            bind_addr: optional("BIND_ADDR", "0.0.0.0:8080"),
            pool_size: optional("POOL_SIZE", "16").parse().unwrap_or(16),
            request_timeout: Duration::from_secs(
                optional("REQUEST_TIMEOUT_SECS", "30").parse().unwrap_or(30),
            ),
            asset_root: optional("ASSET_ROOT", "./assets"),
            skip_migrations: env::var("AURORA_SKIP_MIGRATIONS").is_ok(),
        })
    }
}

fn require(key: &str) -> Result<String, ConfigError> {
    env::var(key).map_err(|_| ConfigError::Missing(key.to_string()))
}

fn optional(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

#[derive(Debug)]
pub enum ConfigError {
    Missing(String),
}
EOF

cat > "$A/docs/architecture.md" <<'EOF'
# Architecture

Aurora is a single Rust binary in front of Postgres. There is no queue, no
cache tier and no second service; everything below is one process.

## Request path

A request enters through axum, picks up a request id in middleware, and is
routed by `src/routes.rs`. Handlers borrow a connection from the pool for the
shortest span they can and never hold one across an await that can block on
the network.

    client -> axum -> middleware (request id, tracing, timeout)
           -> handler -> sqlx pool -> postgres

## Why one binary

The content model is small and the read path is almost entirely cached by the
CDN. Splitting it would buy independent deploys we do not need and cost us the
transactional writes we do.

## Migrations

Migrations live in `migrations/` and are applied in filename order at boot.
They are forward-only: a bad migration is fixed by writing another one, never
by editing a file that has already run somewhere.

## Assets

Uploads are written to object storage and referenced by key. The `assets/`
directory in the repo holds only build-time art - logos, the favicon and the
open-graph image.

## Observability

Structured logs via tracing, one span per request. There is no metrics
endpoint yet; the load balancer's own counters have been enough so far.
EOF

# A real service has more than four modules, and a panel showing eight entries
# in a two-column view is mostly empty space. Density is what makes a file
# manager look like a file manager.
for f in main.rs lib.rs routes.rs db.rs error.rs auth.rs cache.rs \
         handlers.rs middleware.rs models.rs pages.rs render.rs \
         search.rs sessions.rs storage.rs telemetry.rs uploads.rs \
         validation.rs; do
  mk "$A/src/$f" "$((RANDOM % 9 + 2))k"
done
mkdir -p "$A/tests"
for f in api_smoke.rs auth_flow.rs migrations.rs render_golden.rs; do
  mk "$A/tests/$f" "$((RANDOM % 6 + 1))k"
done
dated "$A/src/config.rs"
dated "$A/README.md"; dated "$A/Cargo.toml"
dated "$A/docs/architecture.md"

printf 'DATABASE_URL=postgres://aurora:aurora@localhost/aurora\nBIND_ADDR=0.0.0.0:8080\nPOOL_SIZE=16\n' > "$A/.env"
printf 'target/\n.env\n*.log\n.DS_Store\n' > "$A/.gitignore"
dated "$A/.env"; dated "$A/.gitignore"
cp "$REPO_ROOT/LICENSE" "$A/LICENSE"; dated "$A/LICENSE"

mk "$A/migrations/0001_initial_schema.sql" 3k
mk "$A/migrations/0002_add_authors.sql" 1k
mk "$A/migrations/0003_page_revisions.sql" 4k
mk "$A/migrations/0004_search_index.sql" 2k
mk "$A/migrations/0005_media_library.sql" 2k
mk "$A/migrations/0006_drop_legacy_tags.sql" 1k
mk "$A/migrations/0007_page_slugs.sql" 2k

# Extra top-level files: this directory is the one most often on screen, and a
# mixed listing is what shows the category colouring doing its job.
mk "$A/Cargo.lock" 148k
mk "$A/docker-compose.yml" 2k
mk "$A/Dockerfile" 1k
mk "$A/rustfmt.toml" 256
mk "$A/CHANGELOG.md" 24k
mk "$A/CONTRIBUTING.md" 6k
mk "$A/openapi.json" 96k
mk "$A/aurora.log" 2m
mk "$A/coverage.html" 780k

mk "$A/assets/logo.svg" 12k
mk "$A/assets/favicon.ico" 15k
cp "$REPO_ROOT/src-tauri/icon-master.png" "$A/assets/hero@2x.png"
dated "$A/assets/hero@2x.png"

mk "$A/docs/api-reference.pdf" 840k
mk "$A/docs/changelog.md" 18k
mk "$A/docs/deployment.md" 11k
mk "$A/docs/data-model.md" 9k
mk "$A/docs/runbook.md" 14k
mk "$A/docs/schema.png" 620k

# A real Mach-O binary, so hex mode has something honest to show.
cp /bin/echo "$A/target/release/aurora-cms" 2>/dev/null || sized "$A/target/release/aurora-cms" 4m
chmod 755 "$A/target/release/aurora-cms"
dated "$A/target/release/aurora-cms"

# --- Projects/harbor-analytics ---------------------------------------------

H="$TARGET/Projects/harbor-analytics"
mkdir -p "$H"/{data,notebooks,reports}
mk "$H/data/sessions-2026-06.csv" 14m
mk "$H/data/sessions-2026-07.csv" 16m
mk "$H/data/events.parquet" 62m
mk "$H/data/schema.json" 8k
mk "$H/notebooks/cohort-retention.ipynb" 420k
mk "$H/notebooks/funnel-analysis.ipynb" 310k
mk "$H/notebooks/scratch.ipynb" 44k
mk "$H/reports/Q2-2026-summary.pdf" 2m
mk "$H/reports/kpis.xlsx" 186k
mk "$H/README.md" 2k
mk "$H/requirements.txt" 512

# --- Projects/website-redesign ---------------------------------------------

W="$TARGET/Projects/website-redesign"
mkdir -p "$W/mockups"
mk "$W/index.html" 22k
mk "$W/styles.css" 31k
mk "$W/app.js" 47k
mk "$W/mockups/home-v3.png" 3m
mk "$W/mockups/pricing-v2.png" 2m
mk "$W/mockups/nav-states.png" 1m

# --- Media -----------------------------------------------------------------

P="$TARGET/Media/Photos/2026-06 Lisbon"
mkdir -p "$P"
# Many small-ish files: operation progress is count-based, so a folder with a
# real number of entries is what makes the progress bar visibly move on camera.
for n in $(seq 4821 4852); do mk "$P/IMG_$n.jpg" "$((RANDOM % 3000 + 1800))k"; done
mk "$P/lisbon-notes.txt" 3k
# A trip folder whose photos are dated across ten months reads as fake the
# moment anyone sorts by date, so these are re-stamped across the trip itself.
pi=0
for f in "$P"/IMG_*.jpg; do
  touch -t "202606$(printf '%02d' $((pi / 5 + 11)))$(printf '%02d%02d' $((pi % 12 + 8)) $((pi * 7 % 60)))" "$f"
  pi=$((pi + 1))
done
touch -t 202606211930 "$P/lisbon-notes.txt"

# A destination that already holds three of the photo names, so copying from
# the trip folder into it raises a real collision dialog on camera rather than
# one staged with a doctored screenshot.
S="$TARGET/Media/Photos/Selects"
mkdir -p "$S"
for n in 4821 4822 4823; do mk "$S/IMG_$n.jpg" "$((RANDOM % 3000 + 1800))k"; done
mk "$S/contact-sheet.pdf" 4m
touch -t 202606240915 "$S"/IMG_*.jpg

# An empty destination. The video copies into this so the operation runs to
# completion on camera; the collision dialog is told by the screenshot instead,
# where a modal that needs dismissing cannot derail the next forty seconds.
mkdir -p "$TARGET/Media/Photos/Export"

R="$TARGET/Media/Screen Recordings"
mkdir -p "$R"
mk "$R/onboarding-flow.mov" 148m
mk "$R/bug-repro-1043.mov" 24m
mk "$R/checkout-regression.mp4" 61m

# --- Archives --------------------------------------------------------------

mkdir -p "$TARGET/Archives"
mk "$TARGET/Archives/aurora-cms-2026-07-14.tar.gz" 38m
mk "$TARGET/Archives/photos-backup.zip" 1200m
mk "$TARGET/Archives/node_modules.tar.bz2" 210m
mk "$TARGET/Archives/logs-2026-05.tar.xz" 7m

# --- Documents -------------------------------------------------------------

mkdir -p "$TARGET/Documents"/{Invoices,Contracts}
mk "$TARGET/Documents/Invoices/2026-04 Northwind.pdf" 88k
mk "$TARGET/Documents/Invoices/2026-05 Acme.pdf" 92k
mk "$TARGET/Documents/Invoices/2026-06 Acme.pdf" 91k
mk "$TARGET/Documents/Invoices/2026-07 Globex.pdf" 104k
mk "$TARGET/Documents/Contracts/msa-northwind.pdf" 340k
mk "$TARGET/Documents/Contracts/nda-signed.pdf" 210k
mk "$TARGET/Documents/tax-return-2025.pdf" 1m
mk "$TARGET/Documents/reading-list.md" 4k

# --- scripts ---------------------------------------------------------------

mkdir -p "$TARGET/scripts"
cat > "$TARGET/scripts/deploy.sh" <<'EOF'
#!/usr/bin/env bash
# Promote a tagged build to an environment. Staging is automatic on merge;
# production is always a deliberate run of this script.
set -euo pipefail

ENVIRONMENT="${1:-staging}"
TAG="${2:-$(git describe --tags --abbrev=0)}"

case "$ENVIRONMENT" in
  staging|production) ;;
  *) echo "unknown environment: $ENVIRONMENT" >&2; exit 1 ;;
esac

echo "==> deploying $TAG to $ENVIRONMENT"

cargo build --release --locked
./scripts/run-migrations.sh "$ENVIRONMENT"

rsync -az --delete \
  target/release/aurora-cms \
  "deploy@$ENVIRONMENT.aurora.example:/srv/aurora/bin/"

ssh "deploy@$ENVIRONMENT.aurora.example" 'systemctl restart aurora'

echo "==> waiting for health check"
for _ in $(seq 1 30); do
  if curl -sf "https://$ENVIRONMENT.aurora.example/healthz" >/dev/null; then
    echo "==> $TAG is live on $ENVIRONMENT"
    exit 0
  fi
  sleep 2
done

echo "health check never passed; rolling back" >&2
exit 1
EOF
chmod 755 "$TARGET/scripts/deploy.sh"
dated "$TARGET/scripts/deploy.sh"

mk "$TARGET/scripts/backup.sh" 2k; chmod 755 "$TARGET/scripts/backup.sh"
mk "$TARGET/scripts/rotate-logs.sh" 1k; chmod 755 "$TARGET/scripts/rotate-logs.sh"
mk "$TARGET/scripts/notes.txt" 6k

# --- hidden ----------------------------------------------------------------

mkdir -p "$TARGET/.config/aurora"
mk "$TARGET/.config/aurora/credentials.toml" 1k
mk "$TARGET/.config/aurora/defaults.toml" 2k
printf 'export AURORA_ENV=local\nexport PATH="$HOME/.cargo/bin:$PATH"\n' > "$TARGET/.profile"
dated "$TARGET/.profile"

# Stamp the directories themselves last: creating files inside a folder bumps
# its own mtime, so any earlier touch would have been overwritten.
find "$TARGET" -type d -print0 | while IFS= read -r -d '' d; do
  dated "$d"
done
touch -t 202601010000 "$TARGET/$MARKER"

echo "demo tree built at $TARGET"
find "$TARGET" -type f | wc -l | xargs printf '  %s files\n'
find "$TARGET" -type d | wc -l | xargs printf '  %s directories\n'
