#!/usr/bin/env bash
# Management access to the explicitly configured prototype VM.
set -Eeuo pipefail
project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
login=${VM_SSH_USER:-root}
[[ $login == root || $login == codex ]]
if [[ $# == 1 ]]; then
    remote="set -e; $1"
    if [[ $login == codex ]]; then
        printf -v remote 'sudo -n bash -c %q' "$remote"
    fi
    set -- "$remote"
fi
exec ssh -T -i "$project/.local/vm-access/id_ed25519" \
    -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=yes \
    -o "UserKnownHostsFile=$project/.local/vm-access/known_hosts" "$login@192.168.122.91" "$@"
