{ pkgs, ... }:

let
  stateDir = "/var/lib/ui-box-state";
  ioThresholdBytes = 20 * 1024 * 1024;

  # The marker file names are the contract with the hypervisor's idle-stop
  # timer, which reads them from its side of the same share. Rename them here
  # and the host stops being able to tell a busy lab from a dead one.
  idleMark = pkgs.writeShellApplication {
    name = "uibox-idle-mark";

    runtimeInputs = with pkgs; [
      coreutils
      gawk
      iproute2
      systemd
      util-linux
    ];

    text = ''
      state=${stateDir}

      # Not `[ -d "$state" ] || exit 0`. An unmounted share leaves the mount
      # point behind as an ordinary directory, so the markers would be written
      # to the root overlay where the host cannot see them, and the host would
      # read the silence as "this guest booted a moment ago" forever. Refuse
      # rather than write somewhere nobody is reading.
      if ! mountpoint -q "$state"; then
          echo "$state is not a mount point: markers written here are invisible to the host" >&2
          exit 1
      fi

      touch "$state/alive"

      busy=0
      reasons=""

      mark_busy() {
          busy=1
          reasons="$reasons''${reasons:+,}$1"
      }

      if [ -f "$state/keepalive" ]; then
          until=$(cat "$state/keepalive" 2>/dev/null || echo 0)
          case "$until" in
              (*[!0-9]*|"") until=0 ;;
          esac
          [ "$until" -gt "$(date +%s)" ] && mark_busy keepalive
      fi

      if [ "$(loginctl list-sessions --no-legend 2>/dev/null | wc -l)" -gt 0 ]; then
          mark_busy sessions
      fi

      if [ "$(ss -Htn state established 2>/dev/null | grep -cv '127\.0\.0\.1')" -gt 0 ]; then
          mark_busy connections
      fi

      load=$(awk '{print $1}' /proc/loadavg)
      if awk -v l="$load" 'BEGIN { exit !(l >= 0.30) }'; then
          mark_busy "load=$load"
      fi

      disk=$(awk '{s += $6 + $10} END {print (s + 0) * 512}' /proc/diskstats)
      net=$(cat /sys/class/net/*/statistics/rx_bytes /sys/class/net/*/statistics/tx_bytes 2>/dev/null \
            | awk '{s += $1} END {print s + 0}')
      cur=$((disk + net))

      prev_file=/run/uibox-io-prev
      if [ -f "$prev_file" ]; then
          prev=$(cat "$prev_file" 2>/dev/null || echo 0)
          case "$prev" in
              (*[!0-9]*|"") prev=$cur ;;
          esac
          if [ "$((cur - prev))" -gt ${toString ioThresholdBytes} ]; then
              mark_busy io
          fi
      fi
      printf '%s' "$cur" > "$prev_file"

      if [ "$busy" -eq 1 ] || [ ! -f "$state/busy" ]; then
          touch "$state/busy"
      fi

      # A healthy run has to look different from a broken one. Without this both
      # answer with silence, and "the markers are being published" is then
      # indistinguishable from "the unit has not run since the share vanished".
      echo "alive; busy=$busy''${reasons:+ ($reasons)}"
    '';
  };
in
{
  systemd.services.uibox-idle-mark = {
    description = "Publish idle markers to the host state share";

    # Pulls the mount in and orders against it. Deliberately not paired with
    # AssertPathIsMountPoint: an assertion that does not hold leaves the unit
    # inactive rather than failed, so `systemctl --failed` stays clean and the
    # standard health check keeps answering "fine". Letting the script exit
    # non-zero instead is what puts the unit in the failed state that both
    # `systemctl --failed` and a degraded system-running report will show.
    unitConfig.RequiresMountsFor = stateDir;

    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${idleMark}/bin/uibox-idle-mark";
    };
  };

  systemd.timers.uibox-idle-mark = {
    description = "Publish idle markers every 30s";
    wantedBy = [ "timers.target" ];

    timerConfig = {
      OnBootSec = "15s";
      OnUnitActiveSec = "30s";
      AccuracySec = "5s";
    };
  };

  environment.systemPackages = [
    (pkgs.writeShellApplication {
      name = "uibox-keepalive";
      runtimeInputs = with pkgs; [
        coreutils
        util-linux
      ];

      text = ''
        state=${stateDir}

        # Unlike the timer, degrading is the right answer here: this wraps a
        # command someone is waiting on, and refusing to run it because the lab
        # cannot hold itself awake would be the worse failure. It says so on
        # stderr, which is what keeps it distinguishable from a held run.
        if ! mountpoint -q "$state"; then
            echo "uibox-keepalive: $state is not mounted, running without a keepalive" >&2
            exec "$@"
        fi

        holder=""
        trap 'rm -f "$state/keepalive"; [ -n "$holder" ] && kill "$holder" 2>/dev/null; true' EXIT

        while true; do
            date -d '+5 minutes' +%s > "$state/keepalive"
            sleep 60
        done &
        holder=$!

        "$@"
      '';
    })
  ];
}
