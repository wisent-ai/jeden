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
#   ./headless-mtls-material.sh --out <material-dir> ...
#   jeden headless <addr> <material-dir>/server-cert.pem \
#         <material-dir>/server-key.pem <material-dir>/ca.pem \
#         <material-dir>/identity-map.json <limits.json>
#   ./headless-drive.sh --material <material-dir> --addr <addr> --page <N> \
#         [--session <id> --prompt <text> | --session <id> --request <id>]
#
# <addr> is the host:port the daemon binds; <limits.json> is the operator's
# declaration of the host's bounds (`jeden headless` documents its fields).
# --page is how many sessions, turns or events each read asks for: a client
# chooses it, and the daemon refuses one below one.
#
# Without --session the sequence stops after session/list, which is what a
# client does on a host it has just been granted. session/list and session/open
# answer `access_denied` unless the identity map entry for this certificate
# names `workspaces`; that refusal is the expected output of a run without one.
# With --prompt the session is opened, its history read and the prompt sent;
# the daemon names the request, so a replay is asked for with --request and
# the id the prompt answered with.
#
# Requires: openssl (1.1.1+ for -alpn), a running `jeden headless`.
set -eu

# A command used wrongly, as sysexits(3) names it: https://man.freebsd.org/cgi/man.cgi?query=sysexits "EX_USAGE (64)"
EX_USAGE=64
# A missing input file, as sysexits(3) names it: https://man.freebsd.org/cgi/man.cgi?query=sysexits "EX_NOINPUT (66)"
EX_NOINPUT=66
USAGE='Usage: headless-drive.sh --material <dir> --addr <host:port> --page <N> [--session <id> --prompt <text> | --session <id> --request <id>]'
PROTOCOL='jeden.session.v1'

refuse() {
    printf '%s\n%s\n' "$1" "$USAGE" >&2
    exit "$EX_USAGE"
}

MATERIAL=
ADDR=
PAGE=
SESSION=
PROMPT=
REQUEST=
expect=
for argument in "$@"; do
    case $expect in
        material) MATERIAL=$argument; expect=; continue ;;
        addr) ADDR=$argument; expect=; continue ;;
        page) PAGE=$argument; expect=; continue ;;
        session) SESSION=$argument; expect=; continue ;;
        prompt) PROMPT=$argument; expect=; continue ;;
        request) REQUEST=$argument; expect=; continue ;;
    esac
    case $argument in
        --material) expect=material ;;
        --addr) expect=addr ;;
        --page) expect=page ;;
        --session) expect=session ;;
        --prompt) expect=prompt ;;
        --request) expect=request ;;
        *) refuse "unknown argument: $argument" ;;
    esac
done
[ -z "$expect" ] || refuse "--$expect needs a value"
[ -n "$MATERIAL" ] || refuse "--material names no directory"
[ -n "$ADDR" ] || refuse "--addr names no listener"
[ -n "$PAGE" ] || refuse "--page names no page size"
if [ -n "$PROMPT" ] && [ -n "$REQUEST" ]; then
    refuse "--prompt sends a prompt and --request watches one; give one of them"
fi
if [ -z "$SESSION" ] && { [ -n "$PROMPT" ] || [ -n "$REQUEST" ]; }; then
    refuse "--prompt and --request need --session"
fi
if [ -n "$SESSION" ] && [ -z "$PROMPT" ] && [ -z "$REQUEST" ]; then
    refuse "--session needs --prompt or --request"
fi

for name in client-cert.pem client-key.pem ca.pem; do
    [ -r "$MATERIAL/$name" ] || { printf 'missing %s in %s\n' "$name" "$MATERIAL" >&2; exit "$EX_NOINPUT"; }
done

# One request per line. `meta.idempotencyKey` is required on every request and
# `meta.traceId` is a plain string in the daemon's envelope, so both are always
# sent. The key carries a per-run stamp, because the daemon's idempotency store
# is durable: replaying yesterday's key returns yesterday's answer, which is the
# contract working and not what an example wants to demonstrate.
RUN="$(date +%s)-$$"
request() {
    printf '{"id":"%s","method":"%s","params":%s,"meta":{"protocolVersion":"%s","idempotencyKey":"%s-%s","traceId":"%s-%s"}}\n' \
        "$1" "$2" "$3" "$PROTOCOL" "$1" "$RUN" "$1" "$RUN"
}

{
    request req-readiness 'health/readiness' '{}'
    request req-list 'session/list' "$(printf '{"limit":%s}' "$PAGE")"
    if [ -n "$PROMPT" ]; then
        request req-open 'session/open' "$(printf '{"sessionId":"%s"}' "$SESSION")"
        request req-history 'session/history' "$(printf '{"sessionId":"%s","limit":%s}' "$SESSION" "$PAGE")"
        request req-prompt 'session/prompt' "$(printf '{"sessionId":"%s","prompt":"%s"}' "$SESSION" "$PROMPT")"
        printf '# rerun with the requestId above to watch it:\n' >&2
        printf '#   %s --material %s --addr %s --page %s --session %s --request <requestId>\n' \
            "$0" "$MATERIAL" "$ADDR" "$PAGE" "$SESSION" >&2
    elif [ -n "$REQUEST" ]; then
        # One replay read: an empty `events` list means the request is accepted
        # and still running, which is a different answer from a refusal. Run the
        # same line again to read what it has done since.
        request req-replay 'session/replay' \
            "$(printf '{"sessionId":"%s","requestId":"%s","limit":%s}' "$SESSION" "$REQUEST" "$PAGE")"
    fi
} | openssl s_client -connect "$ADDR" \
        -cert "$MATERIAL/client-cert.pem" \
        -key "$MATERIAL/client-key.pem" \
        -CAfile "$MATERIAL/ca.pem" \
        -alpn "$PROTOCOL" \
        -tls1_3 -quiet -ign_eof 2>&1
