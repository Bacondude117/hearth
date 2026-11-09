#!/usr/bin/env bash
# Stand-in for a game server. Behaviour is chosen by $1:
#   normal  - prints a ready banner, echoes commands, exits 0 on "stop"
#   crash   - prints a ready banner, exits 3 on "crash"
#   stubborn- ignores "stop" (to exercise the force-kill path)
mode="${1:-normal}"
echo "Starting fake server"
echo "[12:00:00 INFO]: Done (0.01s)! For help, type \"help\""
while IFS= read -r line; do
  case "$line" in
    stop)
      if [ "$mode" = "stubborn" ]; then echo "ignoring stop"; continue; fi
      echo "Stopping the server"; exit 0 ;;
    crash) echo "boom" >&2; exit 3 ;;
    join*) echo "[12:00:01 INFO]: ${line#join } joined the game" ;;
    leave*) echo "[12:00:02 INFO]: ${line#leave } left the game" ;;
    *) echo "echo: $line" ;;
  esac
done
