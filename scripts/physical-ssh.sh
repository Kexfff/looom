#!/usr/bin/env bash
# Access to the explicitly authorized N100 pilot, never the local computer.
set -Eeuo pipefail
project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
looom_ssh_terminal=(-T)
if [[ ${1:-} == --tty ]]; then
    shift
    looom_ssh_terminal=(-t)
elif [[ $# == 0 ]]; then
    looom_ssh_terminal=(-t)
fi
exec ssh "${looom_ssh_terminal[@]}" -i "$project/.local/physical-access/id_ed25519" \
    -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=yes \
    -o "UserKnownHostsFile=$project/.local/physical-known-hosts" \
    root@192.168.1.200 "$@"
