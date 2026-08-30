let
  spec = builtins.fromJSON (builtins.readFile ../lab.json);

  inherit (spec) net;

  address = "${net.subnet_prefix}.${toString net.host}";
  gateway = "${net.subnet_prefix}.${toString net.gateway_host}";
in
{
  # backend/lab.json is the same file the hypervisor's tofu registry reads, so
  # the address baked in here and the address libvirt reserves cannot disagree.
  # With the lease settled there is nothing to negotiate at boot: dhcpcd cost
  # 7.6s of every cold boot probing for an address no other guest can be handed
  # and then waiting on router advertisements this NAT network never sends.
  networking.useDHCP = false;
  networking.useNetworkd = true;

  # libvirt assigns the NIC's PCIe slot, so the predictable name shifts when the
  # device set changes. There is exactly one physical link, so match its class
  # the way the nixpkgs DHCP fallback does rather than a name.
  systemd.network.networks."10-lan" = {
    matchConfig = {
      Type = "ether";
      Kind = "!*";
    };

    address = [ "${address}/${toString net.prefix_length}" ];

    routes = [
      { Gateway = gateway; }
    ];

    # Nothing on this network speaks IPv6: libvirt gives it no IPv6 address and
    # sends no advertisements. networkd holds a link "not configured" until it
    # gains IPv6LL, so duplicate address detection on an address no one else can
    # hold kept network-online.target 1.4s away. Loopback ::1 is untouched.
    networkConfig = {
      IPv6AcceptRA = false;
      LinkLocalAddressing = "no";
    };

    linkConfig.RequiredForOnline = "routable";
  };

  # networkd pulls in systemd-resolved by default, which would replace
  # /etc/resolv.conf with the 127.0.0.53 stub and stop resolving single-label
  # names. Keep openresolv and the file dhcpcd used to write, byte for byte.
  services.resolved.enable = false;

  networking.nameservers = [ gateway ];
  networking.domain = net.domain;
}
