#!/usr/bin/env bash
# Requires an actual logged-in local Plasma session.
set -Eeuo pipefail
expected_release=${1:-desktop}
[[ $(cat /etc/looom/release-id) == "$expected_release" ]]
systemctl is-active sddm
wayland_session=no
while read -r session; do
    if [[ $(loginctl show-session "$session" -p Type --value) == wayland && \
          $(loginctl show-session "$session" -p Active --value) == yes ]]; then
        wayland_session=yes
        loginctl show-session "$session" -p Name -p User -p Type -p Active -p Seat
    fi
done < <(loginctl list-sessions --no-legend | awk '$3 == "codex" {print $1}')
[[ $wayland_session == yes ]]
for unit in plasma-plasmashell.service plasma-kwin_wayland.service \
    pipewire.service pipewire-pulse.service wireplumber.service; do
    [[ $(systemctl --user -M codex@.host is-active "$unit") == active ]]
    printf '%s: active\n' "$unit"
done
[[ -z $(systemctl --user -M codex@.host --failed --no-legend --plain) ]]
[[ $(stat -c %u:%g /home/codex/.config/plasma-org.kde.plasma.desktop-appletsrc) == 1000:1000 ]]
pacman -Q plasma-desktop plasma-workspace sddm pipewire wireplumber
printf '\nPASS: local Plasma Wayland session and audio services\n'
