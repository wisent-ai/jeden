#!/usr/bin/env sh
# Drive `jeden headless` over mutual TLS — companion to
# docs/examples/headless-mtls-material.sh and the sibling of rpc-drive.sh.
#
# Sends the canonical request sequence a network client uses — readiness,
# session/list, session/open, session/history, session/prompt, session/replay —
# as newline-delimited JSON inside the TLS session, and prints every frame the
# daemon answers. `openssl s_client` is the client here for the same reason
# rpc-drive.sh is a pipe: the point is the wire, not a library.
#
# The listener requires TLS 1.3, ALPN `jeden.session.v1` and a client
# certificate whose SAN the identity map names, so a plaintext or unmapped
# attempt is refused before any frame is read.
#
#   ./headless-mtls-material.sh /tmp/material
#   jeden headless 127.0.0.1:4433 /tmp/material/server-cert.pem \
#         /tmp/material/server-key.pem /tmp/material/ca.pem \
#         /tmp/material/identity-map.json
#   ./headless-drive.sh /tmp/material 127.0.0.1:4433 <session-id>
#
# With no session id the sequence stops after session/list, which is what a
# client does on a host it has just been granted. session/list and session/open
# answer `access_denied` unless the identity map entry for this certificate
# names `workspaces`; that refusal is the expected output of a run without one.
#
# Requires: openssl (1.1.1+ for -alpn), a running `jeden headless`.
set -eu

MATERIAL="${1:?Usage: headless-drive.sh <material-dir> [addr] [session-id] [prompt] [request-id]}"
ADDR="${2:-127.0.0.1:4433}"
SESSION="${3:-}"
PROMPT="${4:-Respond exactly: OK}"
# The daemon names the request, so a replay has to be asked for by the id the
# prompt answered with. Run once to prompt, then again with that id to watch it.
REQUEST="${5:-}"
PROTOCOL='jeden.session.v1'

for name in client-cert.pem client-key.pem ca.pem; do
    [ -r "$MATERIAL/$name" ] || { printf 'missing %s in %s\n' "$name" "$MATERIAL" >&2; exit 66; }
done

# One request per line. `meta.idempotencyKey` is required on every request and
# `meta.traceId` is a plain string in the daemon's envelope, so both are always
# sent. The key carries a per-run stamp, because the daemon's idempotency store
# is durable: replaying yesterday's key returns yesterday's answer, which is the
# contract working and not what an example wants to demonstrate.
RUN="${RUN:-$(date +%s)-$$}"
request() {
    printf '{"id":"%s","method":"%s","params":%s,"meta":{"protocolVersion":"%s","idempotencyKey":"%s-%s","traceId":"%s-%s"}}\n' \
        "$1" "$2" "$3" "$PROTOCOL" "$1" "$RUN" "$1" "$RUN"
}

{
    request req-readiness 'health/readiness' '{}'
    request req-list 'session/list' '{"limit":20}'
    if [ -n "$SESSION" ] && [ -z "$REQUEST" ]; then
        request req-open 'session/open' "$(printf '{"sessionId":"%s"}' "$SESSION")"
        request req-history 'session/history' "$(printf '{"sessionId":"%s","limit":10}' "$SESSION")"
        request req-prompt 'session/prompt' "$(printf '{"sessionId":"%s","prompt":"%s"}' "$SESSION" "$PROMPT")"
        printf '# rerun with the requestId above to watch it:\n' >&2
        printf '#   %s %s %s %s "%s" <requestId>\n' "$0" "$MATERIAL" "$ADDR" "$SESSION" "$PROMPT" >&2
    elif [ -n "$SESSION" ]; then
        # The prompt runs on the daemon's executor, so the replay is polled rather
        # than read once: an empty `events` list means the request is accepted and
        # still running, which is a different answer from a refusal. The poll stays
        # inside the daemon's own 30 s read deadline, because a connection idle past
        # it is closed with `malformed_frame: frame read deadline exceeded` — correct
        # of the daemon, and confusing as the last line of an example.
        for _ in 1 2 3 4 5 6 7 8; do
            request req-replay 'session/replay' \
                "$(printf '{"sessionId":"%s","requestId":"%s","limit":100}' "$SESSION" "$REQUEST")"
            sleep 3
        done
    fi
} | openssl s_client -connect "$ADDR" \
        -cert "$MATERIAL/client-cert.pem" \
        -key "$MATERIAL/client-key.pem" \
        -CAfile "$MATERIAL/ca.pem" \
        -alpn "$PROTOCOL" \
        -tls1_3 -quiet -ign_eof 2>&1
