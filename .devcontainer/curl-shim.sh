#!/bin/bash
# curl shim — injects --resolve for .localhost hosts so RFC 6761 curl behaviour
# (which hardcodes loopback for .localhost) doesn't break the verify loop.
# For any other host execs the real curl unchanged.

resolves=()
for arg in "$@"; do
    case "$arg" in
        http://*.localhost/*|https://*.localhost/*|http://*.localhost|https://*.localhost)
            host="${arg#*://}"; host="${host%%/*}"; host="${host%%:*}"
            port="80"
            [[ "$arg" == https://* ]] && port="443"
            if [[ "$arg" =~ ://[^/:]+:([0-9]+) ]]; then port="${BASH_REMATCH[1]}"; fi
            gw="$(getent ahostsv4 host.docker.internal 2>/dev/null | awk 'NR==1{print $1}')"
            gw="${gw:-172.17.0.1}"
            resolves+=("--resolve" "${host}:${port}:${gw}")
            ;;
    esac
done
exec /usr/bin/curl "${resolves[@]}" "$@"
