{ modulesPath, ... }:

let
  spec = builtins.fromJSON (builtins.readFile ../lab.json);
in
{
  imports = [
    "${modulesPath}/virtualisation/disk-image.nix"
    "${modulesPath}/profiles/qemu-guest.nix"
  ];

  image.baseName = spec.name;
  image.format = "qcow2";
  image.efiSupport = true;

  boot.loader.systemd-boot.configurationLimit = 5;
  boot.loader.efi.canTouchEfiVariables = false;

  boot.kernelParams = [
    "console=ttyS0,115200"
    "console=tty0"
  ];

  networking.hostName = spec.name;

  time.timeZone = "Europe/Oslo";

  services.openssh = {
    enable = true;
    openFirewall = true;

    settings = {
      PasswordAuthentication = false;
      PermitRootLogin = "no";
    };
  };

  services.qemuGuest.enable = true;

  services.udev.extraRules = ''
    SUBSYSTEM=="cpu", ACTION=="add", TEST=="online", ATTR{online}=="0", ATTR{online}="1"
  '';

  services.chrony = {
    enable = true;
    extraConfig = "makestep 1.0 -1";
  };

  # Sized against the balloon target, not against what the kernel counts at
  # boot. The guest boots seeing `memory_mib` and the balloon only takes pages
  # away afterwards, so a percentage of MemTotal is a percentage of memory the
  # guest does not get to keep, and a compressed swap larger than the RAM
  # backing it thrashes rather than softening pressure. memoryMax is the
  # smaller of the two by definition, so the percentage stays as the shape of
  # the intent and this is what settles it.
  zramSwap = {
    enable = true;
    memoryPercent = 50;
    memoryMax = spec.current_memory_mib * 1024 * 1024 / 2;
  };

  # zram is RAM: reclaiming a page costs a compress, not a seek. 60 is the
  # number for a swap file on a disk and holds anonymous pages back until the
  # page cache has already been evicted; with zram as the only swap the kernel
  # should reach for it first.
  boot.kernel.sysctl."vm.swappiness" = 150;

  users.groups.fredrir.gid = 1000;

  users.users.fredrir = {
    isNormalUser = true;
    uid = 1000;
    group = "fredrir";

    extraGroups = [ "wheel" ];

    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIP7e69HsqnaggjeyngV0qUOurh5F9VMs7cudV0mu0QzD fhansteen@gmail.com"
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIH0jzc3S05J0DFj3W+Gv6J4Hc9fxvUjIOEuTWKfVnVY9 fhansteen@gmail.com"
    ];
  };

  security.sudo.wheelNeedsPassword = false;

  documentation.nixos.enable = false;

  # `documentation.doc` puts "doc" in environment.extraOutputsToInstall, which
  # pulls every package's doc output into the system closure. Nothing here can
  # read rendered HTML. man survives, and is 13 MiB.
  documentation.doc.enable = false;
  documentation.info.enable = false;

  nix.settings = {
    experimental-features = [
      "nix-command"
      "flakes"
    ];

    trusted-users = [
      "root"
      "fredrir"
    ];
  };

  nix.gc = {
    automatic = true;
    dates = "weekly";
    options = "--delete-older-than 14d";
    persistent = false;
    randomizedDelaySec = "45min";
  };

  nix.optimise.automatic = true;

  system.stateVersion = "26.05";
}
