{ pkgs, ... }:

{
  hardware.graphics.enable = true;

  services.dbus.enable = true;
  programs.dconf.enable = true;

  fonts = {
    enableDefaultPackages = true;
    fontconfig.enable = true;

    packages = with pkgs; [
      dejavu_fonts
      jetbrains-mono
      liberation_ttf
      noto-fonts
    ];
  };

  environment.systemPackages = with pkgs; [
    chromium
    ffmpeg
    glib-networking
    imagemagick
    matchbox
    openbox
    webkitgtk_4_1
    wmctrl
    xdotool
    xdpyinfo
    xorg-server
    xset
    xterm
    xvfb-run
  ];
}
