{ pkgs, ... }:

let
  user = "fredrir";
  homeDir = "/home/${user}";
  mountUnit = "home-${user}.mount";
in
{
  # The persistent work disk is the home directory itself, so a checkout sits at
  # ~/<repo> rather than one level down. Everything the root image carries is
  # disposable; everything below here survives a rebuild.
  #
  # nofail, where state.nix's share is hard, and the difference is mkfs.
  # neededForBoot would move this into stage 1, stage 1 here is the scripted one
  # because boot.initrd.systemd.enable is unset, and the scripted stage 1 does
  # not run systemd-makefs. x-systemd.makefs is what formats a blank work disk
  # on its first boot, so hardening this would leave the first boot mounting an
  # unformatted device and failing with nothing to explain it. virtiofs needs no
  # mkfs, which is why the share can be hard and this cannot.
  fileSystems.${homeDir} = {
    device = "/dev/vdb";
    fsType = "ext4";

    options = [
      "nofail"
      "x-systemd.makefs"
      "x-systemd.device-timeout=30s"
    ];
  };

  systemd.services.uibox-home-perms = {
    description = "Hand the work disk to ${user} once it is mounted";

    wantedBy = [ "multi-user.target" ];
    after = [ mountUnit ];
    requires = [ mountUnit ];

    path = with pkgs; [
      coreutils
      util-linux
    ];

    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
    };

    script = ''
      # Not ConditionPathIsMountPoint. A condition that does not hold leaves the
      # unit skipped-and-successful, and an unmounted work disk is invisible
      # every other way: writes to ${homeDir} succeed, they just land on the
      # root overlay, and the loss surfaces only when the next base image
      # replaces that overlay and takes the checkout with it. An assertion would
      # be the same silence — a failed assert leaves the unit inactive rather
      # than failed, so `systemctl --failed` still answers clean. Exiting
      # non-zero is what puts this in the failed state something can notice.
      if ! mountpoint -q ${homeDir}; then
          echo "the work disk is not mounted: ${homeDir} is on the root overlay and the next base image will destroy it" >&2
          exit 1
      fi

      # mkfs leaves the filesystem root 0755 and owned by root; a home directory
      # is neither.
      chown ${user}:${user} ${homeDir}
      chmod 0700 ${homeDir}

      echo "work disk mounted at ${homeDir}, owned by ${user}"
    '';
  };
}
