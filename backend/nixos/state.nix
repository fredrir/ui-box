{
  boot.initrd.kernelModules = [ "virtiofs" ];

  # A virtiofs share of the hypervisor's storage/<lab>/state/ directory. The
  # golden screenshot store and the idle markers both live here, so it is what
  # survives a rebuild of the root image. services.uibox-runner.stateDir
  # defaults into it; the marker names are the host idle-stop timer's contract.
  #
  # nofail, and every consumer of the share says so for itself instead: a guest
  # that refuses to boot cannot be logged into to find out why, and both things
  # this carries already fail loudly on their own. uibox-goldens.service carries
  # RequiresMountsFor, and idle.nix asserts the mount point. A missing share is
  # then a reachable guest with two failed units rather than an emergency shell
  # on the serial console.
  fileSystems."/var/lib/ui-box-state" = {
    device = "uiboxstate";
    fsType = "virtiofs";

    options = [
      "nofail"
      "x-systemd.device-timeout=10s"
    ];
  };
}
