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
    description = "Hand the home disk to ${user} once it is mounted";

    wantedBy = [ "multi-user.target" ];
    after = [ mountUnit ];
    requires = [ mountUnit ];

    unitConfig.ConditionPathIsMountPoint = homeDir;

    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;

      # mkfs leaves the filesystem root 0755 and owned by root; a home directory
      # is neither.
      ExecStart = [
        "${pkgs.coreutils}/bin/chown ${user}:${user} ${homeDir}"
        "${pkgs.coreutils}/bin/chmod 0700 ${homeDir}"
      ];
    };
  };
}
