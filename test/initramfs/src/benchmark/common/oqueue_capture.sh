#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

# Runs a command while recording exported OQueues from the OQueue filesystem (`/oqueues`), for the
# policy explorer (`ldos/policy_explorer/orchestrator/mariposa.py`).
#
# Usage: oqueue_capture.sh PATHS -- CMD [ARGS...]
#   PATHS: comma-separated OQueue paths to record (e.g. `scheduler.events,io.block_io.bio_completion[0]`),
#   or `-` for none.
#
# Writes to /host/oqueue_capture/ after CMD exits (records are kept in RAM during the run):
#   - `exports.yaml`: every exported OQueue, listed one second after CMD starts (so queues CMD
#     creates are included), as a multi-document YAML file: one document per OQueue (its
#     `metadata.yaml`: `name`, `type_name`), then one `strong_observe:` document listing the paths of
#     the available `strong_observe` streams;
#   - `<path>.cbor`: every record of each requested OQueue produced while CMD ran (a CBOR stream).
# Exits with CMD's status.

OUT=/host/oqueue_capture
STAGING=/tmp/oqueue_capture
PATHS=$1
shift
[ "$1" = "--" ] && shift

rm -rf "$OUT" "$STAGING"
mkdir -p "$OUT" "$STAGING"

# Start one reader per requested OQueue before CMD, so none of CMD's records are missed.
READERS=""
if [ "$PATHS" != "-" ]; then
    OLD_IFS=$IFS
    IFS=,
    for P in $PATHS; do
        # `a.b[3].c` lives at /oqueues/a/b/3/c.
        DIR=/oqueues/$(echo "$P" | sed 's/\[/\//g; s/\]//g; s/\./\//g')
        if [ -e "$DIR/strong_observe" ]; then
            # Keep the stream open and read what accumulated every 100 ms, rather than waking on
            # every record (a reader woken per record perturbs the system it observes, e.g. each of
            # its wake-ups is itself a scheduler event).
            (
                exec 3<"$DIR/strong_observe"
                while :; do
                    dd bs=1048576 count=1 <&3 >>"$STAGING/$P.cbor" 2>/dev/null
                    sleep 0.1
                done
            ) &
            READERS="$READERS $!"
        else
            echo "oqueue_capture: no exported OQueue '$P' (no $DIR/strong_observe)" >>"$STAGING/errors.txt"
        fi
    done
    IFS=$OLD_IFS
fi

(
    sleep 1
    for M in $(find /oqueues -name metadata.yaml); do
        echo ---
        cat "$M"
    done
    echo ---
    echo "strong_observe:"
    find /oqueues -name strong_observe | sed 's/^/  - /'
) >"$STAGING/exports.yaml" 2>>"$STAGING/errors.txt" &
LISTER=$!

RC=0
"$@" || RC=$?

wait "$LISTER" 2>/dev/null
for R in $READERS; do
    kill "$R" 2>/dev/null
done
cp "$STAGING"/* "$OUT/" 2>/dev/null
sync
exit "$RC"
