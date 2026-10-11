# Keep the serial console available for diagnostics and show the menu on screen.
if [[ -z "$SSH_CONNECTION" && "$(tty)" == /dev/tty1 ]]; then
    /usr/local/bin/looom-live
fi
