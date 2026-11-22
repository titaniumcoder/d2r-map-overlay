#!/usr/bin/env bash
# Deploys the overlay: release build, the ptrace capability, a snapshot of playstyles/ that the
# running binary reads (so editing the repo's playstyle files never hot-reloads half-done work
# into the live overlay), a restart, and the dashboard in the browser.
#
# Usage: scripts/deploy.sh   (restarts a running overlay; the setcap step needs a cached
# sudo grant: run `sudo -v` first if it asks)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$root/target/release/d2r-map-overlay"
data="${XDG_DATA_HOME:-$HOME/.local/share}/d2r-map-overlay"
snapshot="$data/playstyles-deployed"
log="$data/overlay.log"

cd "$root"
cargo build --release

# The capability first: if it fails, nothing else has changed yet (the running overlay keeps
# its binary and its playstyle snapshot).
if ! sudo -n setcap cap_sys_ptrace=ep "$bin"; then
    echo "deploy: setcap needs sudo; run: sudo setcap cap_sys_ptrace=ep $bin" >&2
    echo "deploy: then re-run this script (nothing was changed or restarted)" >&2
    exit 1
fi

dashboard_open() {
    [ -n "$(ss -Htn state established '( sport = :8471 )')" ]
}
open_dashboard=true
if dashboard_open; then
    open_dashboard=false
fi

# Stop the old overlay before touching its snapshot, and wait until it is gone (its port 8471
# must be free for the new one).
pkill -x d2r-map-overlay || true
for _ in $(seq 50); do
    pgrep -x d2r-map-overlay > /dev/null || break
    sleep 0.1
done
if pgrep -x d2r-map-overlay > /dev/null; then
    echo "deploy: the old overlay did not exit; stop it and re-run" >&2
    exit 1
fi

# Replace the snapshot as a whole: copy next to it, then swap.
mkdir -p "$data"
rm -rf "$snapshot.new"
cp -a "$root/playstyles" "$snapshot.new"
rm -rf "$snapshot"
mv "$snapshot.new" "$snapshot"

nohup env D2R_PLAYSTYLES_DIR="$snapshot" "$bin" > "$log" 2>&1 &
pid=$!
# Report success only once the dashboard answers.
for _ in $(seq 50); do
    if curl -s -o /dev/null http://localhost:8471/dashboard; then
        echo "deploy: overlay running (pid $pid), playstyles from $snapshot, log in $log"
        if $open_dashboard; then
            xdg-open http://localhost:8471/dashboard > /dev/null 2>&1 || true
        else
            echo "deploy: dashboard already open; it reloads itself"
        fi
        exit 0
    fi
    kill -0 "$pid" 2> /dev/null || break
    sleep 0.2
done
echo "deploy: the overlay did not come up; see $log" >&2
exit 1
