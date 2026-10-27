#!/usr/bin/env sh
# Generate demo mTLS material for `jeden headless` — companion to
# docs/headless.md.
#
#   ./headless-mtls-material.sh --out <dir> --days <N> --server-name <name> \
#         --san <uri> --principal <name> --tenant <name> [--workspaces <a:b:...>]
#
# Produces, in <dir>:
#   ca.pem / ca-key.pem                demo client CA
#   server-cert.pem / server-key.pem   server identity for <server-name>
#   client-cert.pem / client-key.pem   client identity, URI SAN <uri>
#   identity-map.json                  maps the client SAN to principal + tenant
#   revoked-serials.txt                empty revocation list
#
# --days is how long every certificate stays valid; --server-name is the DNS
# name clients dial the daemon by; --workspaces lists, colon-separated, the
# absolute directories this client may read and continue the host's own
# sessions in (without it the tenant sees only the sessions it creates through
# the daemon, in its own scratch workspace).
#
# The daemon then starts as:
#   jeden headless <addr> server-cert.pem server-key.pem ca.pem \
#         identity-map.json <limits.json> revoked-serials.txt
#
# Demo material only — never use it outside a test bench.
# Requires: openssl.
set -eu

# A command used wrongly, as sysexits(3) names it: https://man.freebsd.org/cgi/man.cgi?query=sysexits "EX_USAGE (64)"
EX_USAGE=64
# An RSA modulus giving 112 bits of security, the least NIST SP 800-57 Part 1
# Rev. 5 accepts through 2030: https://doi.org/10.6028/NIST.SP.800-57pt1r5 (Table 2, "RSA k = 2048")
RSA_BITS=2048
USAGE='Usage: headless-mtls-material.sh --out <dir> --days <N> --server-name <name> --san <uri> --principal <name> --tenant <name> [--workspaces <a:b:...>]'

refuse() {
    printf '%s\n%s\n' "$1" "$USAGE" >&2
    exit "$EX_USAGE"
}

OUT=
DAYS=
SERVER_NAME=
SAN_URI=
PRINCIPAL=
TENANT=
WORKSPACES=
expect=
for argument in "$@"; do
    case $expect in
        out) OUT=$argument; expect=; continue ;;
        days) DAYS=$argument; expect=; continue ;;
        server-name) SERVER_NAME=$argument; expect=; continue ;;
        san) SAN_URI=$argument; expect=; continue ;;
        principal) PRINCIPAL=$argument; expect=; continue ;;
        tenant) TENANT=$argument; expect=; continue ;;
        workspaces) WORKSPACES=$argument; expect=; continue ;;
    esac
    case $argument in
        --out) expect=out ;;
        --days) expect=days ;;
        --server-name) expect=server-name ;;
        --san) expect=san ;;
        --principal) expect=principal ;;
        --tenant) expect=tenant ;;
        --workspaces) expect=workspaces ;;
        *) refuse "unknown argument: $argument" ;;
    esac
done
[ -z "$expect" ] || refuse "--$expect needs a value"
[ -n "$OUT" ] || refuse "--out names no directory"
[ -n "$DAYS" ] || refuse "--days names no validity"
[ -n "$SERVER_NAME" ] || refuse "--server-name names no server"
[ -n "$SAN_URI" ] || refuse "--san names no client identity"
[ -n "$PRINCIPAL" ] || refuse "--principal names no principal"
[ -n "$TENANT" ] || refuse "--tenant names no tenant"

mkdir -p "$OUT"
cd "$OUT"

echo "== demo client CA"
openssl req -x509 -newkey "rsa:$RSA_BITS" -sha256 -days "$DAYS" -nodes \
  -keyout ca-key.pem -out ca.pem -subj "/CN=jeden-demo-ca"

echo "== server certificate (CN=$SERVER_NAME, SAN DNS:$SERVER_NAME)"
openssl req -newkey "rsa:$RSA_BITS" -nodes -keyout server-key.pem \
  -out server.csr -subj "/CN=$SERVER_NAME"
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca-key.pem \
  -CAcreateserial -days "$DAYS" -sha256 -out server-cert.pem \
  -extfile /dev/stdin <<EOF
subjectAltName = DNS:$SERVER_NAME
EOF

echo "== client certificate with mapped URI SAN ($SAN_URI)"
openssl req -newkey "rsa:$RSA_BITS" -nodes -keyout client-key.pem \
  -out client.csr -subj "/CN=$PRINCIPAL"
openssl x509 -req -in client.csr -CA ca.pem -CAkey ca-key.pem \
  -CAcreateserial -days "$DAYS" -sha256 -out client-cert.pem \
  -extfile /dev/stdin <<EOF
subjectAltName = URI:$SAN_URI
EOF
rm -f server.csr client.csr

echo "== identity map"
workspaces_json=""
if [ -n "$WORKSPACES" ]; then
    listed=""
    remaining=$WORKSPACES
    while [ -n "$remaining" ]; do
        entry=${remaining%%:*}
        case $remaining in
            *:*) remaining=${remaining#*:} ;;
            *) remaining= ;;
        esac
        [ -n "$entry" ] || continue
        if [ -n "$listed" ]; then
            listed="$listed, \"$entry\""
        else
            listed="\"$entry\""
        fi
    done
    workspaces_json=", \"workspaces\": [$listed]"
fi
cat > identity-map.json <<EOF
[
  { "san": "$SAN_URI", "principal": "$PRINCIPAL", "tenant": "$TENANT"$workspaces_json }
]
EOF
cat identity-map.json

: > revoked-serials.txt

echo "== done; material in $PWD"
echo "start: jeden headless <addr> server-cert.pem server-key.pem ca.pem identity-map.json <limits.json> revoked-serials.txt"
