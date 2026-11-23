#!/usr/bin/env bash
# End-to-end contract test: the Rust sync client against the REAL Worker (wrangler dev, local D1).
# Google sign-in can't run here, so sign-ins are seeded straight into the local D1 (users and
# devices with refresh tokens chosen at random for this run). Nothing leaves this machine.
#
#   cloud/scripts/e2e.sh            # from anywhere
#
# Needs: node/npm (cloud/node_modules installed: `cd cloud && npm ci`), openssl, cargo, curl.
# Temporary files: cloud/.dev.vars (git-ignored, deleted on exit) and a temp dir (deleted on exit).
set -uo pipefail
# Local traffic must never go through a proxy.
export NO_PROXY="127.0.0.1,localhost${NO_PROXY:+,$NO_PROXY}" no_proxy="127.0.0.1,localhost${no_proxy:+,$no_proxy}"

CLOUD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$CLOUD_DIR/.." && pwd)"
PORT="${WISPER_E2E_PORT:-8799}"
SLOTS="${WISPER_E2E_SLOTS:-16}"   # independent users (each with computers A and B) for the scenarios
TMP="$(mktemp -d "${TMPDIR:-/tmp}/wisper-e2e.XXXXXX")"
STATE="$TMP/state"
DEVVARS="$CLOUD_DIR/.dev.vars"
WRANGLER_PID=""

descendants() { # all child processes of $1, recursively
  local c
  for c in $(pgrep -P "$1" 2>/dev/null); do echo "$c"; descendants "$c"; done
}

cleanup() {
  if [ -n "$WRANGLER_PID" ]; then
    # wrangler spawns workerd: stop the whole tree, even if wrangler already exited.
    local kids; kids="$(descendants "$WRANGLER_PID")"
    kill -TERM -- "-$WRANGLER_PID" 2>/dev/null; kill -TERM "$WRANGLER_PID" $kids 2>/dev/null
    for _ in 1 2 3 4 5; do
      curl -s -o /dev/null -m1 "http://127.0.0.1:$PORT/" || break
      sleep 1
    done
    kill -KILL -- "-$WRANGLER_PID" 2>/dev/null; kill -KILL "$WRANGLER_PID" $kids 2>/dev/null
  fi
  rm -f "$DEVVARS"
  rm -rf "$TMP"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

if [ -e "$DEVVARS" ]; then
  echo "Refusing to run: $DEVVARS already exists (it may hold real secrets). Move it away first." >&2
  exit 2
fi
if curl -s -o /dev/null "http://127.0.0.1:$PORT/"; then
  echo "Refusing to run: something is already listening on port $PORT." >&2
  exit 2
fi

b64url_sha256() { printf %s "$1" | openssl dgst -sha256 -binary | openssl base64 -A | tr '+/' '-_' | tr -d '='; }
rand_token()   { openssl rand 32 | openssl base64 -A | tr '+/' '-_' | tr -d '='; }

# Test-only secrets, generated now, never printed.
umask 077
cat > "$DEVVARS" <<VARS
MASTER_KEY=$(openssl rand -base64 32)
TOKEN_SECRET=$(openssl rand -base64 32)
GOOGLE_CLIENT_ID=dummy-client-id.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=dummy-client-secret
VARS
umask 022

cd "$CLOUD_DIR" || exit 1
WR="npx --no-install wrangler"

echo "== applying migrations to a fresh local D1"
$WR d1 migrations apply wisper --local --persist-to "$STATE" >"$TMP/migrate.log" 2>&1 || { cat "$TMP/migrate.log"; exit 1; }

echo "== seeding $SLOTS users x 2 computers"
NOW_MS=$(( $(date +%s) * 1000 ))
TOKENS=""
SEED="$TMP/seed.sql"
: > "$SEED"
for n in $(seq 1 "$SLOTS"); do
  echo "INSERT INTO users (id, google_sub, email, name, created_at) VALUES ('user-$n', 'google-sub-$n', 'e2e$n@example.test', 'E2E User $n', $NOW_MS);" >> "$SEED"
  for d in a b; do
    tok="$(rand_token)"
    h="$(b64url_sha256 "$tok")"
    echo "INSERT INTO devices (id, user_id, name, refresh_hash, prev_refresh_hash, created_at, last_seen, revoked) VALUES ('dev-$n-$d', 'user-$n', 'Computer $d', '$h', NULL, $NOW_MS, $NOW_MS, 0);" >> "$SEED"
    TOKENS="$TOKENS${TOKENS:+,}$n$d=$tok"
  done
done
$WR d1 execute wisper --local --persist-to "$STATE" --file "$SEED" >"$TMP/seed.log" 2>&1 || { cat "$TMP/seed.log"; exit 1; }

echo "== starting the Worker on 127.0.0.1:$PORT"
setsid $WR dev --local --persist-to "$STATE" --port "$PORT" --ip 127.0.0.1 >"$TMP/wrangler.log" 2>&1 &
WRANGLER_PID=$!
for i in $(seq 1 120); do
  if curl -s "http://127.0.0.1:$PORT/" | grep -q wisper-api; then break; fi
  if ! kill -0 "$WRANGLER_PID" 2>/dev/null; then echo "wrangler died:"; cat "$TMP/wrangler.log"; exit 1; fi
  sleep 1
  if [ "$i" = 120 ]; then echo "Worker did not start:"; cat "$TMP/wrangler.log"; exit 1; fi
done

echo "== running the contract test"
cd "$REPO_DIR" || exit 1
WISPER_E2E_URL="http://127.0.0.1:$PORT" WISPER_E2E_TOKENS="$TOKENS" WISPER_E2E_STATE="$STATE" \
  cargo test -p wisper-core --test cloud_e2e -- --ignored --nocapture --test-threads=1
RC=$?
if [ "$RC" -ne 0 ]; then echo "---- last Worker log lines ----"; tail -n 40 "$TMP/wrangler.log"; fi
exit "$RC"
