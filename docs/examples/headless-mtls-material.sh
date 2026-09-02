#!/usr/bin/env sh
# Generate demo mTLS material for `jeden headless` — companion to
# docs/headless.md.
#
# Produces, in a fresh directory (default ./headless-demo-material):
#   ca.pem / ca-key.pem          demo client CA
#   server-cert.pem / server-key.pem   server identity (CN=localhost)
#   client-cert.pem / client-key.pem   client identity, URI SAN
#   identity-map.json            maps the client SAN to principal + tenant
#   revoked-serials.txt          empty revocation list
#
# The daemon then starts as:
#   jeden headless 127.0.0.1:4433 server-cert.pem server-key.pem ca.pem \
#         identity-map.json revoked-serials.txt
#
# Demo material only — never use it outside a test bench.
# Requires: openssl.
set -eu

OUT="${1:-headless-demo-material}"
SAN_URI="${SAN_URI:-spiffe://demo/agent-1}"
PRINCIPAL="${PRINCIPAL:-agent-1}"
TENANT="${TENANT:-tenant-1}"
# Absolute directories this client may read and continue the host's own sessions
# in, colon-separated. Empty is the default and means the tenant sees only the
# sessions it creates through the daemon, in its own scratch workspace.
WORKSPACES="${WORKSPACES:-}"

mkdir -p "$OUT"
cd "$OUT"

echo "== demo client CA"
openssl req -x509 -newkey rsa:2048 -sha256 -days 7 -nodes \
  -keyout ca-key.pem -out ca.pem -subj "/CN=jeden-demo-ca" 2>/dev/null

echo "== server certificate (CN=localhost, SAN DNS:localhost)"
openssl req -newkey rsa:2048 -nodes -keyout server-key.pem \
  -out server.csr -subj "/CN=localhost" 2>/dev/null
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca-key.pem \
  -CAcreateserial -days 7 -sha256 -out server-cert.pem \
  -extfile /dev/stdin <<EOF 2>/dev/null
subjectAltName = DNS:localhost, IP:127.0.0.1
EOF

echo "== client certificate with mapped URI SAN ($SAN_URI)"
openssl req -newkey rsa:2048 -nodes -keyout client-key.pem \
  -out client.csr -subj "/CN=$PRINCIPAL" 2>/dev/null
openssl x509 -req -in client.csr -CA ca.pem -CAkey ca-key.pem \
  -CAcreateserial -days 7 -sha256 -out client-cert.pem \
  -extfile /dev/stdin <<EOF 2>/dev/null
subjectAltName = URI:$SAN_URI
EOF
rm -f server.csr client.csr

echo "== identity map"
workspaces_json=""
if [ -n "$WORKSPACES" ]; then
    workspaces_json=$(printf '%s' "$WORKSPACES" | awk -F: '{
        for (i = 1; i <= NF; i++) if (length($i)) {
            printf "%s\"%s\"", (started++ ? ", " : ""), $i
        }
    }')
    workspaces_json=", \"workspaces\": [$workspaces_json]"
fi
cat > identity-map.json <<EOF
[
  { "san": "$SAN_URI", "principal": "$PRINCIPAL", "tenant": "$TENANT"$workspaces_json }
]
EOF
cat identity-map.json

: > revoked-serials.txt

echo "== done; material in $PWD"
echo "start: jeden headless 127.0.0.1:4433 server-cert.pem server-key.pem ca.pem identity-map.json revoked-serials.txt"
